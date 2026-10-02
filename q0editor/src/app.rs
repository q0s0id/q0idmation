use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::App;
use egui::Context;
use q0s_format::q0lang::runtime::{RuntimeAction, RuntimeDiagnostic, TimelineTarget};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, AudioClip, Layer, LayerKey, LayerKind, LayerMetadata, Path as VPath, Placement,
    ProjectDependencyKind, ProjectDependencyNode, ProjectDependencySource, ProjectScene, ProjectV2,
    Q0rg, Q0vAsset, Target, Transform2D, Tween, Vec2, VectorAsset,
};

use crate::file_io;
use crate::panels;
use crate::render::TextureCache;
use crate::state::{
    default_project, CameraTimelineSelection, ClipboardPayload, EditInPlaceContext,
    FrameScriptEditor, History, LibraryDragPayload, LibraryItem, PathRef, PlacementRef,
    ProjectState, SceneRename, Selection, Session, TimelineClipboard, TimelineLayerSelection,
    TimelineRigKeySelection, TimelineSelection, Tool, ToolState,
};

const MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS: usize = 64;

fn bbox_center(bounds: (f32, f32, f32, f32)) -> Vec2 {
    Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5)
}

fn transformed_bbox_size(bounds: (f32, f32, f32, f32), transform: Transform2D) -> (f32, f32) {
    let affine = Affine::from_transform(transform);
    let corners = [
        Vec2::new(bounds.0, bounds.1),
        Vec2::new(bounds.2, bounds.1),
        Vec2::new(bounds.0, bounds.3),
        Vec2::new(bounds.2, bounds.3),
    ]
    .map(|point| affine.apply(point));
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for point in corners {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    (max_x - min_x, max_y - min_y)
}

fn valid_library_folder_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part.trim() == part
                && part != "."
                && part != ".."
                && !part.chars().any(char::is_control)
        })
}

pub(crate) fn remapped_keyframe_placement_index(
    before: &ProjectV2,
    after: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
) -> usize {
    let Some(before_layer) = before
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return placement_idx;
    };
    let Some(source) = before_layer.placements.get(placement_idx) else {
        return placement_idx;
    };
    let Some(after_layer) = after
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return placement_idx;
    };

    if let Some(active) = crate::timeline_edit::keyframe_source_states_at(before_layer, frame)
        .into_iter()
        .find(|state| state.index == placement_idx)
    {
        if let Some((index, _)) =
            after_layer
                .placements
                .iter()
                .enumerate()
                .find(|(_, placement)| {
                    placement.frame == frame
                        && placement.instance_id == source.instance_id
                        && placement.target == source.target
                        && placement.transform == active.transform
                })
        {
            return index;
        }
    }

    let same_source = |placement: &&Placement| {
        placement.frame == source.frame
            && placement.instance_id == source.instance_id
            && placement.target == source.target
            && placement.transform == source.transform
    };
    let occurrence = before_layer.placements[..=placement_idx]
        .iter()
        .filter(same_source)
        .count()
        .saturating_sub(1);
    after_layer
        .placements
        .iter()
        .enumerate()
        .filter(|(_, placement)| {
            placement.frame == source.frame
                && placement.instance_id == source.instance_id
                && placement.target == source.target
                && placement.transform == source.transform
        })
        .nth(occurrence)
        .map(|(index, _)| index)
        .unwrap_or(placement_idx)
}

fn rebind_transform_pivot(
    pivot: &mut Option<crate::state::TransformPivot>,
    previous_selection: &Selection,
    selection: &Selection,
) {
    // Only carry the pivot when the same artwork was deliberately remapped.
    // An unrelated selection must never inherit another object's pivot.
    if let Some(pivot) = pivot.as_mut().filter(|pivot| {
        pivot.selection == *previous_selection && !matches!(selection, Selection::None)
    }) {
        pivot.selection = selection.clone();
    }
}

fn remap_selection_after_keyframes(
    before: &ProjectV2,
    after: &ProjectV2,
    selection: &mut Selection,
    pivot: &mut Option<crate::state::TransformPivot>,
    q0rg_id: u16,
    frame_by_layer: &std::collections::BTreeMap<u16, u16>,
) {
    let previous_selection = selection.clone();
    let remap_index = |reference_q0rg: u16, reference_layer: u16, placement_idx: usize| {
        if reference_q0rg != q0rg_id {
            return placement_idx;
        }
        frame_by_layer
            .get(&reference_layer)
            .map_or(placement_idx, |frame| {
                remapped_keyframe_placement_index(
                    before,
                    after,
                    reference_q0rg,
                    reference_layer,
                    placement_idx,
                    *frame,
                )
            })
    };
    let remap_placement = |reference: &mut PlacementRef| {
        reference.placement_idx = remap_index(
            reference.q0rg_id,
            reference.layer_id,
            reference.placement_idx,
        );
    };
    let remap_path = |reference: &mut PathRef| {
        reference.placement_idx = remap_index(
            reference.q0rg_id,
            reference.layer_id,
            reference.placement_idx,
        );
    };

    match &mut *selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => *placement_idx = remap_index(*q0rg_id, *layer_id, *placement_idx),
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            ..
        } => *placement_idx = remap_index(*q0rg_id, *layer_id, *placement_idx),
        Selection::Paths(paths) => paths.iter_mut().for_each(remap_path),
        Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            objects,
        } => {
            fills.iter_mut().for_each(remap_path);
            strokes.iter_mut().for_each(remap_path);
            stroke_segments
                .iter_mut()
                .for_each(|segment| remap_path(&mut segment.path));
            objects.iter_mut().for_each(remap_placement);
        }
        Selection::StrokeSegment(segment) => remap_path(&mut segment.path),
        Selection::PathPoints { path, .. } | Selection::SubselectPoints { path, .. } => {
            remap_path(path)
        }
        Selection::RawArea {
            placements,
            objects,
            ..
        } => {
            placements.iter_mut().for_each(remap_placement);
            objects.iter_mut().for_each(remap_placement);
        }
        Selection::Mixed { paths, objects } => {
            paths.iter_mut().for_each(remap_path);
            objects.iter_mut().for_each(remap_placement);
        }
        Selection::Multi(objects) => objects.iter_mut().for_each(remap_placement),
        Selection::None | Selection::Asset(_) | Selection::Q0rg(_) => {}
    }
    rebind_transform_pivot(pivot, &previous_selection, selection);
}

pub struct EditorApp {
    pub state: ProjectState,
    pub session: Session,
    pub history: History,
    pub textures: TextureCache,
    pub(crate) animation_preview_textures: TextureCache,
    pub(crate) animation_preview_revision: Option<(u64, u64)>,
    /// Persistent theme, drawing, and script-editor settings.
    /// `<config>/q0editor/settings.json`.
    pub settings: crate::settings::Settings,
    /// Separate non-modal export workshop and immutable export queue.
    pub q0enc: crate::q0enc::Q0EncState,
    /// FLA/XFL import/conversion workshop. Global because it can create a project from Home.
    pub q0fla_open: bool,
    /// Rolling low-overhead performance history copied with Ctrl+Shift+`.
    pub perf: crate::perf::PerfLog,
    pub(crate) release_feed: crate::release_feed::ReleaseFeedState,
    /// Set on the first `update()` so we apply the loaded theme once a
    /// real `Context` is available (the egui ctx isn't valid in
    /// `Default::default`).
    theme_applied: bool,
    /// Set on first `update()` after `q0lang::fonts::install` runs, so
    /// the persisted font choice (default `Consolas`) lands in egui before
    /// any TextEdit asks for `FontFamily::Name("q0lang")`.
    q0lang_font_installed: bool,
    last_window_title: Option<String>,
    pending: Vec<Action>,
    unsaved_action: Option<Action>,
    pending_asset_delete: Option<(u16, usize)>,
    pending_q0rg_delete: Option<(u16, usize)>,
    pending_layer_delete: Option<(u16, u16, usize, usize)>,
    pending_frame_truncate: Option<(u16, u16, usize, usize, usize, usize)>,
    pending_q0rg_replacement: Option<ReplaceQ0rgDialog>,
    media_import_job: Option<MediaImportJob>,
    fla_import_job: Option<FlaImportJob>,
    pending_clipboard_bitmap: Option<crate::system_clipboard::ClipboardBitmap>,
    editor_clipboard: Option<EditorClipboard>,
    native_paste_key_down: bool,
    audio_playback: Option<crate::audio::EditorAudioPlayback>,
    external_open_inbox: Option<crate::single_instance::ExternalOpenInbox>,
    show_project_tree: bool,
    pub(crate) library_source: Option<LibrarySource>,
    pub(crate) library_selection: Option<LibrarySelection>,
    pub(crate) library_multi_selection: Vec<LibrarySelection>,
    pub(crate) library_selection_anchor: Option<LibrarySelection>,
    pub(crate) library_search: String,
    pub(crate) library_sort_ascending: bool,
    pub(crate) library_sort_by_uses: bool,
    pub(crate) library_folder_filter: String,
    pub(crate) library_folder_draft: Option<LibraryFolderDraft>,
    pub(crate) library_cache: crate::panels::library::LibraryCache,
    document_tabs: Vec<DocumentTab>,
    active_document_tab: Option<u64>,
    last_document_tab: Option<u64>,
    next_document_tab_id: u64,
    show_home: bool,
    project_active: bool,
    timeline_visible: bool,
    new_project_dialog: Option<NewProjectSpec>,
    pub close_requested: bool,
}

struct MediaImportJob {
    path: PathBuf,
    receiver: Receiver<Result<Vec<u8>, String>>,
}

struct FlaImportJob {
    path: PathBuf,
    purpose: FlaImportPurpose,
    receiver: Receiver<Result<q0fla::ImportedProject, String>>,
    completed: Option<Result<q0fla::ImportedProject, String>>,
    started_at: Instant,
}

#[derive(Clone, Copy)]
enum FlaImportPurpose {
    Open,
    Convert,
}

#[derive(Debug, Clone)]
struct EditorClipboard {
    payload: ClipboardPayload,
    source_document_tab: Option<u64>,
    source_project: Option<Arc<ProjectV2>>,
    source_project_error: Option<String>,
    placement_depths: Vec<usize>,
}

#[derive(Debug, Clone)]
struct ReplaceQ0rgDialog {
    tab_id: Option<u64>,
    parent_q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    instance_id: u32,
    source_q0rg_id: u16,
    source_q0rg_name: String,
    source_folder: Option<String>,
    frame: u16,
    search: String,
    scroll_to_source: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LibraryFolderDraft {
    pub parent: Option<String>,
    pub rename_path: Option<String>,
    pub name: String,
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
    pending_frame_truncate: Option<(u16, u16, usize, usize, usize, usize)>,
    media_import_job: Option<MediaImportJob>,
}

#[allow(dead_code)]
pub(crate) struct Q0langDocument {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    pub(crate) dirty: bool,
    pub(crate) search: String,
    pub(crate) replace: String,
    pub(crate) show_find: bool,
    pub(crate) show_replace: bool,
}

enum DocumentKind {
    Project {
        workspace: Option<Box<ProjectWorkspace>>,
    },
    Q0lang(Q0langDocument),
}

struct DocumentTab {
    id: u64,
    kind: DocumentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKindSummary {
    Project,
    Q0lang,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTabSummary {
    pub id: u64,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub active: bool,
    pub kind: DocumentKindSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibrarySource {
    All,
    Project(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LibrarySelection {
    pub tab_id: u64,
    pub item: LibraryItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectLibrarySummary {
    pub tab_id: u64,
    pub label: String,
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

fn unsaved_project_file_name(project_name: &str) -> String {
    let name = project_name.trim();
    let name = if name.is_empty() { "untitled" } else { name };
    if name.to_ascii_lowercase().ends_with(".q1s") {
        name.to_string()
    } else {
        format!("{name}.q1s")
    }
}

fn project_tab_title(project_name: &str, path: Option<&std::path::Path>) -> String {
    if let Some(file_stem) = path
        .and_then(std::path::Path::file_stem)
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
    {
        return file_stem.to_string();
    }

    unsaved_project_file_name(project_name)
}

fn project_library_title(project_name: &str, path: Option<&std::path::Path>) -> String {
    if let Some(file_name) = path
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
    {
        return file_name.to_string();
    }

    let name = project_name.trim();
    let name = if name.is_empty() { "untitled" } else { name };
    if name.to_ascii_lowercase().ends_with(".q1s") {
        name.to_string()
    } else {
        format!("{name}.q1s")
    }
}

fn unique_symbol_name(
    project: &ProjectV2,
    requested: &str,
    exclude_q0rg_id: Option<u16>,
) -> String {
    let base = requested.trim();
    let occupied = |candidate: &str| {
        project
            .q0rgs
            .iter()
            .any(|q0rg| Some(q0rg.q0rg_id) != exclude_q0rg_id && q0rg.name == candidate)
    };
    if !occupied(base) {
        return base.to_string();
    }

    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base} #{suffix}");
        if !occupied(&candidate) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
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
    SwitchDocumentTab(u64),
    MoveDocumentTab(u64, usize),
    CloseDocumentTab(u64),
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
    OpenQ0Fla,
    ToggleQ0Fla,
    ImportFlaProject,
    ConvertFlaToQ1s,
    Exit,
    Undo,
    Redo,
    SelectTool(Tool),
    TogglePlay,
    ToggleTimeline,
    ToggleOutlineMode,
    FirstFrame,
    PreviousFrame,
    NextFrame,
    LastFrame,
    InsertFrame,
    InsertKeyframe,
    InsertBlankKeyframe,
    AddLayer,
    AddLayerFolder,
    AddLayerMask,
    RenameLayer(u16, u16, String),
    MoveLayer(u16, u16, i8),
    DropLayer(u16, u16, LayerDropTarget),
    DropLayers(u16, Vec<u16>, LayerDropTarget),
    MoveTimelineFrames(TimelineSelection, u16, u16),
    MoveRigTimelineKey(TimelineRigKeySelection, u16),
    MoveCameraFrames(CameraTimelineSelection, u16),
    IndentLayer(u16, u16),
    OutdentLayer(u16, u16),
    ToggleLayerFolder(u16, u16),
    ToggleLayerVisibility(u16, u16),
    ToggleLayerLock(u16, u16),
    ToggleLayerOutline(u16, u16),
    ToggleMaskArtworkVisibility(u16, u16),
    ToggleAllLayerLocks(u16),
    ToggleAllLayerVisibility(u16),
    ToggleAllLayerOutlines(u16),
    DeleteLayer(u16, u16),
    AddQ0rg,
    AddScene,
    SwitchScene(u16),
    RenameScene(u16, String),
    RenameLibraryItem(LibraryItem, String),
    DeleteLibraryItem(LibraryItem),
    DeleteLibraryItems(Vec<LibraryItem>),
    CreateLibraryFolder(String),
    RenameLibraryFolder(String, String),
    DeleteLibraryFolder(String),
    MoveLibraryItemToFolder(LibraryItem, Option<String>),
    SetQ0rgFrameCount(u16, u16),
    DeleteSelection,
    RemoveSelectedTimelineFrames,
    ConvertSelectionToQ0rg,
    BreakApartSelection,
    ConvertStrokeToFill,
    PlaceQ0rgInstance(u16),
    PlaceLibraryItemAt(LibraryItem, q0s_format::v2::Vec2),
    PlaceLibraryItemOnTimeline(LibraryItem, u16, u16),
    PlaceLibraryDragAt(LibraryDragPayload, q0s_format::v2::Vec2),
    PlaceLibraryDragOnTimeline(LibraryDragPayload, u16, u16),
    EnterQ0rg(u16),
    EnterQ0rgInPlace {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
    },
    BreadcrumbJumpTo(usize),
    ExitQ0rg,
    ToggleCredits,
    ToggleSettings,
    ToggleProjectTree,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CopySelection,
    CutSelection,
    Paste,
    PasteInPlace,
    DuplicateSelection,
    DuplicateSymbol(u16, u16, usize),
    AltDragDuplicateSelection(q0s_format::v2::Vec2),
    BringToFront,
    SendToBack,
    BringForward,
    SendBackward,
    SpreadBetweenLayers,
    SpreadBetweenKeyframes,
    FlipHorizontal,
    FlipVertical,
    RemoveFrame,
    ClearKeyframe,
    CreateMotionTween,
    /// Open the q0lang script editor for the given q0rg, or for the
    /// currently-selected one when `None`.
    OpenQ0langEditor(Option<u16>),
    CloseQ0langEditor,
    OpenFrameScriptEditor(u16, u16, u16),
    DeleteFrameScript(u16, u16, u16),
    SetFrameLabel(u16, u16, Option<String>),
    AddProjectDependency(PathBuf, Option<u16>),
    RenameProjectDependency(u16, String),
    ReparentProjectDependency(u16, Option<u16>),
    RemoveProjectDependency(u16),
    RelinkProjectDependency(u16, PathBuf),
    EmbedProjectDependency(u16),
    OpenProjectDependency(u16),
}

impl Action {
    fn perf_label(&self) -> &'static str {
        match self {
            Self::NewProject => "new-project",
            Self::CreateProject(_) => "create-project",
            Self::ShowHome => "show-home",
            Self::ResumeProject => "resume-project",
            Self::CycleDocumentTab(_) => "cycle-document-tab",
            Self::SwitchDocumentTab(_) => "switch-document-tab",
            Self::MoveDocumentTab(..) => "move-document-tab",
            Self::CloseDocumentTab(_) => "close-document-tab",
            Self::OpenProject => "open-file-dialog",
            Self::OpenProjectFromPath(_) => "open-project-path",
            Self::ImportBitmap => "import-bitmap-dialog",
            Self::ImportBitmapFromPath(_) => "import-bitmap-path",
            Self::ImportMedia => "import-media-dialog",
            Self::ImportMediaFromPath(_) => "import-media-path",
            Self::SaveProject => "save-project",
            Self::SaveProjectAs => "save-project-as-dialog",
            Self::OpenQ0Enc => "open-q0enc",
            Self::ToggleQ0Enc => "toggle-q0enc",
            Self::OpenQ0Fla => "open-q0fla",
            Self::ToggleQ0Fla => "toggle-q0fla",
            Self::ImportFlaProject => "import-fla-dialog",
            Self::ConvertFlaToQ1s => "convert-fla-dialog",
            Self::Exit => "exit",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::SelectTool(_) => "select-tool",
            Self::TogglePlay => "toggle-play",
            Self::ToggleTimeline => "toggle-timeline",
            Self::ToggleOutlineMode => "toggle-outline-mode",
            Self::FirstFrame => "first-frame",
            Self::PreviousFrame => "previous-frame",
            Self::NextFrame => "next-frame",
            Self::LastFrame => "last-frame",
            Self::InsertFrame => "insert-frame",
            Self::InsertKeyframe => "insert-keyframe",
            Self::InsertBlankKeyframe => "insert-blank-keyframe",
            Self::AddLayer => "add-layer",
            Self::AddLayerFolder => "add-layer-folder",
            Self::AddLayerMask => "add-layer-mask",
            Self::RenameLayer(..) => "rename-layer",
            Self::MoveLayer(..) => "move-layer",
            Self::DropLayer(..) => "drop-layer",
            Self::DropLayers(..) => "drop-layers",
            Self::MoveTimelineFrames(..) => "move-timeline-frames",
            Self::MoveRigTimelineKey(..) => "move-rig-timeline-key",
            Self::MoveCameraFrames(..) => "move-camera-frames",
            Self::IndentLayer(..) => "indent-layer",
            Self::OutdentLayer(..) => "outdent-layer",
            Self::ToggleLayerFolder(..) => "toggle-layer-folder",
            Self::ToggleLayerVisibility(..) => "toggle-layer-visibility",
            Self::ToggleLayerLock(..) => "toggle-layer-lock",
            Self::ToggleLayerOutline(..) => "toggle-layer-outline",
            Self::ToggleMaskArtworkVisibility(..) => "toggle-mask-artwork-visibility",
            Self::ToggleAllLayerLocks(_) => "toggle-all-layer-locks",
            Self::ToggleAllLayerVisibility(_) => "toggle-all-layer-visibility",
            Self::ToggleAllLayerOutlines(_) => "toggle-all-layer-outlines",
            Self::DeleteLayer(..) => "delete-layer",
            Self::AddQ0rg => "add-q0rg",
            Self::AddScene => "add-scene",
            Self::SwitchScene(_) => "switch-scene",
            Self::RenameScene(..) => "rename-scene",
            Self::RenameLibraryItem(..) => "rename-library-item",
            Self::DeleteLibraryItem(_) => "delete-library-item",
            Self::DeleteLibraryItems(_) => "delete-library-items",
            Self::CreateLibraryFolder(_) => "create-library-folder",
            Self::RenameLibraryFolder(..) => "rename-library-folder",
            Self::DeleteLibraryFolder(_) => "delete-library-folder",
            Self::MoveLibraryItemToFolder(..) => "move-library-item-to-folder",
            Self::SetQ0rgFrameCount(..) => "set-q0rg-frame-count",
            Self::DeleteSelection => "delete-selection",
            Self::RemoveSelectedTimelineFrames => "remove-selected-timeline-frames",
            Self::ConvertSelectionToQ0rg => "convert-selection-to-q0rg",
            Self::BreakApartSelection => "break-apart-selection",
            Self::ConvertStrokeToFill => "convert-stroke-to-fill",
            Self::PlaceQ0rgInstance(_) => "place-q0rg-instance",
            Self::PlaceLibraryItemAt(..) => "place-library-item-at",
            Self::PlaceLibraryItemOnTimeline(..) => "place-library-item-on-timeline",
            Self::PlaceLibraryDragAt(..) => "place-library-drag-at",
            Self::PlaceLibraryDragOnTimeline(..) => "place-library-drag-on-timeline",
            Self::EnterQ0rg(_) => "enter-q0rg",
            Self::EnterQ0rgInPlace { .. } => "enter-q0rg-in-place",
            Self::BreadcrumbJumpTo(_) => "breadcrumb-jump-to",
            Self::ExitQ0rg => "exit-q0rg",
            Self::ToggleCredits => "toggle-credits",
            Self::ToggleSettings => "toggle-settings",
            Self::ToggleProjectTree => "toggle-project-tree",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::ZoomReset => "zoom-reset",
            Self::CopySelection => "copy-selection",
            Self::CutSelection => "cut-selection",
            Self::Paste => "paste",
            Self::PasteInPlace => "paste-in-place",
            Self::DuplicateSelection => "duplicate-selection",
            Self::DuplicateSymbol(..) => "duplicate-symbol",
            Self::AltDragDuplicateSelection(_) => "alt-drag-duplicate-selection",
            Self::BringToFront => "bring-to-front",
            Self::SendToBack => "send-to-back",
            Self::BringForward => "bring-forward",
            Self::SendBackward => "send-backward",
            Self::SpreadBetweenLayers => "spread-between-layers",
            Self::SpreadBetweenKeyframes => "spread-between-keyframes",
            Self::FlipHorizontal => "flip-horizontal",
            Self::FlipVertical => "flip-vertical",
            Self::RemoveFrame => "remove-frame",
            Self::ClearKeyframe => "clear-keyframe",
            Self::CreateMotionTween => "create-motion-tween",
            Self::OpenQ0langEditor(_) => "open-q0lang-editor",
            Self::CloseQ0langEditor => "close-q0lang-editor",
            Self::OpenFrameScriptEditor(..) => "open-frame-script-editor",
            Self::DeleteFrameScript(..) => "delete-frame-script",
            Self::SetFrameLabel(..) => "set-frame-label",
            Self::AddProjectDependency(..) => "add-project-dependency",
            Self::RenameProjectDependency(..) => "rename-project-dependency",
            Self::ReparentProjectDependency(..) => "reparent-project-dependency",
            Self::RemoveProjectDependency(_) => "remove-project-dependency",
            Self::RelinkProjectDependency(..) => "relink-project-dependency",
            Self::EmbedProjectDependency(_) => "embed-project-dependency",
            Self::OpenProjectDependency(_) => "open-project-dependency",
        }
    }
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
    IntoMask(u16),
    OutOfMask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayerTreeNode {
    layer_id: u16,
    children: Vec<LayerTreeNode>,
}

fn build_layer_tree(project: &ProjectV2, q0rg_id: u16, layers: &[Layer]) -> Vec<LayerTreeNode> {
    fn build_node(
        project: &ProjectV2,
        q0rg_id: u16,
        layers: &[Layer],
        layer_id: u16,
    ) -> LayerTreeNode {
        let children = if project.layer_is_folder(q0rg_id, layer_id) {
            layers
                .iter()
                .rev()
                .filter(|layer| {
                    project.layer_parent_folder(q0rg_id, layer.layer_id) == Some(layer_id)
                })
                .map(|layer| build_node(project, q0rg_id, layers, layer.layer_id))
                .collect()
        } else {
            Vec::new()
        };
        LayerTreeNode { layer_id, children }
    }

    layers
        .iter()
        .rev()
        .filter(|layer| {
            project
                .layer_parent_folder(q0rg_id, layer.layer_id)
                .is_none()
        })
        .map(|layer| build_node(project, q0rg_id, layers, layer.layer_id))
        .collect()
}

fn take_layer_tree_node(nodes: &mut Vec<LayerTreeNode>, layer_id: u16) -> Option<LayerTreeNode> {
    if let Some(index) = nodes.iter().position(|node| node.layer_id == layer_id) {
        return Some(nodes.remove(index));
    }
    for node in nodes {
        if let Some(found) = take_layer_tree_node(&mut node.children, layer_id) {
            return Some(found);
        }
    }
    None
}

fn layer_tree_contains(node: &LayerTreeNode, layer_id: u16) -> bool {
    node.layer_id == layer_id
        || node
            .children
            .iter()
            .any(|child| layer_tree_contains(child, layer_id))
}

fn layer_tree_children_mut(
    nodes: &mut Vec<LayerTreeNode>,
    parent_folder_id: Option<u16>,
) -> Option<&mut Vec<LayerTreeNode>> {
    let Some(parent_id) = parent_folder_id else {
        return Some(nodes);
    };
    for node in nodes {
        if node.layer_id == parent_id {
            return Some(&mut node.children);
        }
        if let Some(children) = layer_tree_children_mut(&mut node.children, Some(parent_id)) {
            return Some(children);
        }
    }
    None
}

fn layer_child_at_parent(
    project: &ProjectV2,
    q0rg_id: u16,
    mut layer_id: u16,
    requested_parent: Option<u16>,
) -> Option<u16> {
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(layer_id) {
            return None;
        }
        let parent = project.layer_parent_folder(q0rg_id, layer_id);
        if parent == requested_parent {
            return Some(layer_id);
        }
        let parent_id = parent?;
        layer_id = parent_id;
    }
}

fn move_layer_tree_sibling(nodes: &mut [LayerTreeNode], layer_id: u16, direction: i8) -> bool {
    if let Some(index) = nodes.iter().position(|node| node.layer_id == layer_id) {
        let target = if direction < 0 {
            index.checked_sub(1)
        } else {
            (index + 1 < nodes.len()).then_some(index + 1)
        };
        if let Some(target) = target {
            nodes.swap(index, target);
            return true;
        }
        return false;
    }
    nodes
        .iter_mut()
        .any(|node| move_layer_tree_sibling(&mut node.children, layer_id, direction))
}

fn layer_tree_depth_within_limit(nodes: &[LayerTreeNode], depth: usize) -> bool {
    if nodes.is_empty() {
        return true;
    }
    if depth > q0s_format::v2::MAX_LAYER_FOLDER_NESTING_DEPTH {
        return false;
    }
    nodes
        .iter()
        .all(|node| layer_tree_depth_within_limit(&node.children, depth + 1))
}

fn flatten_layer_tree_model_order(
    nodes: &[LayerTreeNode],
    parent_folder_id: Option<u16>,
    model_ids: &mut Vec<u16>,
    parent_by_child: &mut std::collections::HashMap<u16, u16>,
) {
    for node in nodes.iter().rev() {
        flatten_layer_tree_model_order(
            &node.children,
            Some(node.layer_id),
            model_ids,
            parent_by_child,
        );
        if let Some(parent_id) = parent_folder_id {
            parent_by_child.insert(node.layer_id, parent_id);
        }
        model_ids.push(node.layer_id);
    }
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
        session.brush_mode = settings.brush_mode.into();
        session.advanced_brush = settings.advanced_brush.to_runtime();
        Self {
            state: ProjectState {
                project,
                file_path: None,
                dirty: false,
                revision: 0,
                render_revision: 0,
                library_revision: 0,
            },
            session,
            history: History::new(),
            textures: TextureCache::default(),
            animation_preview_textures: TextureCache::default(),
            animation_preview_revision: None,
            settings,
            q0enc,
            q0fla_open: false,
            perf: crate::perf::PerfLog::default(),
            release_feed: crate::release_feed::ReleaseFeedState::default(),
            theme_applied: false,
            q0lang_font_installed: false,
            last_window_title: None,
            pending: Vec::new(),
            unsaved_action: None,
            pending_asset_delete: None,
            pending_q0rg_delete: None,
            pending_layer_delete: None,
            pending_frame_truncate: None,
            pending_q0rg_replacement: None,
            media_import_job: None,
            fla_import_job: None,
            pending_clipboard_bitmap: None,
            editor_clipboard: None,
            native_paste_key_down: false,
            audio_playback: None,
            external_open_inbox: None,
            show_project_tree: false,
            library_source: None,
            library_selection: None,
            library_multi_selection: Vec::new(),
            library_selection_anchor: None,
            library_search: String::new(),
            library_sort_ascending: true,
            library_sort_by_uses: false,
            library_folder_filter: String::new(),
            library_folder_draft: None,
            library_cache: crate::panels::library::LibraryCache::default(),
            document_tabs: Vec::new(),
            active_document_tab: None,
            last_document_tab: None,
            next_document_tab_id: 1,
            show_home: true,
            project_active: false,
            timeline_visible: true,
            new_project_dialog: None,
            close_requested: false,
        }
    }
}

impl EditorApp {
    pub(crate) fn has_clipboard(&self) -> bool {
        self.editor_clipboard
            .as_ref()
            .is_some_and(|clipboard| !clipboard.payload.is_empty())
            || self
                .session
                .clipboard
                .as_ref()
                .is_some_and(|payload| !payload.is_empty())
    }

    fn store_editor_clipboard(&mut self, payload: ClipboardPayload) -> Option<String> {
        let placement_depths = if payload.timeline.is_none() && !payload.placements.is_empty() {
            let references = crate::selection_edit::selected_object_refs(
                &self.state.project,
                &self.session.selection,
            );
            let same_layer = references.first().is_some_and(|first| {
                references.iter().all(|reference| {
                    reference.q0rg_id == first.q0rg_id && reference.layer_id == first.layer_id
                })
            });
            if same_layer && references.len() == payload.placements.len() {
                let active = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == references[0].q0rg_id)
                    .and_then(|q0rg| {
                        q0rg.layers
                            .iter()
                            .find(|layer| layer.layer_id == references[0].layer_id)
                    })
                    .map(|layer| {
                        q0s_format::raster::active_placement_states_at(
                            layer,
                            self.session.current_frame,
                        )
                    });
                active
                    .and_then(|active| {
                        references
                            .iter()
                            .map(|reference| {
                                active
                                    .iter()
                                    .position(|state| state.index == reference.placement_idx)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let (source_project, source_project_error) =
            if payload.timeline.is_none() && !payload.placements.is_empty() {
                let roots = payload
                    .placements
                    .iter()
                    .map(|placement| match placement.target {
                        Target::Asset(id) => LibraryItem::Asset(id),
                        Target::Q0rg(id) => LibraryItem::Q0rg(id),
                    })
                    .collect::<Vec<_>>();
                match crate::library_transfer::snapshot_library_items(&self.state.project, &roots) {
                    Ok(project) => (Some(Arc::new(project)), None),
                    Err(error) => (None, Some(error)),
                }
            } else {
                (None, None)
            };

        self.session.clipboard = Some(payload.clone());
        self.editor_clipboard = Some(EditorClipboard {
            payload,
            source_document_tab: self.active_document_tab,
            source_project,
            source_project_error: source_project_error.clone(),
            placement_depths,
        });
        source_project_error
    }

    fn current_editor_clipboard(&self) -> Option<EditorClipboard> {
        self.editor_clipboard.clone().or_else(|| {
            self.session
                .clipboard
                .clone()
                .map(|payload| EditorClipboard {
                    payload,
                    source_document_tab: self.active_document_tab,
                    source_project: None,
                    source_project_error: None,
                    placement_depths: Vec::new(),
                })
        })
    }

    pub fn copy_performance_snapshot(&mut self, ctx: &Context) {
        let report = self.perf.snapshot_text(&self.session.selection);
        let frames = report
            .lines()
            .find_map(|line| line.strip_prefix("captured_frames="))
            .unwrap_or("0")
            .to_owned();
        ctx.output_mut(|output| output.copied_text = report);
        self.session.status = format!("performance log copied ({frames} frames)");
    }

    pub fn queue(&mut self, action: Action) {
        self.pending.push(action);
    }

    pub fn set_external_open_inbox(&mut self, inbox: crate::single_instance::ExternalOpenInbox) {
        self.external_open_inbox = Some(inbox);
    }

    fn poll_external_open_requests(&mut self, ctx: &Context) {
        let Some(inbox) = self.external_open_inbox.clone() else {
            return;
        };
        inbox.attach_context(ctx);
        let requests = inbox.drain();
        if requests.is_empty() {
            return;
        }

        // A file association launch should feel like opening another tab in
        // the already-running editor, including when that window is minimized.
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        for request in requests {
            if let Some(path) = request.path {
                self.handle(ctx, Action::OpenProjectFromPath(path));
            }
        }
    }

    pub fn home_visible(&self) -> bool {
        self.show_home
    }

    pub fn has_active_project(&self) -> bool {
        self.project_active
    }

    pub fn project_tree_open(&self) -> bool {
        self.show_project_tree
    }

    pub fn has_active_document(&self) -> bool {
        !self.show_home && self.active_document_tab.is_some()
    }

    pub fn active_document_is_q0lang(&self) -> bool {
        let Some(active_id) = self.active_document_tab else {
            return false;
        };
        self.document_tabs
            .iter()
            .any(|tab| tab.id == active_id && matches!(tab.kind, DocumentKind::Q0lang(_)))
    }

    pub fn has_open_projects(&self) -> bool {
        self.document_tabs
            .iter()
            .any(|tab| matches!(tab.kind, DocumentKind::Project { .. }))
    }

    pub fn document_tab_summaries(&self) -> Vec<DocumentTabSummary> {
        self.document_tabs
            .iter()
            .filter_map(|tab| {
                let active = self.active_document_tab == Some(tab.id);
                match &tab.kind {
                    DocumentKind::Project { workspace } => {
                        let (title, path, dirty) = if active && self.project_active {
                            (
                                project_tab_title(
                                    &self.state.project.meta.name,
                                    self.state.file_path.as_deref(),
                                ),
                                self.state.file_path.clone(),
                                self.state.dirty,
                            )
                        } else {
                            let workspace = workspace.as_ref()?;
                            (
                                project_tab_title(
                                    &workspace.state.project.meta.name,
                                    workspace.state.file_path.as_deref(),
                                ),
                                workspace.state.file_path.clone(),
                                workspace.state.dirty,
                            )
                        };
                        Some(DocumentTabSummary {
                            id: tab.id,
                            title,
                            path,
                            dirty,
                            active,
                            kind: DocumentKindSummary::Project,
                        })
                    }
                    DocumentKind::Q0lang(document) => Some(DocumentTabSummary {
                        id: tab.id,
                        title: document
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("script.q0l")
                            .to_string(),
                        path: Some(document.path.clone()),
                        dirty: document.dirty,
                        active,
                        kind: DocumentKindSummary::Q0lang,
                    }),
                }
            })
            .collect()
    }

    pub(crate) fn project_library_summaries(&self) -> Vec<ProjectLibrarySummary> {
        self.document_tabs
            .iter()
            .filter_map(|tab| {
                let active = self.active_document_tab == Some(tab.id) && self.project_active;
                let DocumentKind::Project { workspace } = &tab.kind else {
                    return None;
                };
                let (project_name, path) = if active {
                    (
                        self.state.project.meta.name.as_str(),
                        self.state.file_path.as_deref(),
                    )
                } else {
                    let workspace = workspace.as_ref()?;
                    (
                        workspace.state.project.meta.name.as_str(),
                        workspace.state.file_path.as_deref(),
                    )
                };
                Some(ProjectLibrarySummary {
                    tab_id: tab.id,
                    label: project_library_title(project_name, path),
                    active,
                })
            })
            .collect()
    }

    pub(crate) fn active_project_tab_id(&self) -> Option<u64> {
        if self.project_active {
            self.active_document_tab
        } else {
            None
        }
    }

    pub(crate) fn project_for_document_tab(&self, tab_id: u64) -> Option<&ProjectV2> {
        if self.active_document_tab == Some(tab_id) && self.project_active {
            return Some(&self.state.project);
        }
        let tab = self.document_tabs.iter().find(|tab| tab.id == tab_id)?;
        match &tab.kind {
            DocumentKind::Project { workspace } => {
                workspace.as_ref().map(|workspace| &workspace.state.project)
            }
            DocumentKind::Q0lang(_) => None,
        }
    }

    pub(crate) fn project_library_revision_for_tab(&self, tab_id: u64) -> Option<u64> {
        if self.active_document_tab == Some(tab_id) && self.project_active {
            return Some(self.state.library_revision());
        }
        let tab = self.document_tabs.iter().find(|tab| tab.id == tab_id)?;
        match &tab.kind {
            DocumentKind::Project { workspace } => workspace
                .as_ref()
                .map(|workspace| workspace.state.library_revision()),
            DocumentKind::Q0lang(_) => None,
        }
    }

    pub(crate) fn project_revision_for_tab(&self, tab_id: u64) -> Option<u64> {
        if self.active_document_tab == Some(tab_id) && self.project_active {
            return Some(self.state.revision());
        }
        let tab = self.document_tabs.iter().find(|tab| tab.id == tab_id)?;
        match &tab.kind {
            DocumentKind::Project { workspace } => workspace
                .as_ref()
                .map(|workspace| workspace.state.revision()),
            DocumentKind::Q0lang(_) => None,
        }
    }

    pub(crate) fn project_library_preview_frame(&self, tab_id: u64, q0rg_id: u16) -> u16 {
        if self.active_document_tab == Some(tab_id) && self.project_active {
            return if self.session.current_q0rg_id == q0rg_id {
                self.session.current_frame
            } else {
                0
            };
        }
        self.document_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| match &tab.kind {
                DocumentKind::Project { workspace } => workspace.as_ref(),
                DocumentKind::Q0lang(_) => None,
            })
            .map(|workspace| {
                if workspace.session.current_q0rg_id == q0rg_id {
                    workspace.session.current_frame
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }

    pub(crate) fn mark_placement_transforms_changed(&mut self, locators: &[(u16, u16, usize)]) {
        self.state.mark_artwork_dirty();
        let revision = (
            self.active_project_tab_id().unwrap_or(0),
            self.state.render_revision(),
        );
        self.textures
            .placement_transforms_changed(&self.state.project, revision, locators);
    }

    pub(crate) fn with_project_library_view_mut<R>(
        &mut self,
        tab_id: u64,
        render: impl FnOnce(&ProjectV2, &Session, &mut TextureCache, u64) -> R,
    ) -> Option<R> {
        if self.active_document_tab == Some(tab_id) && self.project_active {
            return Some(render(
                &self.state.project,
                &self.session,
                &mut self.textures,
                self.state.render_revision(),
            ));
        }
        let tab = self.document_tabs.iter_mut().find(|tab| tab.id == tab_id)?;
        match &mut tab.kind {
            DocumentKind::Project { workspace } => {
                let workspace = workspace.as_mut()?;
                Some(render(
                    &workspace.state.project,
                    &workspace.session,
                    &mut workspace.textures,
                    workspace.state.render_revision(),
                ))
            }
            DocumentKind::Q0lang(_) => None,
        }
    }

    pub(crate) fn active_q0lang_document_mut(&mut self) -> Option<&mut Q0langDocument> {
        let active_id = self.active_document_tab?;
        let tab = self
            .document_tabs
            .iter_mut()
            .find(|tab| tab.id == active_id)?;
        match &mut tab.kind {
            DocumentKind::Q0lang(document) => Some(document),
            DocumentKind::Project { .. } => None,
        }
    }

    pub(crate) fn active_q0lang_document(&self) -> Option<&Q0langDocument> {
        let active_id = self.active_document_tab?;
        let tab = self.document_tabs.iter().find(|tab| tab.id == active_id)?;
        match &tab.kind {
            DocumentKind::Q0lang(document) => Some(document),
            DocumentKind::Project { .. } => None,
        }
    }

    fn workspace_for_project(
        &self,
        mut project: q0s_format::v2::ProjectV2,
        file_path: Option<PathBuf>,
    ) -> ProjectWorkspace {
        let repaired = crate::timeline_edit::sync_project_folder_timelines(&mut project);
        let mut session = Session::for_project(&project);
        session.brush_cap = self.settings.brush_cap.into();
        session.brush = self.settings.brush.to_runtime();
        session.brush_mode = self.settings.brush_mode.into();
        session.advanced_brush = self.settings.advanced_brush.to_runtime();
        let mut q0enc = crate::q0enc::Q0EncState::default();
        q0enc.apply_persisted_preferences(
            self.settings.last_export_directory.clone(),
            self.settings.png_sequence_create_subfolder,
        );
        ProjectWorkspace {
            state: ProjectState {
                project,
                file_path,
                dirty: repaired,
                revision: 0,
                render_revision: 0,
                library_revision: 0,
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
        let show_shortcuts = self.session.show_shortcuts;
        let shortcut_capture = self.session.shortcut_capture;
        let shortcut_feedback = self.session.shortcut_feedback.clone();
        let show_credits = self.session.show_credits;
        let mut blank = self.workspace_for_project(default_project(), None);
        blank.session.show_settings = show_settings;
        blank.session.show_shortcuts = show_shortcuts;
        blank.session.shortcut_capture = shortcut_capture;
        blank.session.shortcut_feedback = shortcut_feedback;
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
        let show_shortcuts = self.session.show_shortcuts;
        let shortcut_capture = self.session.shortcut_capture;
        let shortcut_feedback = self.session.shortcut_feedback.clone();
        let show_credits = self.session.show_credits;
        workspace.session.show_settings = show_settings;
        workspace.session.show_shortcuts = show_shortcuts;
        workspace.session.shortcut_capture = shortcut_capture;
        workspace.session.shortcut_feedback = shortcut_feedback;
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
        self.project_active
            && (self.media_import_job.is_some()
                || self.q0enc.has_active_job()
                || self.unsaved_action.is_some())
    }

    fn store_active_workspace(&mut self) {
        if !self.project_active {
            return;
        }
        let Some(active_id) = self.active_document_tab else {
            return;
        };
        let workspace = self.take_active_workspace();
        if let Some(tab) = self
            .document_tabs
            .iter_mut()
            .find(|tab| tab.id == active_id)
        {
            if let DocumentKind::Project { workspace: slot } = &mut tab.kind {
                *slot = Some(Box::new(workspace));
            }
        }
        self.project_active = false;
    }

    fn deactivate_active_document(&mut self) {
        if self.project_active {
            self.store_active_workspace();
        }
    }

    fn switch_to_home(&mut self) -> bool {
        self.stop_audio_playback();
        if self.show_home {
            return true;
        }
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before switching tabs".to_string();
            return false;
        }
        self.last_document_tab = self.active_document_tab;
        self.deactivate_active_document();
        self.active_document_tab = None;
        self.show_home = true;
        self.session.playing = false;
        self.session.status = "home".to_string();
        true
    }

    fn cycle_document_tab(&mut self, direction: i8) {
        let mut documents = Vec::with_capacity(self.document_tabs.len() + 1);
        documents.push(None);
        documents.extend(self.document_tabs.iter().map(|tab| Some(tab.id)));
        if documents.len() <= 1 {
            return;
        }
        let current = if self.show_home {
            0
        } else {
            documents
                .iter()
                .position(|tab| *tab == self.active_document_tab)
                .unwrap_or(0)
        };
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(documents.len() - 1)
        } else {
            (current + 1) % documents.len()
        };
        match documents[next] {
            Some(tab_id) => {
                self.switch_to_document_tab(tab_id);
            }
            None => {
                self.switch_to_home();
            }
        }
    }

    fn switch_to_document_tab(&mut self, tab_id: u64) -> bool {
        self.stop_audio_playback();
        if self.active_document_tab == Some(tab_id) && !self.show_home {
            return true;
        }
        let Some(target_index) = self.document_tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before switching tabs".to_string();
            return false;
        }
        self.deactivate_active_document();

        let is_project = matches!(
            self.document_tabs[target_index].kind,
            DocumentKind::Project { .. }
        );
        if is_project {
            let workspace = match &mut self.document_tabs[target_index].kind {
                DocumentKind::Project { workspace } => workspace.take(),
                DocumentKind::Q0lang(_) => None,
            };
            let Some(workspace) = workspace else {
                return false;
            };
            self.install_workspace(*workspace);
            self.project_active = true;
        } else {
            self.project_active = false;
            self.session.playing = false;
            self.session.status = "q0lang source".to_string();
        }
        self.active_document_tab = Some(tab_id);
        self.last_document_tab = Some(tab_id);
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
        self.deactivate_active_document();
        let tab_id = self.next_document_tab_id;
        self.next_document_tab_id = self.next_document_tab_id.saturating_add(1);
        self.document_tabs.push(DocumentTab {
            id: tab_id,
            kind: DocumentKind::Project { workspace: None },
        });
        self.install_workspace(workspace);
        self.active_document_tab = Some(tab_id);
        self.last_document_tab = Some(tab_id);
        self.project_active = true;
        self.show_home = false;
        true
    }

    fn activate_q0lang_document(&mut self, document: Q0langDocument) -> bool {
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before opening another document"
                    .to_string();
            return false;
        }
        self.deactivate_active_document();
        let tab_id = self.next_document_tab_id;
        self.next_document_tab_id = self.next_document_tab_id.saturating_add(1);
        self.document_tabs.push(DocumentTab {
            id: tab_id,
            kind: DocumentKind::Q0lang(document),
        });
        self.active_document_tab = Some(tab_id);
        self.last_document_tab = Some(tab_id);
        self.project_active = false;
        self.show_home = false;
        true
    }

    fn document_tab_dirty(&self, tab_id: u64) -> bool {
        let Some(tab) = self.document_tabs.iter().find(|tab| tab.id == tab_id) else {
            return false;
        };
        match &tab.kind {
            DocumentKind::Project { workspace } => {
                if self.active_document_tab == Some(tab_id) && self.project_active {
                    self.state.dirty
                } else {
                    workspace
                        .as_ref()
                        .is_some_and(|workspace| workspace.state.dirty)
                }
            }
            DocumentKind::Q0lang(document) => document.dirty,
        }
    }

    fn close_document_tab(&mut self, tab_id: u64) {
        if self.active_document_tab != Some(tab_id) && !self.switch_to_document_tab(tab_id) {
            return;
        }
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before closing this tab".to_string();
            return;
        }
        if self.document_tab_dirty(tab_id) {
            self.unsaved_action = Some(Action::CloseDocumentTab(tab_id));
            return;
        }
        let Some(index) = self.document_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if self.project_active {
            let _discarded = self.take_active_workspace();
            self.project_active = false;
        }
        self.document_tabs.remove(index);
        self.active_document_tab = None;
        let replacement = self
            .document_tabs
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|i| self.document_tabs.get(i)))
            .map(|tab| tab.id);
        if let Some(replacement) = replacement {
            self.switch_to_document_tab(replacement);
        } else {
            self.show_home = true;
            self.last_document_tab = None;
            self.session.status = "home".to_string();
        }
    }

    fn first_dirty_document_tab(&self) -> Option<u64> {
        self.document_tabs
            .iter()
            .find_map(|tab| self.document_tab_dirty(tab.id).then_some(tab.id))
    }

    fn has_dirty_documents(&self) -> bool {
        self.first_dirty_document_tab().is_some()
    }

    fn document_tab_path(&self, tab: &DocumentTab) -> Option<PathBuf> {
        match &tab.kind {
            DocumentKind::Project { workspace } => {
                if self.active_document_tab == Some(tab.id) && self.project_active {
                    self.state.file_path.clone()
                } else {
                    workspace
                        .as_ref()
                        .and_then(|workspace| workspace.state.file_path.clone())
                }
            }
            DocumentKind::Q0lang(document) => Some(document.path.clone()),
        }
    }

    fn find_document_tab_by_path(&self, path: &std::path::Path) -> Option<u64> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.document_tabs.iter().find_map(|tab| {
            let candidate = self.document_tab_path(tab)?;
            let candidate =
                std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.to_path_buf());
            (candidate == canonical).then_some(tab.id)
        })
    }

    fn another_project_tab_uses_path(&self, path: &std::path::Path) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.document_tabs.iter().any(|tab| {
            if self.active_document_tab == Some(tab.id) {
                return false;
            }
            let DocumentKind::Project { workspace } = &tab.kind else {
                return false;
            };
            let Some(candidate) = workspace
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

    fn another_document_tab_uses_path(&self, path: &std::path::Path) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.document_tabs.iter().any(|tab| {
            if self.active_document_tab == Some(tab.id) {
                return false;
            }
            let Some(candidate) = self.document_tab_path(tab) else {
                return false;
            };
            let candidate =
                std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.to_path_buf());
            candidate == canonical
        })
    }

    pub(crate) fn resolve_project_dependency_path(&self, node_id: u16) -> Option<PathBuf> {
        let node = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)?;
        let ProjectDependencySource::External(path) = &node.source else {
            return None;
        };
        let path = PathBuf::from(path);
        if path.is_absolute() {
            Some(path)
        } else {
            self.state
                .file_path
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(|base| base.join(path))
        }
    }

    fn relative_path_between(base: &std::path::Path, target: &std::path::Path) -> Option<PathBuf> {
        use std::path::Component;

        let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
        let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
        let base_components = base.components().collect::<Vec<_>>();
        let target_components = target.components().collect::<Vec<_>>();

        let base_prefix = base_components
            .iter()
            .find_map(|component| match component {
                Component::Prefix(prefix) => Some(prefix.as_os_str()),
                _ => None,
            });
        let target_prefix = target_components
            .iter()
            .find_map(|component| match component {
                Component::Prefix(prefix) => Some(prefix.as_os_str()),
                _ => None,
            });
        if base_prefix != target_prefix || base.has_root() != target.has_root() {
            return None;
        }

        let mut common = 0usize;
        while common < base_components.len()
            && common < target_components.len()
            && base_components[common] == target_components[common]
        {
            common += 1;
        }
        let mut relative = PathBuf::new();
        for component in &base_components[common..] {
            if matches!(component, Component::Normal(_)) {
                relative.push("..");
            }
        }
        for component in &target_components[common..] {
            match component {
                Component::Normal(value) => relative.push(value),
                Component::ParentDir => relative.push(".."),
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        Some(if relative.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            relative
        })
    }

    fn rebase_project_dependencies_for_save(
        &self,
        project: &mut ProjectV2,
        destination: &std::path::Path,
    ) {
        let old_base = self
            .state
            .file_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let Some(new_base) = destination.parent() else {
            return;
        };
        for node in &mut project.runtime.project_graph.nodes {
            let ProjectDependencySource::External(stored) = &node.source else {
                continue;
            };
            let stored_path = PathBuf::from(stored);
            let absolute = if stored_path.is_absolute() {
                stored_path
            } else if let Some(old_base) = old_base.as_deref() {
                old_base.join(stored_path)
            } else {
                stored_path
            };
            let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
            let rebased = Self::relative_path_between(new_base, &absolute)
                .unwrap_or_else(|| absolute.clone());
            node.source =
                ProjectDependencySource::External(rebased.to_string_lossy().replace('\\', "/"));
        }
    }

    fn project_dependency_storage_path(&self, path: &std::path::Path) -> String {
        let normalized = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(base) = self
            .state
            .file_path
            .as_deref()
            .and_then(std::path::Path::parent)
        {
            if let Some(relative) = Self::relative_path_between(base, &normalized) {
                return relative.to_string_lossy().replace('\\', "/");
            }
        }
        normalized.to_string_lossy().to_string()
    }

    fn project_dependency_kind(path: &std::path::Path) -> Option<ProjectDependencyKind> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("q0s") => Some(ProjectDependencyKind::Movie),
            Some("q0l" | "q0lang") => Some(ProjectDependencyKind::Q0lang),
            _ => None,
        }
    }

    fn project_dependency_alias_base(path: &std::path::Path) -> String {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dependency");
        let mut alias = String::new();
        for (index, ch) in stem.chars().enumerate() {
            let valid = if index == 0 {
                ch.is_ascii_alphabetic() || ch == '_'
            } else {
                ch.is_ascii_alphanumeric() || ch == '_'
            };
            if valid {
                alias.push(ch);
            } else if index == 0 && ch.is_ascii_digit() {
                alias.push('_');
                alias.push(ch);
            } else {
                alias.push('_');
            }
        }
        if alias.is_empty() {
            "dependency".to_string()
        } else {
            alias
        }
    }

    fn project_dependency_alias_valid(alias: &str) -> bool {
        let mut chars = alias.chars();
        matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
            && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }

    fn unique_project_dependency_alias(&self, parent: Option<u16>, base: &str) -> String {
        let used = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .filter(|node| node.parent_node_id == parent)
            .map(|node| node.alias.as_str())
            .collect::<std::collections::HashSet<_>>();
        if !used.contains(base) {
            return base.to_string();
        }
        for suffix in 2_u32.. {
            let candidate = format!("{base}_{suffix}");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
        }
        unreachable!()
    }

    fn project_dependency_reparent_would_cycle(&self, node_id: u16, parent: Option<u16>) -> bool {
        let Some(mut cursor) = parent else {
            return false;
        };
        let nodes = &self.state.project.runtime.project_graph.nodes;
        let mut seen = std::collections::HashSet::new();
        loop {
            if cursor == node_id || !seen.insert(cursor) {
                return true;
            }
            let Some(node) = nodes.iter().find(|node| node.node_id == cursor) else {
                return false;
            };
            let Some(parent) = node.parent_node_id else {
                return false;
            };
            cursor = parent;
        }
    }

    fn add_project_dependency(&mut self, path: &std::path::Path, parent: Option<u16>) {
        let Some(kind) = Self::project_dependency_kind(path) else {
            self.session.status = "project tree accepts .q0s, .q0l and .q0lang".to_string();
            return;
        };
        if !path.is_file() {
            self.session.status = format!("dependency does not exist: {}", path.display());
            return;
        }
        if parent.is_some_and(|parent| {
            !self
                .state
                .project
                .runtime
                .project_graph
                .nodes
                .iter()
                .any(|node| node.node_id == parent)
        }) {
            self.session.status = "dependency parent no longer exists".to_string();
            return;
        }
        let node_id = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .map(|node| node.node_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let base = Self::project_dependency_alias_base(path);
        let alias = self.unique_project_dependency_alias(parent, &base);
        let source = ProjectDependencySource::External(self.project_dependency_storage_path(path));
        self.history.snapshot(&self.state.project);
        self.state
            .project
            .runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id,
                parent_node_id: parent,
                alias: alias.clone(),
                kind,
                source,
            });
        self.state.mark_dirty();
        self.session.project_graph_selected = Some(node_id);
        self.session.status = format!("linked {alias}");
    }

    fn rename_project_dependency(&mut self, node_id: u16, alias: String) {
        let alias = alias.trim().to_string();
        if !Self::project_dependency_alias_valid(&alias) {
            self.session.status = "dependency alias must be a q0lang identifier".to_string();
            return;
        }
        let Some(parent) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.parent_node_id)
        else {
            return;
        };
        if self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .any(|node| {
                node.node_id != node_id && node.parent_node_id == parent && node.alias == alias
            })
        {
            self.session.status = "a sibling dependency already uses that alias".to_string();
            return;
        }
        self.history.snapshot(&self.state.project);
        if let Some(node) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.alias = alias.clone();
            self.state.mark_dirty();
            self.session.status = format!("renamed dependency to {alias}");
        }
    }

    fn reparent_project_dependency(&mut self, node_id: u16, parent: Option<u16>) {
        if parent == Some(node_id) || self.project_dependency_reparent_would_cycle(node_id, parent)
        {
            self.session.status = "dependency reparent would create a cycle".to_string();
            return;
        }
        if parent.is_some_and(|parent| {
            !self
                .state
                .project
                .runtime
                .project_graph
                .nodes
                .iter()
                .any(|node| node.node_id == parent)
        }) {
            self.session.status = "dependency parent no longer exists".to_string();
            return;
        }
        let Some(current) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .cloned()
        else {
            return;
        };
        if self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .any(|node| {
                node.node_id != node_id
                    && node.parent_node_id == parent
                    && node.alias == current.alias
            })
        {
            self.session.status = "target parent already has a child with this alias".to_string();
            return;
        }
        if current.parent_node_id == parent {
            return;
        }
        self.history.snapshot(&self.state.project);
        if let Some(node) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.parent_node_id = parent;
            self.state.mark_dirty();
            self.session.status = "dependency moved".to_string();
        }
    }

    fn remove_project_dependency(&mut self, node_id: u16) {
        let nodes = &self.state.project.runtime.project_graph.nodes;
        if !nodes.iter().any(|node| node.node_id == node_id) {
            return;
        }
        let mut removed = std::collections::HashSet::from([node_id]);
        loop {
            let before = removed.len();
            for node in nodes {
                if node
                    .parent_node_id
                    .is_some_and(|parent| removed.contains(&parent))
                {
                    removed.insert(node.node_id);
                }
            }
            if removed.len() == before {
                break;
            }
        }
        self.history.snapshot(&self.state.project);
        self.state
            .project
            .runtime
            .project_graph
            .nodes
            .retain(|node| !removed.contains(&node.node_id));
        if self
            .session
            .project_graph_selected
            .is_some_and(|selected| removed.contains(&selected))
        {
            self.session.project_graph_selected = None;
            self.session.project_graph_rename = None;
        }
        self.state.mark_dirty();
        self.session.status = format!("removed {} dependency node(s)", removed.len());
    }

    fn relink_project_dependency(&mut self, node_id: u16, path: &std::path::Path) {
        let Some(kind) = Self::project_dependency_kind(path) else {
            self.session.status = "relink expects .q0s, .q0l or .q0lang".to_string();
            return;
        };
        let Some(expected_kind) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.kind)
        else {
            return;
        };
        if kind != expected_kind {
            self.session.status = "relink file type does not match dependency kind".to_string();
            return;
        }
        if !path.is_file() {
            self.session.status = "relink target does not exist".to_string();
            return;
        }
        let stored = self.project_dependency_storage_path(path);
        self.history.snapshot(&self.state.project);
        if let Some(node) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.source = ProjectDependencySource::External(stored);
            self.state.mark_dirty();
            self.session.status = "dependency relinked".to_string();
        }
    }

    fn embed_project_dependency(&mut self, node_id: u16) {
        let Some(path) = self.resolve_project_dependency_path(node_id) else {
            self.session.status = "dependency is already embedded".to_string();
            return;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            self.session.status = format!("dependency is missing: {}", path.display());
            return;
        };
        if metadata.len() > 256 * 1024 * 1024 {
            self.session.status = "dependency is too large to embed".to_string();
            return;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            self.session.status = format!("failed to read dependency: {}", path.display());
            return;
        };
        self.history.snapshot(&self.state.project);
        if let Some(node) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
        {
            node.source = ProjectDependencySource::Embedded(bytes);
            self.state.mark_dirty();
            self.session.status = "dependency embedded into project".to_string();
        }
    }

    fn open_project_dependency(&mut self, ctx: &Context, node_id: u16) {
        let Some(node) = self
            .state
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .cloned()
        else {
            return;
        };
        match (node.kind, node.source) {
            (ProjectDependencyKind::Q0lang, ProjectDependencySource::External(_)) => {
                if let Some(path) = self.resolve_project_dependency_path(node_id) {
                    self.open_from_path(ctx, &path);
                }
            }
            (ProjectDependencyKind::Movie, ProjectDependencySource::External(_)) => {
                let Some(path) = self.resolve_project_dependency_path(node_id) else {
                    return;
                };
                if !path.is_file() {
                    self.session.status = format!("dependency is missing: {}", path.display());
                    return;
                }
                let player = std::env::current_exe()
                    .ok()
                    .map(|exe| exe.with_file_name("q0player.exe"));
                if let Some(player) = player.filter(|player| player.is_file()) {
                    match std::process::Command::new(player).arg(&path).spawn() {
                        Ok(_) => self.session.status = format!("opened {}", path.display()),
                        Err(error) => {
                            self.session.status = format!("could not open movie: {error}")
                        }
                    }
                } else {
                    self.session.status =
                        format!("q0player is unavailable; linked movie: {}", path.display());
                }
            }
            (_, ProjectDependencySource::Embedded(_)) => {
                self.session.status =
                    "embedded dependencies are read-only in the project tree".to_string()
            }
        }
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
        self.settings.new_project_width = spec.width;
        self.settings.new_project_height = spec.height;
        self.settings.new_project_fps = spec.fps;
        self.settings.new_project_frame_count = spec.frame_count;
        self.settings.save();
        self.new_project_dialog = None;
        self.session.status = format!(
            "new project: {}x{}, {} fps, {} frames",
            spec.width, spec.height, spec.fps, spec.frame_count
        );
    }

    fn drain_actions(&mut self, ctx: &Context) {
        let actions = std::mem::take(&mut self.pending);
        for action in actions {
            let label = action.perf_label();
            let started = Instant::now();
            self.handle(ctx, action);
            self.perf.record_action(label, started.elapsed());
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
        let timeline_edit = matches!(
            &action,
            Action::InsertFrame
                | Action::InsertKeyframe
                | Action::InsertBlankKeyframe
                | Action::RemoveFrame
                | Action::RemoveSelectedTimelineFrames
                | Action::ClearKeyframe
                | Action::MoveTimelineFrames(..)
                | Action::MoveCameraFrames(..)
                | Action::MoveRigTimelineKey(..)
                | Action::SetQ0rgFrameCount(..)
                | Action::SpreadBetweenKeyframes
        ) || (matches!(
            &action,
            Action::DeleteSelection
                | Action::CutSelection
                | Action::Paste
                | Action::PasteInPlace
                | Action::DuplicateSelection
        ) && (self.session.timeline_selection.is_some()
            || self.session.timeline_cell_selection.is_some()
            || self.session.camera_timeline_selection.is_some()));
        let before = self.session.timeline_history_position();
        let old_frame_count = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| q.frame_count);
        let history_revision = self.history.snapshot_revision();
        self.handle_inner(ctx, action);
        if timeline_edit && self.history.snapshot_revision() != history_revision {
            if crate::timeline_edit::sync_folder_timelines(
                &mut self.state.project,
                self.session.current_q0rg_id,
            ) {
                self.state.mark_dirty();
            }
            let frame_count = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == self.session.current_q0rg_id)
                .map(|q| q.frame_count);
            if frame_count < old_frame_count {
                self.session.reconcile_with(&self.state.project);
            }
            self.history.set_undo_timeline_position(before);
        }
    }

    fn handle_inner(&mut self, ctx: &Context, action: Action) {
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
                self.new_project_dialog = Some(NewProjectSpec {
                    width: self.settings.new_project_width,
                    height: self.settings.new_project_height,
                    fps: self.settings.new_project_fps,
                    frame_count: self.settings.new_project_frame_count,
                    ..NewProjectSpec::default()
                });
            }
            Action::CreateProject(spec) => {
                self.create_project(spec);
            }
            Action::ShowHome => {
                self.switch_to_home();
            }
            Action::ResumeProject => {
                if let Some(tab_id) = self.last_document_tab {
                    self.switch_to_document_tab(tab_id);
                }
            }
            Action::CycleDocumentTab(direction) => {
                self.cycle_document_tab(direction);
            }
            Action::SwitchDocumentTab(tab_id) => {
                self.switch_to_document_tab(tab_id);
            }
            Action::MoveDocumentTab(tab_id, target_index) => {
                if let Some(source_index) =
                    self.document_tabs.iter().position(|tab| tab.id == tab_id)
                {
                    if source_index != target_index {
                        let tab = self.document_tabs.remove(source_index);
                        self.document_tabs
                            .insert(target_index.min(self.document_tabs.len()), tab);
                    }
                }
            }
            Action::CloseDocumentTab(tab_id) => {
                self.close_document_tab(tab_id);
            }
            Action::OpenProject => {
                if let Some(path) = file_io::pick_open_path() {
                    self.handle(ctx, Action::OpenProjectFromPath(path));
                }
            }
            Action::OpenProjectFromPath(path) => {
                self.open_from_path(ctx, &path);
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
                if !crate::text_tool::finish_edit(self) {
                    return;
                }
                if self.active_document_is_q0lang() {
                    self.save_current_q0lang();
                } else if self.project_active {
                    let path = self.state.file_path.clone();
                    match path {
                        Some(p) => {
                            self.save_to(&p);
                        }
                        None => self.handle(ctx, Action::SaveProjectAs),
                    }
                }
            }
            Action::SaveProjectAs => {
                if !crate::text_tool::finish_edit(self) {
                    return;
                }
                if self.active_document_is_q0lang() {
                    let suggested = self
                        .active_q0lang_document()
                        .map(|document| document.path.clone());
                    if let Some(path) = file_io::pick_q0lang_save_path(suggested.as_deref()) {
                        self.save_q0lang_to(&path);
                    }
                } else if self.project_active {
                    let suggested = self.state.file_path.clone().unwrap_or_else(|| {
                        PathBuf::from(unsaved_project_file_name(&self.state.project.meta.name))
                    });
                    if let Some(path) = file_io::pick_save_path(Some(&suggested)) {
                        self.save_to(&path);
                    }
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
            Action::OpenQ0Fla => {
                self.q0fla_open = true;
                self.session.status = "q0fla opened".to_string();
            }
            Action::ToggleQ0Fla => {
                self.q0fla_open = !self.q0fla_open;
                self.session.status = if self.q0fla_open {
                    "q0fla opened"
                } else {
                    "q0fla closed"
                }
                .to_string();
            }
            Action::ImportFlaProject => {
                if let Some(path) = file_io::pick_fla_import_path() {
                    self.open_from_path(ctx, &path);
                }
            }
            Action::ConvertFlaToQ1s => {
                if let Some(source) = file_io::pick_fla_import_path() {
                    self.start_fla_import(ctx, &source, FlaImportPurpose::Convert);
                }
            }
            Action::Exit => {
                if self.active_workspace_busy() {
                    self.session.status =
                        "finish or cancel the active import/export before closing q0editor"
                            .to_string();
                } else if let Some(dirty_tab) = self.first_dirty_document_tab() {
                    if self.active_document_tab != Some(dirty_tab) {
                        self.switch_to_document_tab(dirty_tab);
                    }
                    self.unsaved_action = Some(Action::Exit);
                } else {
                    self.close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Undo => {
                let history_started = Instant::now();
                let timeline_position = self.history.undo_timeline_position();
                let before = self.session.timeline_history_position();
                let current = std::mem::replace(&mut self.state.project, default_project());
                let session = &mut self.session;
                match self.history.pop_undo(current, |from, to| {
                    if let Some(position) = timeline_position {
                        session.restore_timeline_history_position(position);
                    }
                    session.reconcile_after_history(from, to)
                }) {
                    Ok(prior) => {
                        if timeline_position.is_some() {
                            self.history.set_redo_timeline_position(before);
                        }
                        self.state.project = prior;
                        self.textures.invalidate();
                        self.state.mark_dirty();
                        self.session.status = "undo".to_string();
                    }
                    Err(current) => {
                        self.state.project = current;
                        self.session.status = "nothing to undo".to_string();
                    }
                }
                self.perf.record(
                    crate::perf::PerfSpan::HistoryTransition,
                    history_started.elapsed(),
                );
            }
            Action::Redo => {
                let history_started = Instant::now();
                let timeline_position = self.history.redo_timeline_position();
                let before = self.session.timeline_history_position();
                let current = std::mem::replace(&mut self.state.project, default_project());
                let session = &mut self.session;
                match self.history.pop_redo(current, |from, to| {
                    if let Some(position) = timeline_position {
                        session.restore_timeline_history_position(position);
                    }
                    session.reconcile_after_history(from, to)
                }) {
                    Ok(next) => {
                        if timeline_position.is_some() {
                            self.history.set_undo_timeline_position(before);
                        }
                        self.state.project = next;
                        self.textures.invalidate();
                        self.state.mark_dirty();
                        self.session.status = "redo".to_string();
                    }
                    Err(current) => {
                        self.state.project = current;
                        self.session.status = "nothing to redo".to_string();
                    }
                }
                self.perf.record(
                    crate::perf::PerfSpan::HistoryTransition,
                    history_started.elapsed(),
                );
            }
            Action::SelectTool(tool) => {
                if !crate::text_tool::finish_edit(self) {
                    return;
                }
                // Subselect anchors are editor control points, not a partial raw-fill
                // selection. Once Subselect is left, promote them to the containing
                // raw path so Select/Free Transform gets the same whole-path semantics
                // as a normal click instead of closing an open stroke into a fake fill.
                if self.session.current_tool == Tool::Subselect && tool != Tool::Subselect {
                    if let Selection::SubselectPoints { path, .. } = self.session.selection.clone()
                    {
                        self.session.selection = Selection::Path {
                            q0rg_id: path.q0rg_id,
                            layer_id: path.layer_id,
                            placement_idx: path.placement_idx,
                            path_idx: path.path_idx,
                        };
                    }
                }
                self.session.current_tool = tool;
                self.session.tool_state = ToolState::Idle;
                self.session.status = format!("tool: {}", tool.label());
            }
            Action::TogglePlay => {
                self.session.pending_timeline_frame = None;
                let wants_play = !self.session.playing;
                self.session.playing = wants_play;
                self.session.last_tick = Instant::now();
                if wants_play {
                    if let Some(&(true, start, end)) = self
                        .session
                        .preview_loop_ranges
                        .get(&self.session.current_q0rg_id)
                    {
                        let total = self
                            .state
                            .project
                            .q0rgs
                            .iter()
                            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
                            .map(|q| q.frame_count.max(1))
                            .unwrap_or(1);
                        let start = start.min(total - 1);
                        let end = end.max(start).min(total - 1);
                        if self.session.current_frame < start || self.session.current_frame > end {
                            self.session.current_frame = start;
                        }
                    }
                    self.initialize_preview_q0lang_if_needed();
                    self.session.preview_audio_resync_needed = false;
                    if self.session.playing {
                        self.session.status = "playing".to_string();
                        if let Err(error) = self.refresh_audio_playback() {
                            self.session.status = format!("playing; audio warning: {error}");
                        }
                    } else {
                        if let Some(audio) = self.audio_playback.as_ref() {
                            audio.set_playing(false);
                        }
                        self.session.status = "stopped by frame code".to_string();
                    }
                } else {
                    if let Some(audio) = self.audio_playback.as_ref() {
                        audio.set_playing(false);
                    }
                    self.session.status = "paused".to_string();
                }
            }
            Action::ToggleTimeline => {
                self.timeline_visible = !self.timeline_visible;
            }
            Action::ToggleOutlineMode => {
                self.session.outline_mode = !self.session.outline_mode;
                self.session.status = if self.session.outline_mode {
                    "outline mode on".to_string()
                } else {
                    "outline mode off".to_string()
                };
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
                if self.camera_track_is_selected() {
                    self.insert_camera_frame_smart();
                } else {
                    self.insert_frame_smart();
                }
            }
            Action::InsertKeyframe => {
                if let Some(selection) =
                    self.session.timeline_rig_key_selection.filter(|selection| {
                        selection.q0rg_id == self.session.current_q0rg_id
                            && matches!(&self.session.selection, Selection::None)
                    })
                {
                    if self
                        .state
                        .project
                        .layer_is_locked(selection.q0rg_id, selection.layer_id)
                    {
                        self.session.status =
                            "rig keys on locked layers cannot be edited".to_string();
                        return;
                    }
                    let before = self.state.project.clone();
                    if crate::timeline_edit::insert_rig_key_for_layer(
                        &mut self.state.project,
                        selection.q0rg_id,
                        selection.layer_id,
                        selection.frame,
                    ) {
                        self.history.snapshot(&before);
                        self.state.mark_dirty();
                        self.session.current_layer_id = selection.layer_id;
                        self.session.current_frame = selection.frame;
                        self.session.pending_timeline_frame = None;
                        self.session.status = format!(
                            "inserted rig key at frame {} without changing artwork keys",
                            u32::from(selection.frame) + 1
                        );
                    } else {
                        self.session.status = "no rig values are bound to this layer".to_string();
                    }
                } else if self.camera_track_is_selected() {
                    self.insert_camera_keyframe_smart();
                } else if self.session.pending_timeline_frame.is_none()
                    && self.timeline_selection_is_multi()
                {
                    self.materialize_timeline_selection();
                } else if self.current_layer_is_folder() {
                    self.session.status = "folders cannot contain keyframes".to_string();
                } else {
                    self.insert_keyframe_smart();
                }
            }
            Action::InsertBlankKeyframe => {
                if self
                    .session
                    .timeline_rig_key_selection
                    .is_some_and(|selection| {
                        selection.q0rg_id == self.session.current_q0rg_id
                            && matches!(&self.session.selection, Selection::None)
                    })
                {
                    self.session.status = "rig tracks cannot contain blank keyframes".to_string();
                } else if self.camera_track_is_selected() {
                    self.session.status = "camera track cannot contain blank keyframes".to_string();
                } else if self.session.pending_timeline_frame.is_none()
                    && self.timeline_selection_is_multi()
                {
                    self.blank_timeline_selection();
                } else if self.current_layer_is_folder() {
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
                                (end, metadata.parent_folder_id)
                            } else if metadata.kind == LayerKind::Mask {
                                (index, metadata.parent_folder_id)
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
                        timeline_frame_count: None,
                        name: format!("Layer {next_id}"),
                        explicit_keyframes: vec![0],
                        placements: Vec::new(),
                    },
                );
                if parent_folder_id.is_some() {
                    self.state.project.layer_metadata.insert(
                        LayerKey::new(q0rg_id, next_id),
                        LayerMetadata {
                            kind: LayerKind::Normal,
                            parent_folder_id,
                            mask_layer_id: None,
                            collapsed: false,
                            hidden: false,
                            locked: false,
                        },
                    );
                }
                self.session.current_layer_id = next_id;
                self.state.mark_dirty();
                self.session.status = "layer added".to_string();
            }
            Action::AddLayerFolder => {
                let q0rg_id = self.session.current_q0rg_id;
                let selected_id = self.session.current_layer_id;
                if self.state.project.layer_is_folder(q0rg_id, selected_id)
                    && self.state.project.layer_folder_depth(q0rg_id, selected_id)
                        >= q0s_format::v2::MAX_LAYER_FOLDER_NESTING_DEPTH
                {
                    self.session.status = "maximum layer folder nesting depth reached".to_string();
                    return;
                }
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
                                (index, Some(selected_id))
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
                        timeline_frame_count: None,
                        name: format!("Folder {next_id}"),
                        explicit_keyframes: Vec::new(),
                        placements: Vec::new(),
                    },
                );
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, next_id),
                    LayerMetadata {
                        kind: LayerKind::Folder,
                        parent_folder_id,
                        mask_layer_id: None,
                        collapsed: false,
                        hidden: false,
                        locked: false,
                    },
                );
                self.session.current_layer_id = next_id;
                self.state.mark_dirty();
                self.session.status = "layer folder added".to_string();
            }
            Action::AddLayerMask => {
                let q0rg_id = self.session.current_q0rg_id;
                let target_layer_id = self.session.current_layer_id;
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let Some(target_index) = q0rg
                    .layers
                    .iter()
                    .position(|layer| layer.layer_id == target_layer_id)
                else {
                    return;
                };
                if self.state.project.layer_is_folder(q0rg_id, target_layer_id) {
                    self.session.status = "select a keyed layer to mask".to_string();
                    return;
                }
                let target_metadata = self.state.project.layer_metadata(q0rg_id, target_layer_id);
                let parent_folder_id = target_metadata.parent_folder_id;
                let next_id = q0rg
                    .layers
                    .iter()
                    .map(|layer| layer.layer_id)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .max(1);
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
                q0rg.layers.insert(
                    (target_index + 1).min(q0rg.layers.len()),
                    Layer {
                        layer_id: next_id,
                        timeline_frame_count: None,
                        name: format!("Mask {next_id}"),
                        explicit_keyframes: vec![0],
                        placements: Vec::new(),
                    },
                );
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, next_id),
                    LayerMetadata {
                        kind: LayerKind::Mask,
                        parent_folder_id,
                        mask_layer_id: None,
                        collapsed: false,
                        hidden: false,
                        locked: false,
                    },
                );
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, target_layer_id),
                    LayerMetadata {
                        mask_layer_id: Some(next_id),
                        ..target_metadata
                    },
                );
                self.session.current_layer_id = next_id;
                self.state.mark_dirty();
                self.session.status = "mask layer added".to_string();
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
                    self.state.mark_dirty();
                    self.session.status = format!("renamed layer to {name}");
                }
            }
            Action::MoveLayer(q0rg_id, layer_id, direction) => {
                self.move_layer_block(q0rg_id, layer_id, direction);
            }
            Action::DropLayer(q0rg_id, layer_id, target) => {
                self.drop_layer(q0rg_id, layer_id, target);
            }
            Action::DropLayers(q0rg_id, layer_ids, target) => {
                self.drop_layer_group(q0rg_id, &layer_ids, target);
            }
            Action::MoveTimelineFrames(selection, target_layer_id, target_frame) => {
                if self.timeline_selection_touches_locked_layer(selection)
                    || self
                        .state
                        .project
                        .layer_is_locked(self.session.current_q0rg_id, target_layer_id)
                {
                    self.session.status =
                        "locked layers cannot be moved or overwritten".to_string();
                    return;
                }
                self.history.snapshot(&self.state.project);
                if let Some(moved) = crate::timeline_edit::move_frames(
                    &mut self.state.project,
                    self.session.current_q0rg_id,
                    selection,
                    target_layer_id,
                    target_frame,
                ) {
                    self.session.timeline_selection = Some(moved);
                    self.session.timeline_layer_selection = None;
                    self.session.current_layer_id = moved.anchor_layer_id;
                    self.session.current_frame = moved.anchor_frame;
                    self.session.pending_timeline_frame = None;
                    self.state.mark_dirty();
                    self.session.status = "moved frames".to_string();
                } else {
                    self.session.status = "could not move frames".to_string();
                }
            }
            Action::MoveRigTimelineKey(selection, target_frame) => {
                if selection.q0rg_id != self.session.current_q0rg_id
                    || self
                        .state
                        .project
                        .layer_is_locked(selection.q0rg_id, selection.layer_id)
                {
                    self.session.status =
                        "locked or inactive rig tracks cannot be moved".to_string();
                    return;
                }
                let before = self.state.project.clone();
                if crate::timeline_edit::move_rig_key_for_layer(
                    &mut self.state.project,
                    selection.q0rg_id,
                    selection.layer_id,
                    selection.frame,
                    target_frame,
                ) {
                    self.history.snapshot(&before);
                    self.state.mark_dirty();
                    self.session.timeline_rig_key_selection = Some(TimelineRigKeySelection {
                        frame: target_frame,
                        ..selection
                    });
                    self.session.current_layer_id = selection.layer_id;
                    self.session.current_frame = target_frame;
                    self.session.pending_timeline_frame = None;
                    self.session.status =
                        format!("moved rig key to frame {}", u32::from(target_frame) + 1);
                } else {
                    self.session.status =
                        "could not move rig key; destination already has a key".to_string();
                }
            }
            Action::MoveCameraFrames(selection, target_frame) => {
                self.move_camera_frames(selection, target_frame);
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
                if !matches!(metadata.kind, LayerKind::Folder | LayerKind::Mask) {
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
                self.state.mark_dirty();
                self.session.status = if metadata.collapsed {
                    if metadata.kind == LayerKind::Mask {
                        "masked layers shown"
                    } else {
                        "folder expanded"
                    }
                } else {
                    if metadata.kind == LayerKind::Mask {
                        "masked layers hidden"
                    } else {
                        "folder collapsed"
                    }
                }
                .to_string();
            }
            Action::ToggleLayerVisibility(q0rg_id, layer_id) => {
                let exists = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id));
                if !exists {
                    return;
                }
                let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
                self.history.snapshot(&self.state.project);
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, layer_id),
                    LayerMetadata {
                        hidden: !metadata.hidden,
                        ..metadata
                    },
                );
                self.session.selection = Selection::None;
                self.state.mark_dirty();
                self.session.status = if metadata.kind == LayerKind::Mask {
                    if metadata.hidden {
                        "masked layers shown"
                    } else {
                        "masked layers hidden"
                    }
                } else if metadata.hidden {
                    "layer shown"
                } else {
                    "layer hidden"
                }
                .to_string();
            }
            Action::ToggleLayerLock(q0rg_id, layer_id) => {
                let exists = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id));
                if !exists {
                    return;
                }
                let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
                self.history.snapshot(&self.state.project);
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, layer_id),
                    LayerMetadata {
                        locked: !metadata.locked,
                        ..metadata
                    },
                );
                self.session.selection = Selection::None;
                self.state.mark_dirty();
                self.session.status = if metadata.locked {
                    "layer unlocked"
                } else {
                    "layer locked"
                }
                .to_string();
            }
            Action::ToggleLayerOutline(q0rg_id, layer_id) => {
                let exists = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id));
                if !exists {
                    return;
                }
                let key = (q0rg_id, layer_id);
                let enabled = if self.session.outline_layers.remove(&key) {
                    false
                } else {
                    self.session.outline_layers.insert(key);
                    true
                };
                self.session.status = if enabled {
                    "layer outline enabled"
                } else {
                    "layer outline disabled"
                }
                .to_string();
            }
            Action::ToggleMaskArtworkVisibility(q0rg_id, layer_id) => {
                let is_mask = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id))
                    && self.state.project.layer_metadata(q0rg_id, layer_id).kind == LayerKind::Mask;
                if !is_mask {
                    return;
                }
                let key = (self.active_project_tab_id().unwrap_or(0), q0rg_id, layer_id);
                let hidden = if self.session.hidden_mask_art_layers.remove(&key) {
                    false
                } else {
                    self.session.hidden_mask_art_layers.insert(key);
                    true
                };
                self.session.status = if hidden {
                    "mask preview enabled; source artwork hidden"
                } else {
                    "mask editing enabled; source and contents shown"
                }
                .to_string();
            }
            Action::ToggleAllLayerVisibility(q0rg_id) => {
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let hide = q0rg.layers.iter().any(|layer| {
                    !self.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                        && self.state.project.layer_is_visible(q0rg_id, layer.layer_id)
                });
                self.history.snapshot(&self.state.project);
                for layer in &q0rg.layers {
                    let key = LayerKey::new(q0rg_id, layer.layer_id);
                    let mut metadata = self.state.project.layer_metadata(q0rg_id, layer.layer_id);
                    metadata.hidden = hide;
                    self.state.project.layer_metadata.insert(key, metadata);
                }
                self.session.selection = Selection::None;
                self.state.mark_dirty();
                self.session.status = if hide {
                    "all layers hidden"
                } else {
                    "all layers shown"
                }
                .to_string();
            }
            Action::ToggleAllLayerOutlines(q0rg_id) => {
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let layer_ids: Vec<_> = q0rg
                    .layers
                    .iter()
                    .filter(|layer| !self.state.project.layer_is_folder(q0rg_id, layer.layer_id))
                    .map(|layer| layer.layer_id)
                    .collect();
                if layer_ids.is_empty() {
                    return;
                }
                let enable = layer_ids
                    .iter()
                    .any(|id| !self.session.outline_layers.contains(&(q0rg_id, *id)));
                self.session.outline_layers.retain(|(id, _)| *id != q0rg_id);
                if enable {
                    self.session
                        .outline_layers
                        .extend(layer_ids.into_iter().map(|id| (q0rg_id, id)));
                }
                self.session.status = if enable {
                    "all layer outlines enabled"
                } else {
                    "all layer outlines disabled"
                }
                .to_string();
            }
            Action::ToggleAllLayerLocks(q0rg_id) => {
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let any_unlocked = q0rg.layers.iter().any(|layer| {
                    !self.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                        && !self.state.project.layer_is_locked(q0rg_id, layer.layer_id)
                });
                let lock = any_unlocked;
                self.history.snapshot(&self.state.project);
                for layer in &q0rg.layers {
                    let key = LayerKey::new(q0rg_id, layer.layer_id);
                    let mut metadata = self.state.project.layer_metadata(q0rg_id, layer.layer_id);
                    metadata.locked = lock;
                    self.state.project.layer_metadata.insert(key, metadata);
                }
                self.session.selection = Selection::None;
                self.state.mark_dirty();
                self.session.status = if lock {
                    "all layers locked"
                } else {
                    "all layers unlocked"
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
                let symbol_name =
                    unique_symbol_name(&self.state.project, &format!("Symbol {next_id}"), None);
                self.state.project.q0rgs.push(Q0rg {
                    q0rg_id: next_id,
                    name: symbol_name,
                    frame_count: 24,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        timeline_frame_count: None,
                        name: "Layer 1".to_string(),
                        explicit_keyframes: vec![0],
                        placements: Vec::new(),
                    }],
                });
                self.session.selection = Selection::Q0rg(next_id);
                self.state.mark_dirty();
                self.session.status = "Symbol created".to_string();
            }
            Action::AddScene => {
                let Some(q0rg_id) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .map(|q0rg| q0rg.q0rg_id)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                else {
                    self.session.status = "scene id limit reached".to_string();
                    return;
                };
                let scenes = self.state.project.scenes();
                let Some(scene_id) = scenes
                    .iter()
                    .map(|scene| scene.scene_id)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                else {
                    self.session.status = "scene id limit reached".to_string();
                    return;
                };
                let scene_number = scenes.len() + 1;
                let name =
                    unique_symbol_name(&self.state.project, &format!("Scene {scene_number}"), None);
                self.history.snapshot(&self.state.project);
                if self.state.project.runtime.scenes.is_empty() {
                    self.state.project.runtime.scenes = self.state.project.scenes();
                }
                self.state.project.q0rgs.push(Q0rg {
                    q0rg_id,
                    name: name.clone(),
                    frame_count: 24,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        timeline_frame_count: None,
                        name: "Layer 1".to_string(),
                        explicit_keyframes: vec![0],
                        placements: Vec::new(),
                    }],
                });
                self.state.project.runtime.scenes.push(ProjectScene {
                    scene_id,
                    q0rg_id,
                    name: name.clone(),
                });
                self.session.current_q0rg_id = q0rg_id;
                self.session.current_layer_id = 1;
                self.session.current_frame = 0;
                self.session.pending_timeline_frame = None;
                self.session.timeline_selection = None;
                self.session.timeline_cell_selection = None;
                self.session.timeline_layer_selection = None;
                self.session.camera_track_selected = false;
                self.session.camera_timeline_selection = None;
                self.session.camera_frame_drag = None;
                self.session.timeline_frame_drag = None;
                self.session.timeline_rig_key_selection = None;
                self.session.timeline_rig_key_drag = None;
                self.session.timeline_layer_drag = None;
                self.session.selection = Selection::None;
                self.session.breadcrumb.clear();
                self.session.edit_in_place.clear();
                self.session.playing = false;
                self.session.frame_script_editor = None;
                self.session.reset_q0lang_preview();
                self.session.scene_rename = Some(SceneRename {
                    scene_id,
                    draft: name,
                    focus_requested: true,
                });
                self.state.mark_dirty();
                self.session.status = "scene created".to_string();
            }
            Action::SwitchScene(scene_id) => {
                let Some(scene) = self
                    .state
                    .project
                    .scenes()
                    .into_iter()
                    .find(|scene| scene.scene_id == scene_id)
                else {
                    self.session.status = "scene no longer exists".to_string();
                    return;
                };
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == scene.q0rg_id)
                else {
                    self.session.status = "scene timeline no longer exists".to_string();
                    return;
                };
                if self.session.current_q0rg_id == scene.q0rg_id
                    && self.session.breadcrumb.is_empty()
                {
                    return;
                }
                let layer_id = q0rg
                    .layers
                    .iter()
                    .find(|layer| {
                        !self
                            .state
                            .project
                            .layer_is_folder(q0rg.q0rg_id, layer.layer_id)
                    })
                    .or_else(|| q0rg.layers.first())
                    .map(|layer| layer.layer_id)
                    .unwrap_or(1);
                self.session.current_q0rg_id = scene.q0rg_id;
                self.session.current_layer_id = layer_id;
                self.session.current_frame = 0;
                self.session.pending_timeline_frame = None;
                self.session.timeline_selection = None;
                self.session.timeline_cell_selection = None;
                self.session.timeline_layer_selection = None;
                self.session.camera_track_selected = false;
                self.session.camera_timeline_selection = None;
                self.session.camera_frame_drag = None;
                self.session.timeline_frame_drag = None;
                self.session.timeline_rig_key_selection = None;
                self.session.timeline_rig_key_drag = None;
                self.session.timeline_layer_drag = None;
                self.session.selection = Selection::None;
                self.session.breadcrumb.clear();
                self.session.edit_in_place.clear();
                self.session.playing = false;
                self.session.scene_rename = None;
                if self
                    .session
                    .frame_script_editor
                    .as_ref()
                    .is_some_and(|editor| editor.q0rg_id != scene.q0rg_id)
                {
                    self.session.frame_script_editor = None;
                }
                self.session.reset_q0lang_preview();
                self.session.status = format!("scene: {}", scene.name);
            }
            Action::RenameScene(scene_id, requested_name) => {
                let name = requested_name.trim();
                if name.is_empty() {
                    self.session.status = "scene name cannot be empty".to_string();
                    return;
                }
                if name.len() > usize::from(u16::MAX) {
                    self.session.status = "scene name is too long".to_string();
                    return;
                }
                if name.chars().any(char::is_control) {
                    self.session.status = "scene name contains invalid characters".to_string();
                    return;
                }
                let Some(scene) = self
                    .state
                    .project
                    .scenes()
                    .into_iter()
                    .find(|scene| scene.scene_id == scene_id)
                else {
                    self.session.status = "scene no longer exists".to_string();
                    return;
                };
                let name = unique_symbol_name(&self.state.project, name, Some(scene.q0rg_id));
                if scene.name == name {
                    self.session.scene_rename = None;
                    return;
                }
                self.history.snapshot(&self.state.project);
                if self.state.project.runtime.scenes.is_empty() {
                    self.state.project.runtime.scenes = self.state.project.scenes();
                }
                if let Some(scene) = self
                    .state
                    .project
                    .runtime
                    .scenes
                    .iter_mut()
                    .find(|scene| scene.scene_id == scene_id)
                {
                    scene.name = name.clone();
                    if let Some(q0rg) = self
                        .state
                        .project
                        .q0rgs
                        .iter_mut()
                        .find(|q0rg| q0rg.q0rg_id == scene.q0rg_id)
                    {
                        q0rg.name = name.clone();
                    }
                }
                self.session.scene_rename = None;
                self.state.mark_dirty();
                self.session.status = format!("renamed scene to {name}");
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
                            .map(|q0rg| q0rg.name.clone())
                        else {
                            self.session.status = "symbol no longer exists".to_string();
                            return;
                        };
                        let name = unique_symbol_name(&self.state.project, &name, Some(q0rg_id));
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
                        if let Some(scene) = self
                            .state
                            .project
                            .runtime
                            .scenes
                            .iter_mut()
                            .find(|scene| scene.q0rg_id == q0rg_id)
                        {
                            scene.name = name.clone();
                        }
                        self.state.mark_dirty();
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
                        self.state.mark_dirty();
                        self.session.status = format!("renamed vector to {name}");
                    }
                }
            }
            Action::DeleteLibraryItem(item) => {
                self.request_library_item_delete(item);
            }
            Action::DeleteLibraryItems(items) => {
                let mut unique_items = Vec::new();
                for item in items {
                    if !unique_items.contains(&item) {
                        unique_items.push(item);
                    }
                }
                if unique_items.is_empty() {
                    return;
                }
                if unique_items.iter().any(|item| match item {
                    LibraryItem::Q0rg(id) => {
                        *id == self.state.project.meta.entry_q0rg_id
                            || self.q0rg_reference_count(*id) > 0
                    }
                    LibraryItem::Asset(id) => self.asset_reference_count(*id) > 0,
                }) {
                    self.session.status =
                        "remove selected items one at a time when they are used by the project"
                            .to_string();
                    return;
                }
                for item in unique_items.iter().copied() {
                    self.request_library_item_delete(item);
                }
                self.library_multi_selection.clear();
                self.library_selection = None;
                self.library_selection_anchor = None;
            }
            Action::CreateLibraryFolder(path) => {
                let path = path.trim().replace('\\', "/");
                if !valid_library_folder_path(&path) {
                    self.session.status = "folder name must not be empty".to_string();
                    return;
                }
                if self.state.project.runtime.library_folders.contains(&path) {
                    self.session.status = "library folder already exists".to_string();
                    return;
                }
                self.history.snapshot(&self.state.project);
                let mut current = String::new();
                for segment in path.split('/') {
                    if !current.is_empty() {
                        current.push('/');
                    }
                    current.push_str(segment);
                    if !self
                        .state
                        .project
                        .runtime
                        .library_folders
                        .contains(&current)
                    {
                        self.state
                            .project
                            .runtime
                            .library_folders
                            .push(current.clone());
                    }
                }
                self.state.project.runtime.library_folders.sort();
                self.state.mark_dirty();
                self.session.status = format!("created library folder {path}");
            }
            Action::RenameLibraryFolder(old_path, new_path) => {
                let old_path = old_path.trim_matches('/').to_string();
                let new_path = new_path.trim().replace('\\', "/");
                if !valid_library_folder_path(&new_path)
                    || !self
                        .state
                        .project
                        .runtime
                        .library_folders
                        .contains(&old_path)
                {
                    self.session.status = "invalid library folder name".to_string();
                    return;
                }
                let affected = self
                    .state
                    .project
                    .runtime
                    .library_folders
                    .iter()
                    .filter(|path| **path == old_path || path.starts_with(&format!("{old_path}/")))
                    .cloned()
                    .collect::<Vec<_>>();
                if affected.iter().any(|path| {
                    let suffix = &path[old_path.len()..];
                    let renamed = format!("{new_path}{suffix}");
                    self.state
                        .project
                        .runtime
                        .library_folders
                        .iter()
                        .any(|existing| !affected.contains(existing) && existing == &renamed)
                }) || new_path == old_path
                    || new_path.starts_with(&format!("{old_path}/"))
                {
                    self.session.status = "a folder with that name already exists".to_string();
                    return;
                }
                self.history.snapshot(&self.state.project);
                for path in &mut self.state.project.runtime.library_folders {
                    if path == &old_path || path.starts_with(&format!("{old_path}/")) {
                        path.replace_range(..old_path.len(), &new_path);
                    }
                }
                for path in self.state.project.runtime.library_item_folders.values_mut() {
                    if path == &old_path || path.starts_with(&format!("{old_path}/")) {
                        path.replace_range(..old_path.len(), &new_path);
                    }
                }
                self.state.project.runtime.library_folders.sort();
                self.state.mark_dirty();
                self.session.status = format!("renamed folder to {new_path}");
            }
            Action::DeleteLibraryFolder(path) => {
                if !self.state.project.runtime.library_folders.contains(&path) {
                    return;
                }
                let parent = path.rsplit_once('/').map(|(parent, _)| parent.to_string());
                self.history.snapshot(&self.state.project);
                if let Some(parent) = &parent {
                    for folder in self.state.project.runtime.library_item_folders.values_mut() {
                        if *folder == path || folder.starts_with(&format!("{path}/")) {
                            *folder = parent.clone();
                        }
                    }
                } else {
                    self.state
                        .project
                        .runtime
                        .library_item_folders
                        .retain(|_, folder| {
                            folder != &path && !folder.starts_with(&format!("{path}/"))
                        });
                }
                self.state
                    .project
                    .runtime
                    .library_folders
                    .retain(|folder| folder != &path && !folder.starts_with(&format!("{path}/")));
                self.state.mark_dirty();
                self.session.status = format!("deleted folder {path}");
            }
            Action::MoveLibraryItemToFolder(item, folder) => {
                if folder.as_ref().is_some_and(|folder| {
                    !self.state.project.runtime.library_folders.contains(folder)
                }) {
                    self.session.status = "library folder no longer exists".to_string();
                    return;
                }
                let reference = match item {
                    LibraryItem::Q0rg(id) => q0s_format::v2::LibraryItemReference::Q0rg(id),
                    LibraryItem::Asset(id) => q0s_format::v2::LibraryItemReference::Asset(id),
                };
                if self
                    .state
                    .project
                    .runtime
                    .library_item_folders
                    .get(&reference)
                    == folder.as_ref()
                {
                    return;
                }
                self.history.snapshot(&self.state.project);
                if let Some(folder) = folder {
                    self.state
                        .project
                        .runtime
                        .library_item_folders
                        .insert(reference, folder);
                } else {
                    self.state
                        .project
                        .runtime
                        .library_item_folders
                        .remove(&reference);
                }
                self.state.mark_dirty();
                self.session.status = "moved library item".to_string();
            }
            Action::SetQ0rgFrameCount(q0rg_id, frame_count) => {
                self.request_frame_count_change(q0rg_id, frame_count);
            }
            Action::DeleteSelection => {
                if let Some(selection) =
                    self.session.timeline_rig_key_selection.filter(|selection| {
                        selection.q0rg_id == self.session.current_q0rg_id
                            && matches!(&self.session.selection, Selection::None)
                    })
                {
                    if self
                        .state
                        .project
                        .layer_is_locked(selection.q0rg_id, selection.layer_id)
                    {
                        self.session.status =
                            "rig keys on locked layers cannot be deleted".to_string();
                        return;
                    }
                    let before = self.state.project.clone();
                    if crate::timeline_edit::delete_rig_key_for_layer(
                        &mut self.state.project,
                        selection.q0rg_id,
                        selection.layer_id,
                        selection.frame,
                    ) {
                        self.history.snapshot(&before);
                        self.state.mark_dirty();
                        self.session.status = format!(
                            "deleted rig key at frame {}",
                            u32::from(selection.frame) + 1
                        );
                    } else {
                        self.session.status = "no rig key at the selected frame".to_string();
                    }
                    return;
                }
                if self.camera_track_is_selected() {
                    self.delete_camera_keyframe_at_current();
                    return;
                }
                if self.session.pending_timeline_frame.is_some()
                    && (self.session.timeline_selection.is_some()
                        || self.session.timeline_cell_selection.is_some())
                {
                    self.session.status =
                        "future frame is not created; press F5, F6, or F7 first".to_string();
                    return;
                }
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
            Action::RemoveSelectedTimelineFrames => {
                self.remove_selected_timeline_frames();
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
            Action::PlaceLibraryDragAt(payload, position) => {
                self.place_library_drag_at(payload, position);
            }
            Action::PlaceLibraryDragOnTimeline(payload, layer_id, frame) => {
                self.place_library_drag_on_timeline(payload, layer_id, frame);
            }
            Action::EnterQ0rg(child_id) => {
                self.enter_q0rg(child_id);
            }
            Action::EnterQ0rgInPlace {
                q0rg_id,
                layer_id,
                placement_idx,
            } => {
                self.enter_q0rg_in_place(q0rg_id, layer_id, placement_idx);
                self.session.symbol_edit_click = None;
            }
            Action::BreadcrumbJumpTo(depth) => {
                self.breadcrumb_jump_to(depth);
            }
            Action::ExitQ0rg => {
                self.session.symbol_edit_click = None;
                if !self.session.breadcrumb.is_empty() {
                    let parent = self.session.breadcrumb.pop().unwrap();
                    let edit_context = self.session.edit_in_place.pop();
                    self.session.current_q0rg_id = parent;
                    self.refresh_layer_for_current_q0rg();
                    self.session.current_frame = edit_context
                        .map(|context| context.parent_frame)
                        .unwrap_or(0);
                    self.session.pending_timeline_frame = None;
                    self.session.timeline_selection = None;
                    self.session.timeline_cell_selection = None;
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
            Action::ToggleProjectTree => {
                self.show_project_tree = !self.show_project_tree;
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
                if self.session.pending_timeline_frame.is_some()
                    && self.session.timeline_selection.is_some()
                {
                    self.session.status =
                        "future frame is not created; press F5, F6, or F7 first".to_string();
                    return;
                }
                let payload = if let Some((q0rg_id, cells)) = self
                    .session
                    .timeline_cell_selection
                    .as_ref()
                    .filter(|(q0rg_id, cells)| {
                        *q0rg_id == self.session.current_q0rg_id && !cells.is_empty()
                    }) {
                    crate::timeline_edit::capture_frame_cells(&self.state.project, *q0rg_id, cells)
                        .map(|frames| crate::state::ClipboardPayload {
                            timeline: Some(TimelineClipboard::Frames(frames)),
                            ..Default::default()
                        })
                } else if let Some(selection) = self.session.timeline_selection {
                    crate::timeline_edit::capture_frames(
                        &self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    )
                    .map(|frames| crate::state::ClipboardPayload {
                        timeline: Some(TimelineClipboard::Frames(frames)),
                        ..Default::default()
                    })
                } else if let Some(selection) = self.session.timeline_layer_selection {
                    crate::timeline_edit::capture_layers(
                        &self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    )
                    .map(|layers| crate::state::ClipboardPayload {
                        timeline: Some(TimelineClipboard::Layers(layers)),
                        ..Default::default()
                    })
                } else {
                    Some(crate::selection_edit::capture_clipboard(
                        &self.state.project,
                        &self.session.selection,
                    ))
                };
                if payload.as_ref().is_none_or(|payload| payload.is_empty()) {
                    self.session.status = "nothing to copy".to_string();
                } else if let Some(payload) = payload {
                    self.session.status = self
                        .store_editor_clipboard(payload)
                        .map(|error| format!("copied; cross-project paste unavailable: {error}"))
                        .unwrap_or_else(|| "copied".to_string());
                }
            }
            Action::CutSelection => {
                if self.session.pending_timeline_frame.is_some()
                    && (self.session.timeline_selection.is_some()
                        || self.session.timeline_cell_selection.is_some())
                {
                    self.session.status =
                        "future frame is not created; press F5, F6, or F7 first".to_string();
                    return;
                }
                if let Some((q0rg_id, cells)) =
                    self.session
                        .timeline_cell_selection
                        .clone()
                        .filter(|(q0rg_id, cells)| {
                            *q0rg_id == self.session.current_q0rg_id && !cells.is_empty()
                        })
                {
                    let Some(frames) = crate::timeline_edit::capture_frame_cells(
                        &self.state.project,
                        q0rg_id,
                        &cells,
                    ) else {
                        self.session.status = "nothing to cut".to_string();
                        return;
                    };
                    if self.delete_timeline_selection().is_some() {
                        let _ = self.store_editor_clipboard(crate::state::ClipboardPayload {
                            timeline: Some(TimelineClipboard::Frames(frames)),
                            ..Default::default()
                        });
                        self.session.status = "cut selected frames".to_string();
                    }
                    return;
                }
                if let Some(selection) = self.session.timeline_selection {
                    if self.timeline_selection_touches_locked_layer(selection) {
                        self.session.status = "locked timeline frames cannot be cut".to_string();
                        return;
                    }
                    let Some(frames) = crate::timeline_edit::capture_frames(
                        &self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    ) else {
                        self.session.status = "nothing to cut".to_string();
                        return;
                    };
                    self.history.snapshot(&self.state.project);
                    crate::timeline_edit::clear_frames(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    );
                    let _ = self.store_editor_clipboard(crate::state::ClipboardPayload {
                        timeline: Some(TimelineClipboard::Frames(frames)),
                        ..Default::default()
                    });
                    self.state.mark_dirty();
                    self.session.status = "cut frames".to_string();
                } else if let Some(selection) = self.session.timeline_layer_selection {
                    let Some(layers) = crate::timeline_edit::capture_layers(
                        &self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    ) else {
                        self.session.status = "nothing to cut".to_string();
                        return;
                    };
                    self.history.snapshot(&self.state.project);
                    crate::timeline_edit::remove_layers(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    );
                    let _ = self.store_editor_clipboard(crate::state::ClipboardPayload {
                        timeline: Some(TimelineClipboard::Layers(layers)),
                        ..Default::default()
                    });
                    self.session.timeline_layer_selection = None;
                    self.session.reconcile_with(&self.state.project);
                    self.state.mark_dirty();
                    self.session.status = "cut layers".to_string();
                } else {
                    let payload = crate::selection_edit::capture_clipboard(
                        &self.state.project,
                        &self.session.selection,
                    );
                    if payload.is_empty() {
                        self.session.status = "nothing to cut".to_string();
                    } else {
                        let clipboard_error = self.store_editor_clipboard(payload);
                        if self.delete_selection() {
                            self.session.status = clipboard_error
                                .map(|error| {
                                    format!("cut; cross-project paste unavailable: {error}")
                                })
                                .unwrap_or_else(|| "cut".to_string());
                        }
                    }
                }
            }
            paste_action @ (Action::Paste | Action::PasteInPlace) => {
                let paste_in_place = matches!(paste_action, Action::PasteInPlace);
                let Some(clipboard) = self.current_editor_clipboard() else {
                    self.session.status = "clipboard empty".to_string();
                    return;
                };
                let mut payload = clipboard.payload.clone();
                let cross_document = clipboard.source_document_tab != self.active_document_tab;
                if cross_document && payload.timeline.is_some() {
                    self.session.status =
                        "timeline clipboard can only be pasted into its source project".to_string();
                    return;
                }
                match payload.timeline.clone() {
                    Some(TimelineClipboard::Frames(frames)) => {
                        if self.current_layer_is_locked() || !self.current_layer_is_visible() {
                            self.session.status =
                                "show and unlock the target layer before pasting frames"
                                    .to_string();
                            return;
                        }
                        if self.current_layer_is_folder() {
                            self.session.status = "paste frames onto a drawable layer".to_string();
                            return;
                        }
                        let target_frame = self.session.timeline_frame();
                        let sparse_clipboard = frames
                            .rows
                            .iter()
                            .any(|row| row.selected_frames.len() != usize::from(frames.width));
                        let before = self.state.project.clone();
                        let mut candidate = before.clone();
                        if let Some(selection) = crate::timeline_edit::paste_frames(
                            &mut candidate,
                            self.session.current_q0rg_id,
                            self.session.current_layer_id,
                            target_frame,
                            &frames,
                        ) {
                            let audio_conflict = candidate.audio_clips.iter().enumerate().any(
                                |(clip_idx, clip)| {
                                    crate::audio::audio_clip_ref(&candidate, clip_idx).is_some_and(
                                        |resolved| {
                                            crate::audio::visual_content_conflicts_with_audio_range(
                                                &candidate,
                                                clip.q0rg_id,
                                                clip.layer_id,
                                                resolved.start_frame,
                                                resolved.end_frame_exclusive,
                                            )
                                        },
                                    )
                                },
                            );
                            if audio_conflict {
                                self.session.status =
                                    "cannot paste visual frames into an audio range".to_string();
                                return;
                            }
                            let pasted_cells = if sparse_clipboard {
                                crate::timeline_edit::pasted_frame_cells(
                                    &candidate,
                                    self.session.current_q0rg_id,
                                    self.session.current_layer_id,
                                    target_frame,
                                    &frames,
                                )
                            } else {
                                Vec::new()
                            };
                            self.history.snapshot(&before);
                            self.state.project = candidate;
                            if sparse_clipboard && !pasted_cells.is_empty() {
                                self.session.timeline_selection = None;
                                self.session.timeline_cell_selection =
                                    Some((self.session.current_q0rg_id, pasted_cells));
                            } else {
                                self.session.timeline_selection = Some(selection);
                                self.session.timeline_cell_selection = None;
                            }
                            self.session.timeline_layer_selection = None;
                            self.session.selection = Selection::None;
                            self.session.current_layer_id = selection.anchor_layer_id;
                            self.session.current_frame = selection.anchor_frame;
                            self.session.pending_timeline_frame = None;
                            self.state.mark_dirty();
                            self.session.status = "pasted frames".to_string();
                        } else {
                            self.session.status = "could not paste frames".to_string();
                        }
                    }
                    Some(TimelineClipboard::Layers(layers)) => {
                        self.history.snapshot(&self.state.project);
                        if let Some(selection) = crate::timeline_edit::paste_layers(
                            &mut self.state.project,
                            self.session.current_q0rg_id,
                            self.session.current_layer_id,
                            &layers,
                        ) {
                            self.session.timeline_layer_selection = Some(selection);
                            self.session.timeline_selection = None;
                            self.session.selection = Selection::None;
                            self.session.current_layer_id = selection.anchor_layer_id;
                            self.session.pending_timeline_frame = None;
                            self.state.mark_dirty();
                            self.session.status = "pasted layers".to_string();
                        } else {
                            self.session.status = "could not paste layers".to_string();
                        }
                    }
                    None => {
                        if self.current_layer_is_locked() || !self.current_layer_is_visible() {
                            self.session.status =
                                "show and unlock the target layer before pasting artwork"
                                    .to_string();
                            return;
                        }
                        if self.session.pending_timeline_frame.is_some() {
                            self.session.status =
                                "create the future frame with F5, F6, or F7 before pasting artwork"
                                    .to_string();
                            return;
                        }
                        if self.current_layer_is_folder() {
                            self.session.status = "folders cannot contain artwork".to_string();
                            return;
                        }
                        if crate::audio::audio_clip_at_frame(
                            &self.state.project,
                            self.session.current_q0rg_id,
                            self.session.current_layer_id,
                            self.session.current_frame,
                        )
                        .is_some()
                        {
                            self.session.status = "audio frames cannot contain artwork".to_string();
                            return;
                        }
                        if payload.is_empty() {
                            self.session.status = "clipboard empty".to_string();
                            return;
                        }
                        let offset = if paste_in_place {
                            q0s_format::v2::Vec2::new(0.0, 0.0)
                        } else {
                            let target = self.session.stage_cursor.unwrap_or_else(|| {
                                q0s_format::v2::Vec2::new(
                                    self.state.project.meta.stage_width as f32 * 0.5,
                                    self.state.project.meta.stage_height as f32 * 0.5,
                                )
                            });
                            crate::selection_edit::paste_offset_for_target(&payload, target)
                        };
                        let result = if cross_document {
                            let mut candidate = self.state.project.clone();
                            if !payload.placements.is_empty() {
                                let Some(source_project) = clipboard.source_project.as_deref()
                                else {
                                    self.session.status =
                                        clipboard.source_project_error.unwrap_or_else(|| {
                                            "cross-project artwork is unavailable in this clipboard"
                                                .to_string()
                                        });
                                    return;
                                };
                                let mut imported_targets =
                                    std::collections::BTreeMap::<(u8, u16), Target>::new();
                                for placement in &mut payload.placements {
                                    let (key, item) = match placement.target {
                                        Target::Asset(id) => ((0, id), LibraryItem::Asset(id)),
                                        Target::Q0rg(id) => ((1, id), LibraryItem::Q0rg(id)),
                                    };
                                    let target = if let Some(target) = imported_targets.get(&key) {
                                        *target
                                    } else {
                                        let (updated, root) =
                                            match crate::library_transfer::import_library_item(
                                                source_project,
                                                &candidate,
                                                item,
                                            ) {
                                                Ok(imported) => imported,
                                                Err(error) => {
                                                    self.session.status = format!(
                                                        "cross-project paste failed: {error}"
                                                    );
                                                    return;
                                                }
                                            };
                                        candidate = updated;
                                        let target = match root {
                                            LibraryItem::Asset(id) => Target::Asset(id),
                                            LibraryItem::Q0rg(id) => Target::Q0rg(id),
                                        };
                                        imported_targets.insert(key, target);
                                        target
                                    };
                                    placement.target = target;
                                }
                            }
                            let result = crate::selection_edit::paste_payload_with_depth(
                                &mut candidate,
                                self.session.current_q0rg_id,
                                self.session.current_layer_id,
                                self.session.current_frame,
                                &payload,
                                offset,
                                &clipboard.placement_depths,
                            );
                            self.history.snapshot(&self.state.project);
                            self.state.project = candidate;
                            result
                        } else {
                            self.history.snapshot(&self.state.project);
                            crate::selection_edit::paste_payload_with_depth(
                                &mut self.state.project,
                                self.session.current_q0rg_id,
                                self.session.current_layer_id,
                                self.session.current_frame,
                                &payload,
                                offset,
                                &clipboard.placement_depths,
                            )
                        };
                        self.session.selection =
                            crate::selection_edit::selection_from_paste(result);
                        self.state.mark_dirty();
                        self.textures.invalidate();
                        self.session.status = if paste_in_place {
                            "pasted in place".to_string()
                        } else {
                            "pasted at cursor".to_string()
                        };
                    }
                }
            }
            Action::DuplicateSelection => {
                if self.session.pending_timeline_frame.is_some()
                    && self.session.timeline_selection.is_some()
                {
                    self.session.status =
                        "future frame is not created; press F5, F6, or F7 first".to_string();
                    return;
                }
                if let Some(selection) = self.session.timeline_selection {
                    if self.timeline_selection_touches_locked_layer(selection) {
                        self.session.status =
                            "locked timeline frames cannot be duplicated".to_string();
                        return;
                    }
                    let before = self.state.project.clone();
                    if let Some(duplicated) = crate::timeline_edit::duplicate_frames(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    ) {
                        self.history.snapshot(&before);
                        self.session.timeline_selection = Some(duplicated);
                        self.session.timeline_layer_selection = None;
                        self.session.selection = Selection::None;
                        self.session.current_layer_id = duplicated.anchor_layer_id;
                        self.session.current_frame = duplicated.anchor_frame;
                        self.state.mark_dirty();
                        self.textures.invalidate();
                        self.session.status = "duplicated frames".to_string();
                    } else {
                        self.session.status = "could not duplicate frames".to_string();
                    }
                } else if let Some(selection) = self.session.timeline_layer_selection {
                    let Some(layers) = crate::timeline_edit::capture_layers(
                        &self.state.project,
                        self.session.current_q0rg_id,
                        selection,
                    ) else {
                        self.session.status = "nothing to duplicate".to_string();
                        return;
                    };
                    let before = self.state.project.clone();
                    if let Some(duplicated) = crate::timeline_edit::paste_layers(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        self.session.current_layer_id,
                        &layers,
                    ) {
                        self.history.snapshot(&before);
                        self.session.timeline_layer_selection = Some(duplicated);
                        self.session.timeline_selection = None;
                        self.session.selection = Selection::None;
                        self.session.current_layer_id = duplicated.anchor_layer_id;
                        self.state.mark_dirty();
                        self.textures.invalidate();
                        self.session.status = "duplicated layers".to_string();
                    } else {
                        self.session.status = "could not duplicate layers".to_string();
                    }
                } else {
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
                        self.session.selection =
                            crate::selection_edit::selection_from_paste(result);
                        self.state.mark_dirty();
                        self.textures.invalidate();
                        self.session.status = "duplicated".to_string();
                    }
                }
            }
            Action::DuplicateSymbol(q0rg_id, layer_id, placement_idx) => {
                self.duplicate_symbol_instance(q0rg_id, layer_id, placement_idx);
            }
            Action::AltDragDuplicateSelection(start_cursor) => {
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
                    return;
                }
                let before = self.state.project.clone();
                let result = crate::selection_edit::paste_payload(
                    &mut self.state.project,
                    self.session.current_q0rg_id,
                    self.session.current_layer_id,
                    self.session.current_frame,
                    &payload,
                    q0s_format::v2::Vec2::new(10.0, 10.0),
                );
                self.history.snapshot(&before);
                self.session.selection = crate::selection_edit::selection_from_paste(result);
                self.state.mark_dirty();
                self.textures.invalidate();
                if !crate::tools::begin_alt_duplicate_drag(self, start_cursor) {
                    self.session.status = "duplicated".to_string();
                }
            }
            Action::BringToFront => {
                self.reorder_selection(true, false);
            }
            Action::SendToBack => {
                self.reorder_selection(false, false);
            }
            Action::BringForward => self.reorder_selection(true, true),
            Action::SendBackward => self.reorder_selection(false, true),
            Action::SpreadBetweenLayers => self.spread_selected_objects_between_layers(),
            Action::SpreadBetweenKeyframes => self.spread_selected_objects_between_keyframes(),
            Action::FlipHorizontal => {
                crate::tools::flip_selected_placements(self, true);
            }
            Action::FlipVertical => {
                crate::tools::flip_selected_placements(self, false);
            }
            Action::RemoveFrame => self.remove_frame(),
            Action::ClearKeyframe => {
                if self.camera_track_is_selected() {
                    self.delete_camera_keyframe_at_current();
                } else {
                    self.clear_keyframe();
                }
            }
            Action::CreateMotionTween => {
                if self.camera_track_is_selected() {
                    self.toggle_camera_tween();
                } else {
                    self.toggle_motion_tween();
                }
            }
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
            Action::SetFrameLabel(q0rg_id, frame, label) => {
                let label = label.map(|name| name.trim().to_owned());
                if !self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .any(|q| q.q0rg_id == q0rg_id && frame < q.frame_count)
                {
                    self.session.status = "frame label destination no longer exists".to_string();
                    return;
                }
                if let Some(name) = &label {
                    if let Err(error) = q0s_format::v2::validate_frame_label_name(name) {
                        self.session.status = error.to_string();
                        return;
                    }
                    if self
                        .state
                        .project
                        .runtime
                        .resolve_frame_label(q0rg_id, name)
                        .is_some_and(|existing| existing != frame)
                    {
                        self.session.status =
                            format!("frame label `{name}` already exists in this symbol");
                        return;
                    }
                }
                let key = q0s_format::v2::FrameLabelKey::new(q0rg_id, frame);
                if self.state.project.runtime.frame_labels.get(&key) == label.as_ref() {
                    return;
                }
                self.history.snapshot(&self.state.project);
                if let Some(label) = label {
                    self.state.project.runtime.frame_labels.insert(key, label);
                    self.session.status = format!("frame {} label saved", u32::from(frame) + 1);
                } else {
                    self.state.project.runtime.frame_labels.remove(&key);
                    self.session.status = "frame label removed".to_string();
                }
                self.state.mark_dirty();
                self.session.reset_q0lang_preview();
            }
            Action::OpenFrameScriptEditor(q0rg_id, layer_id, frame) => {
                if self.state.project.layer_is_folder(q0rg_id, layer_id) {
                    self.session.status = "folders cannot contain frame code".to_string();
                } else if self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .is_some_and(|q0rg| frame < q0rg.frame_count)
                {
                    let source = self
                        .state
                        .project
                        .runtime
                        .frame_scripts
                        .iter()
                        .find(|script| {
                            script.q0rg_id == q0rg_id
                                && script.layer_id == layer_id
                                && script.frame == frame
                        })
                        .map(|script| script.source.clone())
                        .unwrap_or_default();
                    self.session.frame_script_editor = Some(FrameScriptEditor {
                        q0rg_id,
                        layer_id,
                        frame,
                        source,
                    });
                    self.session.status = format!("frame code: frame {}", frame + 1);
                }
            }
            Action::DeleteFrameScript(q0rg_id, layer_id, frame) => {
                if self
                    .state
                    .project
                    .runtime
                    .frame_scripts
                    .iter()
                    .any(|script| {
                        script.q0rg_id == q0rg_id
                            && script.layer_id == layer_id
                            && script.frame == frame
                    })
                {
                    self.history.snapshot(&self.state.project);
                    self.state.project.runtime.frame_scripts.retain(|script| {
                        script.q0rg_id != q0rg_id
                            || script.layer_id != layer_id
                            || script.frame != frame
                    });
                    self.state.mark_dirty();
                    self.session.reset_q0lang_preview();
                    if self
                        .session
                        .frame_script_editor
                        .as_ref()
                        .is_some_and(|editor| {
                            editor.q0rg_id == q0rg_id
                                && editor.layer_id == layer_id
                                && editor.frame == frame
                        })
                    {
                        self.session.frame_script_editor = None;
                    }
                    self.session.status = "frame code removed".to_string();
                } else {
                    self.session.status = "frame has no code".to_string();
                }
            }
            Action::AddProjectDependency(path, parent) => {
                self.add_project_dependency(&path, parent);
            }
            Action::RenameProjectDependency(node_id, alias) => {
                self.rename_project_dependency(node_id, alias);
            }
            Action::ReparentProjectDependency(node_id, parent) => {
                self.reparent_project_dependency(node_id, parent);
            }
            Action::RemoveProjectDependency(node_id) => {
                self.remove_project_dependency(node_id);
            }
            Action::RelinkProjectDependency(node_id, path) => {
                self.relink_project_dependency(node_id, &path);
            }
            Action::EmbedProjectDependency(node_id) => {
                self.embed_project_dependency(node_id);
            }
            Action::OpenProjectDependency(node_id) => {
                self.open_project_dependency(ctx, node_id);
            }
        }
    }

    fn spread_selected_objects_between_layers(&mut self) {
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return;
        }
        let references = crate::selection_edit::selected_object_refs(
            &self.state.project,
            &self.session.selection,
        );
        if references.is_empty() {
            self.session.status = "select objects on one layer first".to_string();
            return;
        }
        let first = references[0];
        if references.iter().any(|reference| {
            reference.q0rg_id != first.q0rg_id || reference.layer_id != first.layer_id
        }) {
            self.session.status = "spread requires objects from one layer".to_string();
            return;
        }

        let q0rg_id = first.q0rg_id;
        let layer_id = first.layer_id;
        let frame = self.session.current_frame;
        let Some((
            source_index,
            source,
            metadata,
            depth_count,
            selected_depths,
            selected_names,
            single_object,
        )) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                let source_index = q0rg
                    .layers
                    .iter()
                    .position(|layer| layer.layer_id == layer_id)?;
                let source = &q0rg.layers[source_index];
                let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
                if metadata.kind != LayerKind::Normal {
                    return None;
                }
                let active = q0s_format::raster::active_placement_states_at(source, frame);
                let selected_indices: std::collections::HashSet<usize> = references
                    .iter()
                    .map(|reference| reference.placement_idx)
                    .collect();
                let active_indices: std::collections::HashSet<usize> =
                    active.iter().map(|placement| placement.index).collect();
                if selected_indices.len() != references.len()
                    || !selected_indices.is_subset(&active_indices)
                {
                    return None;
                }
                let selected_names = active
                    .iter()
                    .filter(|placement| selected_indices.contains(&placement.index))
                    .map(|placement| {
                        let placement = source.placements.get(placement.index)?;
                        Some(match placement.target {
                            Target::Q0rg(q0rg_id) => self
                                .state
                                .project
                                .q0rgs
                                .iter()
                                .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
                                .name
                                .clone(),
                            Target::Asset(_) => source.name.clone(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                let mut counts = std::collections::HashMap::<u16, usize>::new();
                let mut depth_count = 0usize;
                for placement in &source.placements {
                    let count = counts.entry(placement.frame).or_default();
                    *count += 1;
                    depth_count = depth_count.max(*count);
                }
                let selected_depths = active
                    .iter()
                    .enumerate()
                    .filter(|(_, placement)| selected_indices.contains(&placement.index))
                    .map(|(depth, placement)| (depth, placement.index))
                    .collect::<Vec<_>>();
                Some((
                    source_index,
                    source,
                    metadata,
                    depth_count,
                    selected_depths,
                    selected_names,
                    active.len() == 1,
                ))
            })
        else {
            self.session.status =
                "select objects currently visible on one normal unlocked layer".to_string();
            return;
        };

        if single_object {
            let name = selected_names[0].clone();
            if source.name == name {
                self.session.status = "object is already on its own named layer".to_string();
                return;
            }
            self.history.snapshot(&self.state.project);
            self.state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter_mut()
                        .find(|layer| layer.layer_id == layer_id)
                })
                .expect("spread source layer exists")
                .name = name;
            self.state.mark_dirty();
            self.session.status = "named layer after its object".to_string();
            return;
        }

        // Selected depths each get their own track. Keep contiguous unselected
        // depths together, splitting a remainder only where needed so that a
        // selected middle object never jumps above/below neighbouring artwork.
        let selected_ranks = selected_depths
            .iter()
            .map(|(depth, _)| *depth)
            .collect::<std::collections::BTreeSet<_>>();
        let mut groups = Vec::<Vec<usize>>::new();
        for depth in 0..depth_count {
            if !selected_ranks.contains(&depth)
                && groups
                    .last()
                    .is_some_and(|group| !selected_ranks.contains(&group[0]))
            {
                groups
                    .last_mut()
                    .expect("remainder group exists")
                    .push(depth);
            } else {
                groups.push(vec![depth]);
            }
        }
        let source_group = groups
            .iter()
            .position(|group| !selected_ranks.contains(&group[0]))
            .unwrap_or(0);
        let mut group_by_depth = vec![0usize; depth_count];
        for (group_index, group) in groups.iter().enumerate() {
            for depth in group {
                group_by_depth[*depth] = group_index;
            }
        }

        let max_layer_id = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().map(|layer| layer.layer_id).max())
            .unwrap_or(0);
        if u16::try_from(groups.len() - 1)
            .ok()
            .and_then(|extra| max_layer_id.checked_add(extra))
            .is_none()
        {
            self.session.status =
                "not enough layer ids available to spread these objects".to_string();
            return;
        }

        let mut placements_by_depth = vec![Vec::<Placement>::new(); groups.len()];
        let mut depth_by_frame = std::collections::HashMap::<u16, usize>::new();
        for placement in &source.placements {
            let depth = depth_by_frame.entry(placement.frame).or_default();
            placements_by_depth[group_by_depth[*depth]].push(placement.clone());
            *depth += 1;
        }
        let source_keyframes = source.keyframe_frames();
        let base_name = source.name.clone();
        let names_by_depth = selected_depths
            .iter()
            .zip(selected_names)
            .map(|((depth, _), name)| (*depth, name))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut new_layers = Vec::with_capacity(groups.len());
        let mut new_ids = Vec::with_capacity(groups.len());
        let mut allocated = 0u16;
        let mut used_names = std::collections::HashSet::new();
        for (depth, placements) in placements_by_depth.into_iter().enumerate() {
            let new_id = if depth == source_group {
                layer_id
            } else {
                allocated += 1;
                max_layer_id + allocated
            };
            new_ids.push(new_id);
            let base_layer_name = names_by_depth
                .get(&groups[depth][0])
                .cloned()
                .unwrap_or_else(|| base_name.clone());
            let mut layer_name = base_layer_name.clone();
            let mut suffix = 2usize;
            while !used_names.insert(layer_name.to_lowercase()) {
                layer_name = format!("{base_layer_name} {suffix}");
                suffix += 1;
            }
            let mut explicit_keyframes = source.explicit_keyframes.clone();
            explicit_keyframes.extend(
                source_keyframes
                    .iter()
                    .copied()
                    .filter(|frame| !placements.iter().any(|placement| placement.frame == *frame)),
            );
            explicit_keyframes.sort_unstable();
            explicit_keyframes.dedup();
            new_layers.push(Layer {
                layer_id: new_id,
                timeline_frame_count: source.timeline_frame_count,
                name: layer_name,
                explicit_keyframes,
                placements,
            });
        }

        let selected_after_spread: Vec<PlacementRef> = selected_depths
            .iter()
            .filter_map(|(depth, source_placement_idx)| {
                let source_placement = source.placements.get(*source_placement_idx)?;
                let group = group_by_depth[*depth];
                let placement_idx = new_layers
                    .get(group)?
                    .placements
                    .iter()
                    .position(|placement| placement.frame == source_placement.frame)?;
                Some(PlacementRef {
                    q0rg_id,
                    layer_id: *new_ids.get(group)?,
                    placement_idx,
                })
            })
            .collect();
        if selected_after_spread.len() != selected_depths.len() {
            self.session.status = "could not map the selected object depths".to_string();
            return;
        }

        let before = self.state.project.clone();
        self.history.snapshot(&before);
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        q0rg.layers.splice(source_index..=source_index, new_layers);
        for new_id in new_ids.iter().copied().filter(|new_id| *new_id != layer_id) {
            self.state
                .project
                .layer_metadata
                .insert(LayerKey::new(q0rg_id, new_id), metadata);
        }
        self.session.current_layer_id = selected_after_spread[0].layer_id;
        self.session.timeline_selection = Some(TimelineSelection::single(
            self.session.current_layer_id,
            frame,
        ));
        self.session.timeline_cell_selection = None;
        self.session.selection = if selected_after_spread.len() == 1 {
            let reference = selected_after_spread[0];
            Selection::Placement {
                q0rg_id,
                layer_id: reference.layer_id,
                placement_idx: reference.placement_idx,
            }
        } else {
            Selection::Multi(selected_after_spread)
        };
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = format!("spread {} depth levels between layers", new_ids.len());
    }

    fn spread_selected_objects_between_keyframes(&mut self) {
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return;
        }
        let references = crate::selection_edit::selected_object_refs(
            &self.state.project,
            &self.session.selection,
        );
        if references.is_empty() {
            self.session.status = "select objects on one layer first".to_string();
            return;
        }
        let first = references[0];
        if references.iter().any(|reference| {
            reference.q0rg_id != first.q0rg_id || reference.layer_id != first.layer_id
        }) {
            self.session.status = "spread requires objects from one layer".to_string();
            return;
        }

        let q0rg_id = first.q0rg_id;
        let layer_id = first.layer_id;
        let frame = self.session.current_frame;
        let Some((mut placements, background, layer_frame_count, frame_count, has_keyframe)) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                let layer = q0rg
                    .layers
                    .iter()
                    .find(|layer| layer.layer_id == layer_id)?;
                if self.state.project.layer_metadata(q0rg_id, layer_id).kind == LayerKind::Folder {
                    return None;
                }
                let active = q0s_format::raster::active_placement_states_at(layer, frame);
                let selected_indices = references
                    .iter()
                    .map(|reference| reference.placement_idx)
                    .collect::<std::collections::HashSet<_>>();
                let active_indices = active
                    .iter()
                    .map(|placement| placement.index)
                    .collect::<std::collections::HashSet<_>>();
                if selected_indices.len() != references.len()
                    || !selected_indices.is_subset(&active_indices)
                {
                    return None;
                }
                let mut placements = Vec::new();
                let mut background = Vec::new();
                for (depth, active) in active.into_iter().enumerate() {
                    let source = layer.placements.get(active.index)?;
                    let placement = Placement {
                        instance_id: source.instance_id,
                        frame,
                        target: source.target,
                        transform: active.transform,
                        tween: Tween::None,
                        fx: active.fx,
                    };
                    if selected_indices.contains(&active.index) {
                        placements.push((depth, placement));
                    } else {
                        background.push((depth, placement));
                    }
                }
                Some((
                    placements,
                    background,
                    layer.timeline_frame_count.unwrap_or(q0rg.frame_count),
                    q0rg.frame_count,
                    layer.has_keyframe(frame),
                ))
            })
        else {
            self.session.status =
                "select objects currently visible on one normal unlocked layer".to_string();
            return;
        };

        if placements.len() == 1 && has_keyframe {
            self.session.status = "selected object is already on this keyframe".to_string();
            return;
        }

        let extra_frames = match u16::try_from(placements.len().saturating_sub(1)) {
            Ok(extra) => extra,
            Err(_) => {
                self.session.status = "too many selected objects to spread across keyframes".into();
                return;
            }
        };
        let Some(new_layer_frame_count) = layer_frame_count.checked_add(extra_frames) else {
            self.session.status = "not enough timeline space to create keyframes".into();
            return;
        };
        let new_frame_count = frame_count.max(new_layer_frame_count);
        let Some(last_frame) = frame.checked_add(extra_frames) else {
            self.session.status = "not enough timeline space to create keyframes".into();
            return;
        };
        if last_frame >= new_frame_count {
            self.session.status = "not enough timeline space to create keyframes".into();
            return;
        }
        if crate::audio::audio_clip_at_frame(&self.state.project, q0rg_id, layer_id, frame)
            .is_some()
        {
            self.session.status = "audio clips cannot share a layer with visual keyframes".into();
            return;
        }

        let before = self.state.project.clone();
        let mut updated = before.clone();
        if let Some(q0rg) = updated
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        {
            if let Some(layer) = q0rg
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
            {
                for source in &mut layer.placements {
                    if source.frame < frame
                        && source
                            .tween
                            .to_frame()
                            .is_some_and(|to_frame| to_frame >= frame)
                    {
                        source.tween = source.tween.with_to_frame(frame);
                    }
                }
            }
        }
        // Make consecutive cells on this row. The row-aware F5 operation
        // preserves every other layer's duration even at the shared tail.
        for _ in 0..extra_frames {
            if crate::timeline_edit::insert_hold_frames_after_selected_cells(
                &mut updated,
                q0rg_id,
                &[(layer_id, frame)],
            )
            .is_none()
            {
                self.session.status = "could not make room for the new keyframes".into();
                return;
            }
        }

        let Some(layer) = updated
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        else {
            return;
        };
        layer
            .placements
            .retain(|placement| placement.frame != frame);
        let mut first_placement_idx = layer.placements.len();
        for (offset, (selected_depth, placement)) in placements.drain(..).enumerate() {
            let keyframe = frame + offset as u16;
            layer.ensure_explicit_keyframe(keyframe);
            // Unselected objects remain on every generated key, in their
            // original depth order relative to that key's selected object.
            let mut contents = background.clone();
            contents.push((selected_depth, placement));
            contents.sort_by_key(|(depth, _)| *depth);
            for (depth, mut placement) in contents {
                placement.frame = keyframe;
                if offset == 0 && depth == selected_depth {
                    first_placement_idx = layer.placements.len();
                }
                layer.push_placement(placement);
            }
        }

        self.history.snapshot(&before);
        self.state.project = updated;
        self.session.current_layer_id = layer_id;
        self.session.current_frame = frame;
        self.session.timeline_selection = Some(TimelineSelection::single(layer_id, frame));
        self.session.timeline_cell_selection = None;
        self.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx: first_placement_idx,
        };
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = format!(
            "spread {} objects across keyframes {}–{}",
            extra_frames + 1,
            frame + 1,
            last_frame + 1
        );
    }

    fn reorder_selection(&mut self, to_front: bool, one_step: bool) {
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return;
        }
        if !matches!(
            self.session.selection,
            Selection::Placement { .. } | Selection::Multi(_)
        ) {
            self.session.status = "select objects to change their drawing order".to_string();
            return;
        }
        let selection_before = self.session.selection.clone();
        let refs =
            crate::selection_edit::selected_object_refs(&self.state.project, &selection_before);
        let frame = self.session.current_frame;
        let make_orders = |project: &ProjectV2, refs: &[PlacementRef]| {
            let mut orders = Vec::new();
            for q0rg in &project.q0rgs {
                for layer in &q0rg.layers {
                    let selected: std::collections::HashSet<_> = refs
                        .iter()
                        .filter(|r| r.q0rg_id == q0rg.q0rg_id && r.layer_id == layer.layer_id)
                        .map(|r| r.placement_idx)
                        .collect();
                    if selected.is_empty() {
                        continue;
                    }
                    let slots: Vec<_> = crate::render::active_placements_at(layer, frame)
                        .into_iter()
                        .map(|(index, _)| index)
                        .collect();
                    let mut order = slots.clone();
                    if one_step {
                        if to_front {
                            for index in (0..order.len().saturating_sub(1)).rev() {
                                if selected.contains(&order[index])
                                    && !selected.contains(&order[index + 1])
                                {
                                    order.swap(index, index + 1);
                                }
                            }
                        } else {
                            for index in 1..order.len() {
                                if selected.contains(&order[index])
                                    && !selected.contains(&order[index - 1])
                                {
                                    order.swap(index, index - 1);
                                }
                            }
                        }
                    } else {
                        order.sort_by_key(|index| selected.contains(index) == to_front);
                    }
                    if order != slots {
                        orders.push((q0rg.q0rg_id, layer.layer_id, slots, order));
                    }
                }
            }
            orders
        };
        if make_orders(&self.state.project, &refs).is_empty() {
            self.session.status =
                "objects are already at the edge of the drawing order".to_string();
            return;
        }
        self.history.snapshot(&self.state.project);
        let Some(mut refs) = crate::tools::materialize_placement_refs_for_edit(
            &mut self.state.project,
            &refs,
            frame,
        ) else {
            self.session.status = "could not change drawing order".to_string();
            return;
        };
        let orders = make_orders(&self.state.project, &refs);
        for (q0rg_id, layer_id, slots, order) in orders {
            let layer = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == q0rg_id)
                .unwrap()
                .layers
                .iter_mut()
                .find(|l| l.layer_id == layer_id)
                .unwrap();
            let placements: Vec<_> = order
                .iter()
                .map(|index| layer.placements[*index].clone())
                .collect();
            for reference in refs
                .iter_mut()
                .filter(|r| r.q0rg_id == q0rg_id && r.layer_id == layer_id)
            {
                if let Some(position) = order
                    .iter()
                    .position(|index| *index == reference.placement_idx)
                {
                    reference.placement_idx = slots[position];
                }
            }
            for (slot, placement) in slots.into_iter().zip(placements) {
                layer.placements[slot] = placement;
            }
        }
        let refs = crate::selection_edit::selected_object_refs(
            &self.state.project,
            &Selection::Multi(refs),
        );
        self.session.selection = if refs.len() == 1 {
            Selection::Placement {
                q0rg_id: refs[0].q0rg_id,
                layer_id: refs[0].layer_id,
                placement_idx: refs[0].placement_idx,
            }
        } else {
            Selection::Multi(refs)
        };
        rebind_transform_pivot(
            &mut self.session.transform_pivot,
            &selection_before,
            &self.session.selection,
        );
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = match (to_front, one_step) {
            (true, true) => "brought forward one step",
            (false, true) => "sent backward one step",
            (true, false) => "brought to front",
            (false, false) => "sent to back",
        }
        .to_string();
    }

    fn current_layer_is_folder(&self) -> bool {
        self.state
            .project
            .layer_is_folder(self.session.current_q0rg_id, self.session.current_layer_id)
    }

    fn current_layer_is_locked(&self) -> bool {
        self.state
            .project
            .layer_is_locked(self.session.current_q0rg_id, self.session.current_layer_id)
    }

    fn current_layer_is_visible(&self) -> bool {
        self.state
            .project
            .layer_is_visible(self.session.current_q0rg_id, self.session.current_layer_id)
    }

    fn timeline_selection_touches_locked_layer(&self, selection: TimelineSelection) -> bool {
        let q0rg_id = self.session.current_q0rg_id;
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
        else {
            return false;
        };
        let Some(anchor) = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.anchor_layer_id)
        else {
            return false;
        };
        let Some(focus) = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.focus_layer_id)
        else {
            return false;
        };
        let first = anchor.min(focus);
        let last = anchor.max(focus);
        q0rg.layers[first..=last].iter().any(|layer| {
            !self.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                && self.state.project.layer_is_locked(q0rg_id, layer.layer_id)
        })
    }

    fn timeline_selection_touches_audio(&self, selection: TimelineSelection) -> bool {
        let q0rg_id = self.session.current_q0rg_id;
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return false;
        };
        let Some(anchor_layer) = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.anchor_layer_id)
        else {
            return false;
        };
        let Some(focus_layer) = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.focus_layer_id)
        else {
            return false;
        };
        let first_layer = anchor_layer.min(focus_layer);
        let last_layer = anchor_layer.max(focus_layer);
        let first_frame = selection.anchor_frame.min(selection.focus_frame);
        let end_frame = selection
            .anchor_frame
            .max(selection.focus_frame)
            .saturating_add(1);
        self.state.project.audio_clips.iter().any(|clip| {
            if clip.q0rg_id != q0rg_id {
                return false;
            }
            let Some(layer_index) = q0rg
                .layers
                .iter()
                .position(|layer| layer.layer_id == clip.layer_id)
            else {
                return false;
            };
            if !(first_layer..=last_layer).contains(&layer_index) {
                return false;
            }
            q0s_format::v2::audio_clip_end_frame(&self.state.project, *clip)
                .is_some_and(|clip_end| first_frame < clip_end && clip.start_frame < end_frame)
        })
    }

    pub(crate) fn selection_touches_locked_layer(&self) -> bool {
        let locked =
            |q0rg_id: u16, layer_id: u16| self.state.project.layer_is_locked(q0rg_id, layer_id);
        match &self.session.selection {
            Selection::Placement {
                q0rg_id, layer_id, ..
            }
            | Selection::Path {
                q0rg_id, layer_id, ..
            } => locked(*q0rg_id, *layer_id),
            Selection::Paths(paths) => paths.iter().any(|path| locked(path.q0rg_id, path.layer_id)),
            Selection::StrokeSegment(segment) => {
                locked(segment.path.q0rg_id, segment.path.layer_id)
            }
            Selection::RawParts {
                fills,
                strokes,
                stroke_segments,
                objects,
            } => fills
                .iter()
                .chain(strokes)
                .map(|path| (path.q0rg_id, path.layer_id))
                .chain(
                    stroke_segments
                        .iter()
                        .map(|segment| (segment.path.q0rg_id, segment.path.layer_id)),
                )
                .chain(objects.iter().map(|item| (item.q0rg_id, item.layer_id)))
                .any(|(q0rg_id, layer_id)| locked(q0rg_id, layer_id)),
            Selection::PathPoints { path, .. } | Selection::SubselectPoints { path, .. } => {
                locked(path.q0rg_id, path.layer_id)
            }
            Selection::RawArea {
                placements,
                objects,
                ..
            } => placements
                .iter()
                .chain(objects)
                .any(|item| locked(item.q0rg_id, item.layer_id)),
            Selection::Mixed { paths, objects } => paths
                .iter()
                .map(|path| (path.q0rg_id, path.layer_id))
                .chain(objects.iter().map(|item| (item.q0rg_id, item.layer_id)))
                .any(|(q0rg_id, layer_id)| locked(q0rg_id, layer_id)),
            Selection::Multi(items) => items.iter().any(|item| locked(item.q0rg_id, item.layer_id)),
            Selection::None | Selection::Asset(_) | Selection::Q0rg(_) => false,
        }
    }

    fn current_q0rg_has_locked_artwork_layer(&self) -> bool {
        let q0rg_id = self.session.current_q0rg_id;
        self.state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .is_some_and(|q0rg| {
                q0rg.layers.iter().any(|layer| {
                    !self.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                        && self.state.project.layer_is_locked(q0rg_id, layer.layer_id)
                })
            })
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
        self.state
            .project
            .runtime
            .library_item_folders
            .remove(&q0s_format::v2::LibraryItemReference::Q0rg(q0rg_id));
        self.state
            .project
            .runtime
            .scenes
            .retain(|scene| scene.q0rg_id != q0rg_id);
        self.state
            .project
            .runtime
            .frame_labels
            .retain(|key, _| key.q0rg_id != q0rg_id);
        self.state
            .project
            .audio_clips
            .retain(|clip| clip.q0rg_id != q0rg_id);
        self.state
            .project
            .runtime
            .frame_scripts
            .retain(|script| script.q0rg_id != q0rg_id);
        self.state
            .project
            .runtime
            .instance_names
            .retain(|key, _| key.q0rg_id != q0rg_id);
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
        self.state.mark_dirty();
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
        let removed_layer_ids: std::collections::HashSet<u16> = q0rg.layers[start..end]
            .iter()
            .map(|layer| layer.layer_id)
            .collect();
        let item_count = q0rg.layers[start..end]
            .iter()
            .map(|layer| layer.placements.len() + layer.explicit_keyframes.len())
            .sum::<usize>()
            + self
                .state
                .project
                .audio_clips
                .iter()
                .filter(|clip| {
                    clip.q0rg_id == q0rg_id && removed_layer_ids.contains(&clip.layer_id)
                })
                .count()
            + self
                .state
                .project
                .runtime
                .frame_scripts
                .iter()
                .filter(|script| {
                    script.q0rg_id == q0rg_id && removed_layer_ids.contains(&script.layer_id)
                })
                .count();
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
        let removed_instance_ids = self.state.project.q0rgs[q0rg_index].layers[start..end]
            .iter()
            .flat_map(|layer| &layer.placements)
            .filter_map(|placement| (placement.instance_id != 0).then_some(placement.instance_id))
            .collect::<std::collections::HashSet<_>>();
        self.state.project.q0rgs[q0rg_index]
            .layers
            .drain(start..end);
        self.state
            .project
            .audio_clips
            .retain(|clip| clip.q0rg_id != q0rg_id || !removed_ids.contains(&clip.layer_id));
        self.state
            .project
            .runtime
            .frame_scripts
            .retain(|script| script.q0rg_id != q0rg_id || !removed_ids.contains(&script.layer_id));
        self.state.project.runtime.instance_names.retain(|key, _| {
            key.q0rg_id != q0rg_id || !removed_instance_ids.contains(&key.instance_id)
        });
        self.state
            .project
            .layer_metadata
            .retain(|key, _| key.q0rg_id != q0rg_id || !removed_ids.contains(&key.layer_id));
        for (key, metadata) in &mut self.state.project.layer_metadata {
            if key.q0rg_id == q0rg_id
                && metadata
                    .mask_layer_id
                    .is_some_and(|mask_id| removed_ids.contains(&mask_id))
            {
                metadata.mask_layer_id = None;
            }
        }

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
                timeline_frame_count: Some(1),
                name: format!("Layer {next_id}"),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            });
        }
        self.session.layer_rename = None;
        crate::timeline_edit::sync_folder_timelines(&mut self.state.project, q0rg_id);
        self.session.reconcile_with(&self.state.project);
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = match metadata.kind {
            LayerKind::Folder => "deleted layer folder",
            LayerKind::Mask => "deleted mask layer",
            LayerKind::Normal => "deleted layer",
        }
        .to_string();
    }

    /// Return the model-order range occupied by a timeline row. Folder rows own
    /// their complete descendant subtree, stored contiguously before the folder.
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
            && self.state.project.layer_is_descendant_of(
                q0rg_id,
                q0rg.layers[start - 1].layer_id,
                layer_id,
            )
        {
            start -= 1;
        }
        Some((start, index + 1))
    }

    fn apply_layer_tree(&mut self, q0rg_id: u16, nodes: &[LayerTreeNode]) -> bool {
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return false;
        };
        let current_layers = self.state.project.q0rgs[q0rg_index].layers.clone();
        let mut model_ids = Vec::with_capacity(current_layers.len());
        let mut parent_by_child = std::collections::HashMap::new();
        flatten_layer_tree_model_order(nodes, None, &mut model_ids, &mut parent_by_child);
        if model_ids.len() != current_layers.len() {
            return false;
        }

        let current_ids = current_layers
            .iter()
            .map(|layer| layer.layer_id)
            .collect::<Vec<_>>();
        let parent_changed = current_layers.iter().any(|layer| {
            self.state
                .project
                .layer_parent_folder(q0rg_id, layer.layer_id)
                != parent_by_child.get(&layer.layer_id).copied()
        });
        if current_ids == model_ids && !parent_changed {
            return false;
        }

        let mut layers_by_id = current_layers
            .into_iter()
            .map(|layer| (layer.layer_id, layer))
            .collect::<std::collections::HashMap<_, _>>();
        let mut reordered = Vec::with_capacity(model_ids.len());
        for id in model_ids {
            let Some(layer) = layers_by_id.remove(&id) else {
                return false;
            };
            reordered.push(layer);
        }
        self.state.project.q0rgs[q0rg_index].layers = reordered;

        let layer_ids = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .map(|layer| layer.layer_id)
            .collect::<Vec<_>>();
        for layer_id in layer_ids {
            let key = LayerKey::new(q0rg_id, layer_id);
            let mut metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
            metadata.parent_folder_id = parent_by_child.get(&layer_id).copied();
            if metadata == LayerMetadata::default() {
                self.state.project.layer_metadata.remove(&key);
            } else {
                self.state.project.layer_metadata.insert(key, metadata);
            }
        }
        true
    }

    fn move_layer_block(&mut self, q0rg_id: u16, layer_id: u16, direction: i8) {
        if direction == 0 {
            return;
        }
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let mut nodes = build_layer_tree(&self.state.project, q0rg_id, &q0rg.layers);
        if !move_layer_tree_sibling(&mut nodes, layer_id, direction) {
            return;
        }
        self.history.snapshot(&self.state.project);
        if !self.apply_layer_tree(q0rg_id, &nodes) {
            return;
        }
        self.state.mark_dirty();
        self.session.status = if direction < 0 {
            "layer moved up"
        } else {
            "layer moved down"
        }
        .to_string();
    }

    fn drop_layer(&mut self, q0rg_id: u16, layer_id: u16, target: LayerDropTarget) {
        self.drop_layer_group(q0rg_id, &[layer_id], target);
    }

    fn drop_layer_group(&mut self, q0rg_id: u16, layer_ids: &[u16], target: LayerDropTarget) {
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let mut selected = layer_ids
            .iter()
            .copied()
            .filter(|layer_id| q0rg.layers.iter().any(|layer| layer.layer_id == *layer_id))
            .collect::<Vec<_>>();
        selected.dedup();
        if selected.is_empty() {
            return;
        }

        // If a folder and one of its visible children are both in the range,
        // move the folder once; its child already travels with it.
        let roots = selected
            .iter()
            .copied()
            .filter(|layer_id| {
                !selected.iter().any(|ancestor| {
                    ancestor != layer_id
                        && self
                            .state
                            .project
                            .layer_is_descendant_of(q0rg_id, *layer_id, *ancestor)
                })
            })
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return;
        }

        let direct_mask_update = match target {
            LayerDropTarget::IntoMask(mask_id) => Some(Some(mask_id)),
            LayerDropTarget::OutOfMask => Some(None),
            _ => None,
        };
        if let Some(mask_layer_id) = direct_mask_update {
            self.assign_layer_mask_group(q0rg_id, &roots, mask_layer_id);
            return;
        }

        let mask_update_target = match target {
            LayerDropTarget::Before {
                layer_id: target_id,
                ..
            }
            | LayerDropTarget::After {
                layer_id: target_id,
                ..
            } if !roots.contains(&target_id) => {
                Some(self.state.project.layer_mask_source(q0rg_id, target_id))
            }
            _ => None,
        };
        let mask_updates = mask_update_target
            .map(|mask_layer_id| {
                roots
                    .iter()
                    .copied()
                    .filter(|layer_id| Some(*layer_id) != mask_layer_id)
                    .map(|layer_id| (layer_id, mask_layer_id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if mask_updates.iter().any(|(layer_id, mask_layer_id)| {
            mask_layer_id.is_some_and(|mask_id| {
                !self.layer_mask_assignment_valid(q0rg_id, *layer_id, mask_id)
            })
        }) {
            self.session.status = "that move would create a circular mask".to_string();
            return;
        }

        let mut nodes = build_layer_tree(&self.state.project, q0rg_id, &q0rg.layers);
        let dragged_nodes = roots
            .iter()
            .filter_map(|layer_id| take_layer_tree_node(&mut nodes, *layer_id))
            .collect::<Vec<_>>();
        if dragged_nodes.is_empty() {
            return;
        }

        let inserted = match target {
            LayerDropTarget::IntoFolder(folder_id) => {
                if dragged_nodes
                    .iter()
                    .any(|node| layer_tree_contains(node, folder_id))
                    || !self.state.project.layer_is_folder(q0rg_id, folder_id)
                {
                    false
                } else if let Some(children) = layer_tree_children_mut(&mut nodes, Some(folder_id))
                {
                    children.splice(0..0, dragged_nodes);
                    true
                } else {
                    false
                }
            }
            LayerDropTarget::IntoMask(_) | LayerDropTarget::OutOfMask => false,
            LayerDropTarget::Before {
                layer_id: target_id,
                parent_folder_id,
            }
            | LayerDropTarget::After {
                layer_id: target_id,
                parent_folder_id,
            } => {
                let Some(container_target_id) = layer_child_at_parent(
                    &self.state.project,
                    q0rg_id,
                    target_id,
                    parent_folder_id,
                ) else {
                    return;
                };
                if dragged_nodes
                    .iter()
                    .any(|node| layer_tree_contains(node, container_target_id))
                {
                    false
                } else {
                    let before = matches!(target, LayerDropTarget::Before { .. });
                    if let Some(container) = layer_tree_children_mut(&mut nodes, parent_folder_id) {
                        if let Some(target_index) = container
                            .iter()
                            .position(|node| node.layer_id == container_target_id)
                        {
                            let insert_at = target_index + usize::from(!before);
                            container.splice(insert_at..insert_at, dragged_nodes);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            }
        };
        if !inserted {
            return;
        }
        if !layer_tree_depth_within_limit(&nodes, 0) {
            self.session.status = "maximum layer folder nesting depth reached".to_string();
            return;
        }

        let mask_changed = mask_updates.iter().any(|(layer_id, mask_layer_id)| {
            self.state
                .project
                .layer_metadata(q0rg_id, *layer_id)
                .mask_layer_id
                != *mask_layer_id
        });
        self.history.snapshot(&self.state.project);
        let tree_changed = self.apply_layer_tree(q0rg_id, &nodes);
        for (layer_id, mask_layer_id) in mask_updates {
            self.write_layer_mask_assignment(q0rg_id, layer_id, mask_layer_id);
        }
        if !tree_changed && !mask_changed {
            return;
        }
        let primary_layer_id = roots[0];
        self.session.current_layer_id = primary_layer_id;
        self.session.timeline_selection = None;
        self.session.timeline_layer_selection =
            Some(TimelineLayerSelection::single(primary_layer_id));
        self.session.timeline_scroll_to_layer = Some((q0rg_id, primary_layer_id));
        self.state.mark_dirty();
        self.session.status = if mask_changed {
            if mask_update_target.flatten().is_some() {
                "layer moved into mask".to_string()
            } else {
                "layer moved out of mask".to_string()
            }
        } else if roots.len() > 1 {
            format!("moved {} layers by drag-and-drop", roots.len())
        } else {
            "layer moved by drag-and-drop".to_string()
        };
    }

    fn layer_mask_assignment_valid(&self, q0rg_id: u16, layer_id: u16, mask_id: u16) -> bool {
        let project = &self.state.project;
        let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
            return false;
        };
        q0rg.layers.iter().any(|layer| layer.layer_id == layer_id)
            && q0rg.layers.iter().any(|layer| layer.layer_id == mask_id)
            && project.layer_metadata(q0rg_id, mask_id).kind == LayerKind::Mask
            && !project.layer_is_descendant_of(q0rg_id, mask_id, layer_id)
            && !project
                .layer_mask_sources(q0rg_id, mask_id)
                .contains(&layer_id)
    }

    fn write_layer_mask_assignment(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        mask_layer_id: Option<u16>,
    ) -> bool {
        let key = LayerKey::new(q0rg_id, layer_id);
        let mut metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        if metadata.mask_layer_id == mask_layer_id {
            return false;
        }
        metadata.mask_layer_id = mask_layer_id;
        if metadata == LayerMetadata::default() {
            self.state.project.layer_metadata.remove(&key);
        } else {
            self.state.project.layer_metadata.insert(key, metadata);
        }
        true
    }

    fn assign_layer_mask_group(
        &mut self,
        q0rg_id: u16,
        layer_ids: &[u16],
        mask_layer_id: Option<u16>,
    ) {
        let mut unique_ids = Vec::with_capacity(layer_ids.len());
        for layer_id in layer_ids {
            if !unique_ids.contains(layer_id) {
                unique_ids.push(*layer_id);
            }
        }
        if unique_ids.is_empty() {
            return;
        }
        if mask_layer_id.is_some_and(|mask_id| {
            unique_ids
                .iter()
                .any(|layer_id| !self.layer_mask_assignment_valid(q0rg_id, *layer_id, mask_id))
        }) {
            self.session.status = "that move would create a circular mask".to_string();
            return;
        }
        let changed = unique_ids.iter().any(|layer_id| {
            self.state
                .project
                .layer_metadata(q0rg_id, *layer_id)
                .mask_layer_id
                != mask_layer_id
        });
        if !changed {
            return;
        }

        self.history.snapshot(&self.state.project);
        for layer_id in &unique_ids {
            self.write_layer_mask_assignment(q0rg_id, *layer_id, mask_layer_id);
        }
        self.session.current_layer_id = unique_ids[0];
        self.session.timeline_selection = None;
        self.session.timeline_layer_selection = Some(TimelineLayerSelection::single(unique_ids[0]));
        self.session.timeline_scroll_to_layer = Some((q0rg_id, unique_ids[0]));
        self.state.mark_dirty();
        self.session.status = if mask_layer_id.is_some() {
            "layer moved into mask"
        } else {
            "layer moved out of mask"
        }
        .to_string();
    }

    fn indent_layer(&mut self, q0rg_id: u16, layer_id: u16) {
        let parent = self.state.project.layer_parent_folder(q0rg_id, layer_id);
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let siblings = q0rg
            .layers
            .iter()
            .rev()
            .filter(|layer| {
                self.state
                    .project
                    .layer_parent_folder(q0rg_id, layer.layer_id)
                    == parent
            })
            .map(|layer| layer.layer_id)
            .collect::<Vec<_>>();
        let Some(index) = siblings.iter().position(|id| *id == layer_id) else {
            return;
        };
        let Some(folder_id) = index
            .checked_sub(1)
            .and_then(|index| siblings.get(index))
            .copied()
        else {
            self.session.status = "put a folder directly above the layer first".to_string();
            return;
        };
        if !self.state.project.layer_is_folder(q0rg_id, folder_id) {
            self.session.status = "put a folder directly above the layer first".to_string();
            return;
        }
        self.drop_layer(q0rg_id, layer_id, LayerDropTarget::IntoFolder(folder_id));
        if self.state.project.layer_parent_folder(q0rg_id, layer_id) == Some(folder_id) {
            self.session.status = "layer moved into folder".to_string();
        }
    }

    fn outdent_layer(&mut self, q0rg_id: u16, layer_id: u16) {
        let Some(folder_id) = self.state.project.layer_parent_folder(q0rg_id, layer_id) else {
            return;
        };
        let grandparent = self.state.project.layer_parent_folder(q0rg_id, folder_id);
        self.drop_layer(
            q0rg_id,
            layer_id,
            LayerDropTarget::After {
                layer_id: folder_id,
                parent_folder_id: grandparent,
            },
        );
        if self.state.project.layer_parent_folder(q0rg_id, layer_id) == grandparent {
            self.session.status = "layer moved out of folder".to_string();
        }
    }

    fn asset_reference_count(&self, asset_id: u16) -> usize {
        let placement_refs = self
            .state
            .project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .filter(|placement| matches!(placement.target, Target::Asset(id) if id == asset_id))
            .count();
        placement_refs
            + self
                .state
                .project
                .audio_clips
                .iter()
                .filter(|clip| clip.asset_id == asset_id)
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
                placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| to_frame >= requested)
            })
            .count();
        let removed_audio = self
            .state
            .project
            .audio_clips
            .iter()
            .filter(|clip| clip.q0rg_id == q0rg_id)
            .filter(|clip| {
                clip.start_frame >= requested
                    || q0s_format::v2::audio_clip_end_frame(&self.state.project, **clip)
                        .is_some_and(|end| end > requested)
            })
            .count();
        let removed_frame_scripts = self
            .state
            .project
            .runtime
            .frame_scripts
            .iter()
            .filter(|script| script.q0rg_id == q0rg_id && script.frame >= requested)
            .count();
        let removed_frame_labels = self
            .state
            .project
            .runtime
            .frame_labels
            .keys()
            .filter(|key| key.q0rg_id == q0rg_id && key.frame >= requested)
            .count();
        let (removed_camera_keys, removed_camera_tweens) =
            if q0rg_id == self.state.project.meta.entry_q0rg_id {
                (
                    self.state
                        .project
                        .runtime
                        .camera
                        .keys
                        .iter()
                        .filter(|key| key.frame >= requested)
                        .count(),
                    self.state
                        .project
                        .runtime
                        .camera
                        .keys
                        .iter()
                        .filter(|key| {
                            key.tween
                                .to_frame()
                                .is_some_and(|to_frame| to_frame >= requested)
                        })
                        .count(),
                )
            } else {
                (0, 0)
            };

        if requested < q0rg.frame_count
            && (removed_keyframes > 0
                || removed_tweens > 0
                || removed_audio > 0
                || removed_frame_scripts > 0
                || removed_frame_labels > 0
                || removed_camera_keys > 0
                || removed_camera_tweens > 0)
        {
            self.pending_frame_truncate = Some((
                q0rg_id,
                requested,
                removed_keyframes,
                removed_tweens,
                removed_audio,
                removed_frame_scripts,
            ));
            self.session.status = "reducing frames requires confirmation".to_string();
            return;
        }
        self.apply_frame_count(q0rg_id, requested);
    }

    fn apply_frame_count(&mut self, q0rg_id: u16, frame_count: u16) {
        self.history.snapshot(&self.state.project);
        let frame_count = frame_count.max(1);
        self.state
            .project
            .runtime
            .frame_labels
            .retain(|key, _| key.q0rg_id != q0rg_id || key.frame < frame_count);
        let audio_to_remove: std::collections::HashSet<(u16, u16, u16, u16)> = self
            .state
            .project
            .audio_clips
            .iter()
            .filter(|clip| clip.q0rg_id == q0rg_id)
            .filter(|clip| {
                clip.start_frame >= frame_count
                    || q0s_format::v2::audio_clip_end_frame(&self.state.project, **clip)
                        .is_some_and(|end| end > frame_count)
            })
            .map(|clip| (clip.q0rg_id, clip.layer_id, clip.start_frame, clip.asset_id))
            .collect();
        self.state.project.audio_clips.retain(|clip| {
            !audio_to_remove.contains(&(
                clip.q0rg_id,
                clip.layer_id,
                clip.start_frame,
                clip.asset_id,
            ))
        });
        self.state
            .project
            .runtime
            .frame_scripts
            .retain(|script| script.q0rg_id != q0rg_id || script.frame < frame_count);
        if q0rg_id == self.state.project.meta.entry_q0rg_id {
            self.state
                .project
                .runtime
                .camera
                .keys
                .retain(|key| key.frame < frame_count);
            for key in &mut self.state.project.runtime.camera.keys {
                if key
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| to_frame >= frame_count)
                {
                    key.tween = Tween::None;
                }
            }
        }
        if self
            .session
            .frame_script_editor
            .as_ref()
            .is_some_and(|editor| editor.q0rg_id == q0rg_id && editor.frame >= frame_count)
        {
            self.session.frame_script_editor = None;
        }
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        q0rg.frame_count = frame_count;
        for layer in &mut q0rg.layers {
            layer
                .explicit_keyframes
                .retain(|frame| *frame < q0rg.frame_count);
            layer
                .placements
                .retain(|placement| placement.frame < q0rg.frame_count);
            for placement in &mut layer.placements {
                if placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| to_frame >= q0rg.frame_count)
                {
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
        let final_frame_count = q0rg.frame_count;
        self.session.selection = Selection::None;
        self.state.mark_dirty();
        self.session.status = format!("symbol length: {} frame(s)", final_frame_count);
    }

    /// Convert the current selection Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› one placement *or* a marquee multi Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС›
    /// into a fresh q0rg symbol. Internal placements keep their original
    /// stage-space transforms so the visual result is byte-identical to
    /// before; the new outer placement uses IDENTITY transform and points
    /// at the new q0rg, so dragging it moves the whole group as one unit.
    fn convert_selection_to_q0rg(&mut self) {
        let selection_before = self.session.selection.clone();
        if !matches!(
            selection_before,
            Selection::Placement { .. }
                | Selection::Multi(_)
                | Selection::Path { .. }
                | Selection::Paths(_)
                | Selection::RawArea { .. }
        ) {
            self.session.status = "Convert to Symbol: select graphics or objects first".to_string();
            return;
        }
        if self.selection_touches_locked_layer() {
            self.session.status = "locked artwork cannot be converted".to_string();
            return;
        }
        let Some(new_q0rg_id) = self
            .state
            .project
            .q0rgs
            .iter()
            .map(|q0rg| q0rg.q0rg_id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.session.status = "no free symbol ids".to_string();
            return;
        };
        let frame = self.session.current_frame;
        let raw_refs = match &self.session.selection {
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => Some(vec![crate::state::PathRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
                path_idx: *path_idx,
            }]),
            Selection::Paths(refs) if !refs.is_empty() => Some(refs.clone()),
            _ => None,
        };
        let area = match &self.session.selection {
            Selection::RawArea {
                placements,
                objects,
                bounds_min,
                bounds_max,
                clip_polygon,
            } => Some((
                placements.clone(),
                objects.clone(),
                *bounds_min,
                *bounds_max,
                clip_polygon.clone(),
            )),
            _ => None,
        };
        let mut refs = if let Some((raw, objects, ..)) = &area {
            raw.iter().chain(objects).copied().collect()
        } else {
            crate::selection_edit::selected_object_refs(
                &self.state.project,
                &self.session.selection,
            )
        };
        if refs.is_empty() && raw_refs.is_none() {
            self.session.status = "Convert to Symbol: select graphics or objects first".to_string();
            return;
        }
        self.history.snapshot(&self.state.project);
        if let Some(raw_refs) = raw_refs {
            let Some(materialized) =
                crate::tools::materialize_raw_paths_as_placements(self, &raw_refs)
            else {
                self.session.status =
                    "Convert to Symbol: raw selection is no longer available".to_string();
                return;
            };
            refs = materialized;
        }
        // Bake the entire affected layer key on a hold before moving anything.
        // The old key and the unselected siblings keep their contents and depth.
        let Some(materialized) = crate::tools::materialize_placement_refs_for_edit(
            &mut self.state.project,
            &refs,
            frame,
        ) else {
            self.session.status = "Convert to Symbol: selection is no longer available".to_string();
            return;
        };
        let area = area.map(|(raw, objects, min, max, clip)| {
            let raw_count = raw.len();
            debug_assert_eq!(materialized.len(), raw_count + objects.len());
            (
                materialized[..raw_count].to_vec(),
                materialized[raw_count..].to_vec(),
                min,
                max,
                clip,
            )
        });
        // Selection order and numeric layer ids are not drawing order.
        let refs = crate::selection_edit::selected_object_refs(
            &self.state.project,
            &Selection::Multi(materialized),
        );
        let Some(anchor) = refs.last().copied() else {
            self.session.status = "Convert to Symbol: selection is empty".to_string();
            return;
        };
        let parent_q0rg_id = anchor.q0rg_id;
        if refs
            .iter()
            .any(|reference| reference.q0rg_id != parent_q0rg_id)
        {
            self.session.status = "Select objects from one symbol before converting".to_string();
            return;
        }
        let parent = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
            .unwrap();
        let anchor_layer = parent
            .layers
            .iter()
            .find(|layer| layer.layer_id == anchor.layer_id)
            .unwrap();
        // Everything after the highest selected placement remains untouched.
        // Counting that tail also handles partially retained raw placements.
        let trailing_placements = anchor_layer.placements.len() - anchor.placement_idx - 1;
        let mut new_layers = Vec::new();
        let mut area_payloads = Vec::new();
        for layer in &parent.layers {
            let mine: Vec<_> = refs
                .iter()
                .filter(|reference| reference.layer_id == layer.layer_id)
                .collect();
            if mine.is_empty() {
                continue;
            }
            let layer_id = new_layers.len() as u16 + 1;
            let mut placements = Vec::new();
            for reference in mine {
                if let Some((raw, objects, min, max, clip)) = &area {
                    // Capture each depth separately; one combined clipboard puts
                    // all raw graphics below all symbols and loses interleaving.
                    let whole_object = objects.contains(reference);
                    let selection = Selection::RawArea {
                        placements: if !whole_object && raw.contains(reference) {
                            vec![*reference]
                        } else {
                            Vec::new()
                        },
                        objects: if whole_object {
                            vec![*reference]
                        } else {
                            Vec::new()
                        },
                        bounds_min: *min,
                        bounds_max: *max,
                        clip_polygon: clip.clone(),
                    };
                    let payload =
                        crate::selection_edit::capture_clipboard(&self.state.project, &selection);
                    if !payload.is_empty() {
                        area_payloads.push((layer_id, payload));
                    }
                } else {
                    let mut placement = layer.placements[reference.placement_idx].clone();
                    placement.instance_id = 0;
                    placement.frame = 0;
                    placement.tween = Tween::None;
                    placements.push(placement);
                }
            }
            new_layers.push(Layer {
                layer_id,
                timeline_frame_count: None,
                name: layer.name.clone(),
                explicit_keyframes: Vec::new(),
                placements,
            });
        }
        if area.is_some() && area_payloads.is_empty() {
            self.session.status = "Convert to Symbol: selection is empty".to_string();
            return;
        }
        let symbol_name =
            unique_symbol_name(&self.state.project, &format!("Symbol {new_q0rg_id}"), None);
        self.state.project.q0rgs.push(Q0rg {
            q0rg_id: new_q0rg_id,
            name: symbol_name,
            frame_count: 1,
            script: String::new(),
            layers: new_layers,
        });
        for (layer_id, payload) in area_payloads {
            crate::selection_edit::paste_payload(
                &mut self.state.project,
                new_q0rg_id,
                layer_id,
                0,
                &payload,
                q0s_format::v2::Vec2::new(0.0, 0.0),
            );
        }
        if let Some((raw, objects, min, max, clip)) = &area {
            crate::selection_edit::remove_raw_area_and_objects(
                &mut self.state.project,
                raw,
                objects,
                *min,
                *max,
                clip.as_deref(),
                frame,
            );
        } else {
            let parent = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
                .unwrap();
            for layer in &mut parent.layers {
                for reference in refs
                    .iter()
                    .rev()
                    .filter(|reference| reference.layer_id == layer.layer_id)
                {
                    layer.placements.remove(reference.placement_idx);
                }
                if refs
                    .iter()
                    .any(|reference| reference.layer_id == layer.layer_id)
                {
                    layer.ensure_explicit_keyframe(frame);
                }
            }
        }
        let layer = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
            .unwrap()
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == anchor.layer_id)
            .unwrap();
        let placement_idx = layer.placements.len().saturating_sub(trailing_placements);
        layer.extend_timeline_through(frame);
        layer.placements.insert(
            placement_idx,
            Placement {
                instance_id: 0,
                frame,
                target: Target::Q0rg(new_q0rg_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        );
        self.session.current_layer_id = anchor.layer_id;
        self.session.timeline_scroll_to_layer = Some((parent_q0rg_id, anchor.layer_id));
        self.session.selection = Selection::Placement {
            q0rg_id: parent_q0rg_id,
            layer_id: anchor.layer_id,
            placement_idx,
        };
        rebind_transform_pivot(
            &mut self.session.transform_pivot,
            &selection_before,
            &self.session.selection,
        );
        self.state.mark_dirty();
        self.textures.invalidate();
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
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return;
        }
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
        self.state.mark_dirty();
        self.session.status = "stroke converted to fill".to_string();
    }

    fn paste_bitmap_from_clipboard(&mut self, clipboard: crate::system_clipboard::ClipboardBitmap) {
        if !self.project_active || self.show_home || self.active_document_is_q0lang() {
            return;
        }
        if self.current_layer_is_locked() || !self.current_layer_is_visible() {
            self.session.status =
                "show and unlock the target layer before pasting a bitmap".to_string();
            return;
        }
        if self.session.pending_timeline_frame.is_some() {
            self.session.status =
                "create the future frame with F5, F6, or F7 before pasting a bitmap".to_string();
            return;
        }
        if self.current_layer_is_folder() {
            self.session.status = "folders cannot contain artwork".to_string();
            return;
        }
        if crate::audio::audio_clip_at_frame(
            &self.state.project,
            self.session.current_q0rg_id,
            self.session.current_layer_id,
            self.session.current_frame,
        )
        .is_some()
        {
            self.session.status = "audio frames cannot contain artwork".to_string();
            return;
        }
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
            self.session.status =
                "clipboard bitmap paste failed: asset id space exhausted".to_string();
            return;
        };
        let bitmap = match crate::bitmap_import::decode_bitmap_bytes(&clipboard.encoded, asset_id) {
            Ok(bitmap) => bitmap,
            Err(error) => {
                self.session.status = format!("clipboard bitmap paste failed: {error}");
                return;
            }
        };
        let target = self.session.stage_cursor.unwrap_or_else(|| {
            Vec2::new(
                self.state.project.meta.stage_width as f32 * 0.5,
                self.state.project.meta.stage_height as f32 * 0.5,
            )
        });
        let q0rg_id = self.session.current_q0rg_id;
        let layer_id = self.session.current_layer_id;
        let frame = self.session.current_frame;
        let before = self.state.project.clone();
        if crate::tools::materialize_layer_keyframe_for_edit(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
        )
        .is_none()
        {
            self.state.project = before;
            self.session.status = "could not paste bitmap on this frame".to_string();
            return;
        }
        self.history.snapshot(&before);
        let width = f32::from(bitmap.width);
        let height = f32::from(bitmap.height);
        self.state.project.assets.push(Asset::Bitmap(bitmap));
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
            self.state.project = before;
            self.session.status = "could not paste bitmap on this layer".to_string();
            return;
        };
        let placement_idx = layer.placements.len();
        layer.push_placement(Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D {
                tx: target.x - width * 0.5,
                ty: target.y - height * 0.5,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
        });
        self.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        };
        self.session.timeline_selection = None;
        self.session.timeline_layer_selection = None;
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = format!(
            "pasted bitmap from clipboard ({}x{})",
            width as u32, height as u32
        );
    }

    fn process_pending_clipboard_bitmap(&mut self) {
        if let Some(bitmap) = self.pending_clipboard_bitmap.take() {
            self.paste_bitmap_from_clipboard(bitmap);
        }
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
                self.state.mark_dirty();
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
                        .map_err(|error| format!("q0v import failed: {error}"))
                        .and_then(|bytes| {
                            q0video::q0v::validate_bytes(&bytes)?;
                            Ok(bytes)
                        });
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
        } else if crate::audio::is_supported_audio_path(path) {
            let (sender, receiver) = mpsc::channel();
            let worker_path = source_path.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("q0editor-audio-import".to_string())
                .spawn(move || {
                    let result = crate::audio::transcode_audio_to_q0v(&worker_path);
                    let _ = sender.send(result);
                })
            {
                self.session.status = format!("start audio import worker failed: {error}");
                return;
            }
            receiver
        } else {
            self.session.status = format!(
                "unsupported media file: {} (expected png, jpg, webp, wav, mp3, ogg, flac, mp4 or q0v)",
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
        } else if crate::audio::is_supported_audio_path(path) {
            format!("decoding audio {}...", path.display())
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
                let is_audio = crate::audio::is_supported_audio_path(&job.path);
                let prefix = if is_mp4 {
                    "mp4 import failed"
                } else if is_audio {
                    "audio import failed"
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
        let media = match q0video::q0v::probe_header(&bytes) {
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
        let default_name = if media.spec.audio && !media.spec.video {
            "Audio"
        } else {
            "Video"
        };
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(default_name)
            .to_string();

        self.history.snapshot(&self.state.project);
        self.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id,
            bytes: bytes.into(),
        }));
        self.state
            .project
            .asset_names
            .insert(asset_id, name.clone());
        self.state.mark_dirty();
        self.session.selection = Selection::Asset(asset_id);
        self.textures.invalidate_asset(asset_id);
        self.session.status = if media.spec.audio && !media.spec.video {
            let seconds = media.audio_samples_per_channel as f64
                / f64::from(media.spec.audio_sample_rate.max(1));
            format!(
                "imported {name} as audio asset {asset_id} ({seconds:.2}s, {} hz, {} channel(s))",
                media.spec.audio_sample_rate, media.spec.audio_channels
            )
        } else {
            format!(
                "imported {name} as q0v asset {asset_id} ({}x{}, {} frames at {} fps{})",
                media.spec.width,
                media.spec.height,
                media.spec.timeline_frames,
                media.spec.fps,
                if media.spec.audio { "; audio" } else { "" }
            )
        };
    }
    pub(crate) fn break_apart_fx_block_reason(&self) -> Option<&'static str> {
        let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection
        else {
            return None;
        };
        let layer = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        let placement = layer.placements.get(placement_idx)?;
        let active_fx =
            q0s_format::raster::active_placement_states_at(layer, self.session.current_frame)
                .into_iter()
                .find(|active| active.index == placement_idx)
                .map(|active| active.fx)
                .unwrap_or(placement.fx);
        if !active_fx.is_identity() {
            return Some(
                "Reset instance FX before Break Apart: group effects cannot be flattened losslessly.",
            );
        }
        if let Target::Q0rg(child_id) = placement.target {
            if q0s_format::raster::q0rg_frame_has_placement_fx(
                &self.state.project,
                child_id,
                self.session.current_frame,
            ) {
                return Some(
                    "Break Apart is disabled while the symbol contains active FX, so nested effects are never silently destroyed.",
                );
            }
        }
        None
    }

    pub fn can_break_apart_selection(&self) -> bool {
        if self.break_apart_fx_block_reason().is_some() {
            return false;
        }
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
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return;
        }
        if let Some(reason) = self.break_apart_fx_block_reason() {
            self.session.status = reason.to_string();
            return;
        }
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
        let bake_transform = Affine::from_transform(active_transform);
        #[cfg(feature = "appearance-mask-eraser")]
        let baked_appearance =
            baked_vector_appearance(&self.state.project, &source, bake_transform);
        let baked = bake_vector_asset(&source, new_asset_id, bake_transform);
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
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = baked_appearance {
            self.state
                .project
                .asset_appearances
                .insert(new_asset_id, appearance);
        }
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
            crate::tools::remove_assets_and_metadata(&mut self.state.project, [new_asset_id]);
            return;
        };
        placement.target = Target::Asset(new_asset_id);
        placement.transform = Transform2D::IDENTITY;
        placement.tween = Tween::None;
        self.session.selection =
            raw_vector_selection(q0rg_id, layer_id, mapped_idx, source.paths.len());
        self.session.status = "Vector broken apart into raw graphics".to_string();
        self.state.mark_dirty();
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
        #[cfg(feature = "appearance-mask-eraser")]
        let mut pending_appearances = Vec::new();
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
                                #[cfg(feature = "appearance-mask-eraser")]
                                if let Some(appearance) =
                                    baked_vector_appearance(&self.state.project, &vector, composed)
                                {
                                    pending_appearances.push((next_asset_id, appearance));
                                }
                                let baked = bake_vector_asset(&vector, next_asset_id, composed);
                                let path_count = baked.paths.len();
                                pending_assets.push(Asset::Vector(baked));
                                pieces.push(BreakApartPiece {
                                    placement: Placement {
                                        instance_id: 0,
                                        frame: self.session.current_frame,
                                        target: Target::Asset(next_asset_id),
                                        transform: Transform2D::IDENTITY,
                                        tween: Tween::None,
                                        fx: Default::default(),
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
                            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => {
                                let Some(transform) = affine_to_transform(composed) else {
                                    self.session.status =
                                        "Break Apart cannot represent a singular bitmap transform"
                                            .to_string();
                                    return;
                                };
                                pieces.push(BreakApartPiece {
                                    placement: Placement {
                                        instance_id: 0,
                                        frame: self.session.current_frame,
                                        target: Target::Asset(asset_id),
                                        transform,
                                        tween: Tween::None,
                                        fx: Default::default(),
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
                                instance_id: 0,
                                frame: self.session.current_frame,
                                target: Target::Q0rg(nested_id),
                                transform,
                                tween: Tween::None,
                                fx: Default::default(),
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
        #[cfg(feature = "appearance-mask-eraser")]
        self.state
            .project
            .asset_appearances
            .extend(pending_appearances);
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
        self.state.mark_dirty();
        self.textures.invalidate();
    }

    fn import_external_library_drag(
        &self,
        payload: LibraryDragPayload,
    ) -> Result<Option<(ProjectV2, LibraryItem)>, String> {
        if Some(payload.source_tab_id) == self.active_project_tab_id() {
            return Ok(None);
        }
        let source = self
            .project_for_document_tab(payload.source_tab_id)
            .ok_or_else(|| "source library project is no longer open".to_string())?;
        crate::library_transfer::import_library_item(source, &self.state.project, payload.item)
            .map(Some)
    }

    /// Run one cross-project library drop as a single undoable transaction.
    /// Placement helpers keep their existing history behaviour, but their
    /// intermediate import snapshot is discarded and replaced with the true
    /// pre-drag project. If placement is rejected, the imported graph is rolled
    /// back completely instead of leaving orphan library items behind.
    fn with_external_library_import(
        &mut self,
        payload: LibraryDragPayload,
        place: impl FnOnce(&mut Self, LibraryItem),
    ) {
        let imported = match self.import_external_library_drag(payload) {
            Ok(None) => {
                place(self, payload.item);
                return;
            }
            Ok(Some(imported)) => imported,
            Err(error) => {
                self.session.status = format!("library copy failed: {error}");
                return;
            }
        };
        let (candidate, imported_item) = imported;
        let original = self.state.project.clone();
        let original_dirty = self.state.dirty;
        let mut real_history = std::mem::take(&mut self.history);
        self.state.project = candidate.clone();
        place(self, imported_item);
        let placed = self.state.project != candidate;
        let _temporary_history = std::mem::take(&mut self.history);
        self.history = std::mem::take(&mut real_history);
        if placed {
            self.history.snapshot(&original);
            self.state.mark_dirty();
            self.textures.invalidate();
        } else {
            self.state.project = original;
            self.state.dirty = original_dirty;
            self.textures.invalidate();
        }
    }

    fn place_library_drag_at(&mut self, payload: LibraryDragPayload, position: Vec2) {
        self.with_external_library_import(payload, |app, item| {
            app.place_library_item_at(item, position);
        });
    }

    fn place_library_drag_on_timeline(
        &mut self,
        payload: LibraryDragPayload,
        layer_id: u16,
        frame: u16,
    ) {
        self.with_external_library_import(payload, |app, item| {
            app.place_library_item_on_timeline(item, layer_id, frame);
        });
    }

    fn place_library_item_on_timeline(
        &mut self,
        item: LibraryItem,
        target_layer_id: u16,
        frame: u16,
    ) {
        let q0rg_id = self.session.current_q0rg_id;
        if self.state.project.layer_is_locked(q0rg_id, target_layer_id)
            || !self
                .state
                .project
                .layer_is_visible(q0rg_id, target_layer_id)
        {
            self.session.status =
                "show and unlock the target layer before placing media".to_string();
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
                    Asset::Q0v(video) => q0video::q0v::probe_header(&video.bytes)
                        .ok()
                        .map(|header| (asset_id, header.spec, header.audio_samples_per_channel)),
                    _ => None,
                }),
            LibraryItem::Q0rg(_) => None,
        };

        let Some((asset_id, media_spec, audio_samples_per_channel)) = q0v else {
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
        let audio_only = media_spec.audio && !media_spec.video;
        let duration_frames = if audio_only {
            audio_samples_per_channel
                .saturating_mul(u64::from(project_fps))
                .div_ceil(u64::from(media_spec.audio_sample_rate.max(1)))
                .max(1)
        } else {
            u64::from(media_spec.timeline_frames)
                .saturating_mul(u64::from(project_fps))
                .div_ceil(u64::from(media_spec.fps.max(1)))
                .max(1)
        };
        let end_exclusive = u64::from(frame).saturating_add(duration_frames);
        if end_exclusive > u64::from(u16::MAX) {
            self.session.status = if audio_only {
                "audio is too long for this timeline".to_string()
            } else {
                "video is too long for this timeline".to_string()
            };
            return;
        }
        let mut end_exclusive = end_exclusive as u16;
        if audio_only {
            let Some(layer) = self.state.project.q0rgs[q0rg_index]
                .layers
                .iter()
                .find(|layer| layer.layer_id == target_layer_id)
            else {
                self.session.status = "target audio layer is no longer available".to_string();
                return;
            };
            if let Some(next) = layer
                .keyframe_frames()
                .into_iter()
                .filter(|key| *key > frame)
                .chain(
                    self.state
                        .project
                        .audio_clips
                        .iter()
                        .filter(|clip| {
                            clip.q0rg_id == q0rg_id
                                && clip.layer_id == target_layer_id
                                && clip.start_frame > frame
                        })
                        .map(|clip| clip.start_frame),
                )
                .min()
            {
                end_exclusive = end_exclusive.min(next);
            }
        }
        let old_frame_count = self.state.project.q0rgs[q0rg_index].frame_count;
        let new_frame_count = old_frame_count.max(end_exclusive);
        if audio_only {
            if crate::audio::audio_range_intersects(
                &self.state.project,
                q0rg_id,
                target_layer_id,
                frame,
                end_exclusive,
            ) {
                self.session.status = "audio clips cannot overlap on the same layer".to_string();
                return;
            }
            if crate::audio::visual_content_conflicts_with_audio_range(
                &self.state.project,
                q0rg_id,
                target_layer_id,
                frame,
                end_exclusive,
            ) {
                self.session.status =
                    "audio frames must be empty; choose an empty timeline range".to_string();
                return;
            }
            self.history.snapshot(&self.state.project);
            let q0rg = &mut self.state.project.q0rgs[q0rg_index];
            q0rg.frame_count = new_frame_count;
            let Some(layer) = q0rg
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == target_layer_id)
            else {
                return;
            };
            layer.ensure_explicit_keyframe(frame);
            layer.extend_timeline_through(end_exclusive.saturating_sub(1));
            self.state.project.audio_clips.push(AudioClip {
                q0rg_id,
                layer_id: target_layer_id,
                start_frame: frame,
                asset_id,
                gain: 1.0,
                muted: false,
                ..Default::default()
            });
            self.session.current_layer_id = target_layer_id;
            self.session.current_frame = frame;
            self.session.timeline_selection = Some(crate::state::TimelineSelection::single(
                target_layer_id,
                frame,
            ));
            self.session.selection = Selection::None;
            self.state.mark_dirty();
            self.session.status = format!(
                "placed audio on layer {} at frame {} ({} project frames)",
                target_layer_id,
                frame + 1,
                end_exclusive - frame
            );
            return;
        }
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
            .unwrap_or_else(|| {
                if audio_only {
                    format!("Audio {asset_id}")
                } else {
                    format!("Video {asset_id}")
                }
            });
        let target = Target::Asset(asset_id);
        let transform = if audio_only {
            Transform2D::IDENTITY
        } else {
            let center = Vec2::new(
                self.state.project.meta.stage_width as f32 * 0.5,
                self.state.project.meta.stage_height as f32 * 0.5,
            );
            crate::render::centered_target_transform(&self.state.project, target, center)
        };
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
                timeline_frame_count: None,
                name,
                explicit_keyframes,
                placements: vec![Placement {
                    instance_id: 0,
                    frame,
                    target,
                    transform,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            },
        );
        if let Some(parent_folder_id) = parent_folder_id {
            self.state.project.layer_metadata.insert(
                LayerKey::new(q0rg_id, next_layer_id),
                LayerMetadata {
                    kind: LayerKind::Normal,
                    parent_folder_id: Some(parent_folder_id),
                    mask_layer_id: None,
                    collapsed: false,
                    hidden: false,
                    locked: false,
                },
            );
        }
        self.session.current_layer_id = next_layer_id;
        self.session.current_frame = frame;
        self.session.timeline_selection = Some(crate::state::TimelineSelection::single(
            next_layer_id,
            frame,
        ));
        self.session.selection = if audio_only {
            Selection::None
        } else {
            Selection::Placement {
                q0rg_id,
                layer_id: next_layer_id,
                placement_idx: 0,
            }
        };
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.status = if audio_only {
            format!(
                "placed audio on timeline at frame {} ({} project frames)",
                frame + 1,
                duration_frames
            )
        } else {
            format!(
                "placed q0v on timeline at frame {} ({} project frames)",
                frame + 1,
                duration_frames
            )
        };
    }

    fn place_library_item_at(&mut self, item: LibraryItem, position: q0s_format::v2::Vec2) {
        if crate::audio::library_item_is_audio_only(&self.state.project, item) {
            self.session.status =
                "audio lives on the timeline; drag it onto a timeline frame".to_string();
            return;
        }
        if self.current_layer_is_locked() || !self.current_layer_is_visible() {
            self.session.status =
                "show and unlock the target layer before placing artwork".to_string();
            return;
        }
        if self.current_layer_is_folder() {
            self.session.status = "folders cannot contain artwork".to_string();
            return;
        }
        if crate::audio::audio_clip_at_frame(
            &self.state.project,
            self.session.current_q0rg_id,
            self.session.current_layer_id,
            self.session.current_frame,
        )
        .is_some()
        {
            self.session.status = "audio frames cannot contain artwork".to_string();
            return;
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
            layer.push_placement(Placement {
                instance_id: 0,
                frame,
                target,
                transform,
                tween: Tween::None,
                fx: Default::default(),
            });
            self.state.mark_dirty();
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
        self.enter_q0rg_with_context(child_id, None);
    }

    fn enter_q0rg_in_place(&mut self, child_id: u16, layer_id: u16, placement_idx: usize) {
        let parent_q0rg_id = self.session.current_q0rg_id;
        let parent_frame = self.session.current_frame;
        let Some((instance_id, fallback_transform)) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
            .and_then(|parent_q0rg| {
                let layer = parent_q0rg
                    .layers
                    .iter()
                    .find(|layer| layer.layer_id == layer_id)?;
                let placement = layer.placements.get(placement_idx)?;
                if placement.target != Target::Q0rg(child_id) {
                    return None;
                }
                let active = q0s_format::raster::active_placement_states_at(layer, parent_frame)
                    .into_iter()
                    .find(|active| active.index == placement_idx)?;
                Some((
                    placement.instance_id,
                    Affine::from_transform(active.transform),
                ))
            })
        else {
            return;
        };
        let local_transform = q0s_format::rig::rig_for_q0rg(&self.state.project, parent_q0rg_id)
            .map(|rig| q0s_format::rig::evaluate_rig(rig, f32::from(parent_frame), &[]))
            .and_then(|pose| pose.binding_transform(instance_id))
            .unwrap_or(fallback_transform);
        let parent_to_stage = self
            .session
            .edit_in_place
            .last()
            .map(|context| context.child_to_stage)
            .unwrap_or(Affine::IDENTITY);
        let child_to_stage = Affine::compose(parent_to_stage, local_transform);
        if child_to_stage.inverse().is_none() {
            self.session.status = "can't edit this symbol in place at zero scale".to_string();
            return;
        }
        self.enter_q0rg_with_context(
            child_id,
            Some(EditInPlaceContext {
                parent_q0rg_id,
                parent_frame,
                layer_id,
                placement_idx,
                parent_to_stage,
                child_to_stage,
            }),
        );
    }

    fn enter_q0rg_with_context(&mut self, child_id: u16, context: Option<EditInPlaceContext>) {
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
        if let Some(context) = context {
            self.session.edit_in_place.push(context);
        } else {
            self.session.edit_in_place.clear();
        }
        self.session.current_q0rg_id = child_id;
        self.refresh_layer_for_current_q0rg();
        self.session.current_frame = 0;
        self.session.timeline_selection = None;
        self.session.timeline_cell_selection = None;
        self.session.selection = Selection::None;
        self.session.tool_state = crate::state::ToolState::Idle;
        self.session.status = "symbol opened".to_string();
    }

    fn breadcrumb_jump_to(&mut self, depth: usize) {
        // Breadcrumb entries are indexed exactly as displayed in the stage bar.
        let Some(new_current) = self.session.breadcrumb.get(depth).copied() else {
            return;
        };
        let edit_context_index = self
            .session
            .edit_in_place
            .iter()
            .rposition(|context| context.parent_q0rg_id == new_current);
        self.session.current_frame = edit_context_index
            .map(|index| self.session.edit_in_place[index].parent_frame)
            .unwrap_or(0);
        if let Some(index) = edit_context_index {
            self.session.edit_in_place.truncate(index);
        } else {
            self.session.edit_in_place.clear();
        }
        self.session.breadcrumb.truncate(depth);
        self.session.current_q0rg_id = new_current;
        self.refresh_layer_for_current_q0rg();
        self.session.pending_timeline_frame = None;
        self.session.timeline_selection = None;
        self.session.timeline_cell_selection = None;
        self.session.timeline_layer_selection = None;
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
        let affected_layers: std::collections::BTreeSet<(u16, u16)> = refs
            .iter()
            .map(|reference| (reference.q0rg_id, reference.layer_id))
            .collect();

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
            crate::tools::remove_assets_and_metadata(
                &mut self.state.project,
                empty_assets.iter().copied(),
            );
        }
        for (q0rg_id, layer_id) in affected_layers {
            crate::tools::preserve_blank_keyframe_after_content_delete(
                &mut self.state.project,
                q0rg_id,
                layer_id,
                self.session.current_frame,
            );
        }
        self.state.mark_dirty();
        self.session.selection = Selection::None;
        self.textures.invalidate();
        true
    }

    /// Delete the selected timeline cells without removing time itself.
    /// Each selected cell becomes a real blank keyframe, so deleting a held
    /// frame also stops the previous keyframe from leaking through it.
    fn delete_timeline_selection(&mut self) -> Option<(usize, usize, bool)> {
        if let Some((q0rg_id, cells)) = self.session.timeline_cell_selection.clone() {
            return self.delete_timeline_cell_selection(q0rg_id, cells);
        }
        let selection = self.session.timeline_selection?;
        if self.timeline_selection_touches_locked_layer(selection) {
            self.session.status = "locked timeline frames cannot be cleared".to_string();
            return None;
        }
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
        let mut audio_cells = std::collections::HashSet::new();
        let mut audio_clips_to_remove = std::collections::HashSet::new();
        for clip in &self.state.project.audio_clips {
            if clip.q0rg_id != q0rg_id || !editable_layer_ids.contains(&clip.layer_id) {
                continue;
            }
            let Some(clip_end) = q0s_format::v2::audio_clip_end_frame(&self.state.project, *clip)
            else {
                continue;
            };
            let overlap_start = clip.start_frame.max(first_frame);
            let overlap_end = clip_end.min(last_frame.saturating_add(1));
            if overlap_start < overlap_end {
                audio_clips_to_remove.insert((
                    clip.q0rg_id,
                    clip.layer_id,
                    clip.start_frame,
                    clip.asset_id,
                ));
            }
            for frame in overlap_start..overlap_end {
                audio_cells.insert((clip.layer_id, frame));
            }
        }
        let audio_selection = !audio_cells.is_empty();
        if editable_layer_ids.is_empty() {
            self.session.selection = Selection::None;
            self.session.timeline_selection = None;
            return Some((0, 0, false));
        }

        let needs_change = audio_selection
            || editable_layer_ids.iter().any(|layer_id| {
                let layer = q0rg
                    .layers
                    .iter()
                    .find(|layer| layer.layer_id == *layer_id)
                    .expect("selected editable layer still exists");
                (first_frame..=last_frame).any(|frame| !layer.is_blank_keyframe(frame))
                    || layer.placements.iter().any(|placement| {
                        placement
                            .tween
                            .to_frame()
                            .is_some_and(|to_frame| (first_frame..=last_frame).contains(&to_frame))
                    })
            });
        if !needs_change {
            self.session.selection = Selection::None;
            return Some((cell_count, 0, false));
        }

        self.history.snapshot(&self.state.project);
        let before_audio = self.state.project.audio_clips.len();
        self.state.project.audio_clips.retain(|clip| {
            !audio_clips_to_remove.contains(&(
                clip.q0rg_id,
                clip.layer_id,
                clip.start_frame,
                clip.asset_id,
            ))
        });
        let removed_audio = before_audio - self.state.project.audio_clips.len();
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .expect("selected q0rg disappeared during timeline delete");
        let mut removed_items = removed_audio;
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
                if placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| (first_frame..=last_frame).contains(&to_frame))
                {
                    placement.tween = Tween::None;
                }
            }
            for frame in first_frame..=last_frame {
                if !audio_cells.contains(&(layer_id, frame)) {
                    layer.ensure_explicit_keyframe(frame);
                }
            }
        }

        self.state.mark_dirty();
        self.session.selection = Selection::None;
        Some((cell_count, removed_items, true))
    }

    fn remove_selected_timeline_frames(&mut self) {
        if self.camera_track_is_selected() || self.session.timeline_rig_key_selection.is_some() {
            self.session.status = "select artwork frame cells to remove".to_string();
            return;
        }

        if self.session.pending_timeline_frame.is_some() {
            self.session.status =
                "future frame is not created; press f5, f6, or f7 first".to_string();
            return;
        }

        let q0rg_id = self.session.current_q0rg_id;
        let mut cells = self
            .session
            .timeline_cell_selection
            .as_ref()
            .filter(|(selected_q0rg, selected)| *selected_q0rg == q0rg_id && !selected.is_empty())
            .map(|(_, selected)| selected.clone())
            .unwrap_or_default();
        if cells.is_empty() {
            if let Some(selection) = self.session.timeline_selection {
                cells = crate::timeline_edit::selected_frame_cells(
                    &self.state.project,
                    q0rg_id,
                    selection,
                )
                .unwrap_or_default();
            }
        }
        if cells.is_empty() {
            cells.push((self.session.current_layer_id, self.session.timeline_frame()));
        }

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
        let frame_count = q0rg.frame_count;
        cells.retain(|(layer_id, frame)| {
            *frame < frame_count
                && q0rg.layers.iter().any(|layer| {
                    layer.layer_id == *layer_id
                        && !self.state.project.layer_is_folder(q0rg_id, *layer_id)
                })
        });
        if cells.is_empty() {
            self.session.status = "no timeline frame cells selected".to_string();
            return;
        }
        if cells
            .iter()
            .any(|(layer_id, _)| self.state.project.layer_is_locked(q0rg_id, *layer_id))
        {
            self.session.status = "unlock selected layers before removing frame cells".to_string();
            return;
        }

        let first_frame = cells.iter().map(|(_, frame)| *frame).min().unwrap_or(0);
        let active_layer_id = if cells
            .iter()
            .any(|(layer_id, _)| *layer_id == self.session.current_layer_id)
        {
            self.session.current_layer_id
        } else {
            cells[0].0
        };
        let before = self.state.project.clone();
        let removed = crate::timeline_edit::remove_selected_frame_cells(
            &mut self.state.project,
            q0rg_id,
            &cells,
        )
        .unwrap_or(0);
        if removed == 0 {
            self.session.status = "no timeline frame cells selected".to_string();
            return;
        }
        if self.state.project != before {
            self.history.snapshot(&before);
            self.state.mark_dirty();
            self.textures.invalidate();
            let shifted_layers = cells
                .iter()
                .map(|(layer_id, _)| *layer_id)
                .collect::<std::collections::HashSet<_>>()
                .len();
            self.session.status =
                format!("removed {removed} frame cell(s) and shifted {shifted_layers} layer(s)");
        } else {
            self.session.status = "selected frame cells were already empty".to_string();
        }

        self.session.current_layer_id = active_layer_id;
        let frame_count = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map_or(1, |q| q.frame_count);
        let first_frame = first_frame.min(frame_count.saturating_sub(1));
        self.session.set_timeline_frame(first_frame, frame_count);
        self.session.pending_timeline_frame = None;
        self.session.timeline_selection =
            Some(TimelineSelection::single(active_layer_id, first_frame));
        self.session.timeline_cell_selection = None;
        self.session.timeline_layer_selection = None;
        self.session.selection = Selection::None;
    }

    fn delete_timeline_cell_selection(
        &mut self,
        q0rg_id: u16,
        cells: Vec<(u16, u16)>,
    ) -> Option<(usize, usize, bool)> {
        if q0rg_id != self.session.current_q0rg_id {
            return None;
        }
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
        let cells: std::collections::HashSet<(u16, u16)> = cells
            .into_iter()
            .filter(|(layer_id, frame)| {
                *frame < q0rg.frame_count
                    && q0rg.layers.iter().any(|layer| {
                        layer.layer_id == *layer_id
                            && !self.state.project.layer_is_folder(q0rg_id, *layer_id)
                    })
            })
            .collect();
        if cells.is_empty() {
            return Some((0, 0, false));
        }
        if cells
            .iter()
            .any(|(layer_id, _)| self.state.project.layer_is_locked(q0rg_id, *layer_id))
        {
            self.session.status = "locked timeline frames cannot be cleared".to_string();
            return None;
        }

        let mut audio_clips_to_remove = std::collections::HashSet::new();
        let mut audio_cells = std::collections::HashSet::new();
        for clip in &self.state.project.audio_clips {
            if clip.q0rg_id != q0rg_id {
                continue;
            }
            let Some(clip_end) = q0s_format::v2::audio_clip_end_frame(&self.state.project, *clip)
            else {
                continue;
            };
            for &(layer_id, frame) in &cells {
                if layer_id == clip.layer_id && frame >= clip.start_frame && frame < clip_end {
                    audio_cells.insert((layer_id, frame));
                    audio_clips_to_remove.insert((
                        clip.q0rg_id,
                        clip.layer_id,
                        clip.start_frame,
                        clip.asset_id,
                    ));
                }
            }
        }
        let needs_change = !audio_clips_to_remove.is_empty()
            || cells.iter().any(|(layer_id, frame)| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == *layer_id)
                    .is_some_and(|layer| {
                        !layer.is_blank_keyframe(*frame)
                            || layer
                                .placements
                                .iter()
                                .any(|placement| placement.tween.to_frame() == Some(*frame))
                    })
            });
        if !needs_change {
            self.session.selection = Selection::None;
            return Some((cells.len(), 0, false));
        }

        self.history.snapshot(&self.state.project);
        let before_audio = self.state.project.audio_clips.len();
        self.state.project.audio_clips.retain(|clip| {
            !audio_clips_to_remove.contains(&(
                clip.q0rg_id,
                clip.layer_id,
                clip.start_frame,
                clip.asset_id,
            ))
        });
        let mut removed_items = before_audio - self.state.project.audio_clips.len();
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .expect("selected q0rg disappeared during sparse timeline delete");
        for (layer_id, frame) in cells.iter().copied() {
            let Some(layer) = q0rg
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
            else {
                continue;
            };
            let before = layer.placements.len();
            layer
                .placements
                .retain(|placement| placement.frame != frame);
            removed_items += before - layer.placements.len();
            for placement in &mut layer.placements {
                if placement.tween.to_frame() == Some(frame) {
                    placement.tween = Tween::None;
                }
            }
            if !audio_cells.contains(&(layer_id, frame)) {
                layer.ensure_explicit_keyframe(frame);
            }
        }
        self.state.mark_dirty();
        self.session.selection = Selection::None;
        Some((cells.len(), removed_items, true))
    }

    fn delete_selection(&mut self) -> bool {
        self.session
            .clear_inactive_frame_selection(&self.state.project);
        if self.selection_touches_locked_layer() {
            self.session.status = "layer is locked".to_string();
            return false;
        }
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
                        crate::tools::preserve_blank_keyframe_after_content_delete(
                            &mut self.state.project,
                            q0rg_id,
                            layer_id,
                            self.session.current_frame,
                        );
                        self.state.mark_dirty();
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
            Selection::StrokeSegment(segment) => {
                self.history.snapshot(&self.state.project);
                let Some(path) = crate::tools::materialize_raw_stroke_segment(self, segment) else {
                    return false;
                };
                let changed = crate::selection_edit::remove_materialized_paths_and_objects(
                    &mut self.state.project,
                    &[path],
                    &[],
                    self.session.current_frame,
                );
                if changed {
                    self.state.mark_dirty();
                    self.session.selection = Selection::None;
                    self.textures.invalidate();
                }
                changed
            }
            Selection::PathPoints {
                path,
                mut anchor_indices,
                ..
            }
            | Selection::SubselectPoints {
                path,
                mut anchor_indices,
                ..
            } => {
                anchor_indices.sort_unstable();
                anchor_indices.dedup();
                if anchor_indices.is_empty() {
                    // Subselect can activate a contour merely to reveal all of
                    // its points. Delete must not interpret that as "delete the
                    // whole contour" until concrete anchors are selected.
                    return false;
                }
                let source_path = self
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
                        Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
                    })
                    .cloned();
                let Some(source_path) = source_path else {
                    return false;
                };
                anchor_indices.retain(|index| *index < source_path.anchors.len());
                if anchor_indices.is_empty() {
                    return false;
                }
                let minimum_survivors = if source_path.closed { 3 } else { 1 };
                if source_path
                    .anchors
                    .len()
                    .saturating_sub(anchor_indices.len())
                    < minimum_survivors
                {
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
                let changed = self
                    .state
                    .project
                    .assets
                    .iter_mut()
                    .find(|asset| asset.id() == asset_id)
                    .and_then(|asset| match asset {
                        Asset::Vector(vector) => vector.paths.get_mut(path.path_idx),
                        Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
                    })
                    .is_some_and(|raw_path| {
                        crate::tools::delete_path_anchors_safely(raw_path, &anchor_indices)
                    });
                if changed {
                    self.textures.invalidate_asset(asset_id);
                    self.state.mark_dirty();
                    self.session.selection = Selection::None;
                }
                changed
            }
            Selection::Mixed { paths, objects } => {
                self.history.snapshot(&self.state.project);
                let changed = crate::selection_edit::remove_materialized_paths_and_objects(
                    &mut self.state.project,
                    &paths,
                    &objects,
                    self.session.current_frame,
                );
                if changed {
                    self.state.mark_dirty();
                    self.session.selection = Selection::None;
                    self.textures.invalidate();
                }
                changed
            }
            Selection::RawArea {
                placements,
                objects,
                bounds_min,
                bounds_max,
                clip_polygon,
            } => {
                self.history.snapshot(&self.state.project);
                let changed = crate::selection_edit::remove_raw_area_and_objects(
                    &mut self.state.project,
                    &placements,
                    &objects,
                    bounds_min,
                    bounds_max,
                    clip_polygon.as_deref(),
                    self.session.current_frame,
                );
                if changed {
                    self.state.mark_dirty();
                    self.session.selection = Selection::None;
                    self.textures.invalidate();
                }
                changed
            }
            Selection::Asset(asset_id) => {
                self.history.snapshot(&self.state.project);
                crate::tools::remove_assets_and_metadata(&mut self.state.project, [asset_id]);
                self.state
                    .project
                    .runtime
                    .library_item_folders
                    .remove(&q0s_format::v2::LibraryItemReference::Asset(asset_id));
                for q in &mut self.state.project.q0rgs {
                    for layer in &mut q.layers {
                        layer.placements.retain(|p| match p.target {
                            q0s_format::v2::Target::Asset(id) => id != asset_id,
                            _ => true,
                        });
                    }
                }
                self.state.mark_dirty();
                self.session.selection = Selection::None;
                self.textures.invalidate();
                true
            }
            Selection::Multi(refs) => {
                if refs.is_empty() {
                    return false;
                }
                self.history.snapshot(&self.state.project);
                let Some(mut sorted) = crate::tools::materialize_placement_refs_for_edit(
                    &mut self.state.project,
                    &refs,
                    self.session.current_frame,
                ) else {
                    return false;
                };
                let affected_layers: std::collections::BTreeSet<(u16, u16)> = sorted
                    .iter()
                    .map(|reference| (reference.q0rg_id, reference.layer_id))
                    .collect();
                // Remove from the back of each freshly materialized layer so
                // placement indices remain stable throughout the operation.
                sorted.sort_by(|a, b| {
                    a.q0rg_id
                        .cmp(&b.q0rg_id)
                        .then(a.layer_id.cmp(&b.layer_id))
                        .then(b.placement_idx.cmp(&a.placement_idx))
                });
                for reference in sorted {
                    if let Some(layer) = self
                        .state
                        .project
                        .q0rgs
                        .iter_mut()
                        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
                        .and_then(|q0rg| {
                            q0rg.layers
                                .iter_mut()
                                .find(|layer| layer.layer_id == reference.layer_id)
                        })
                    {
                        if reference.placement_idx < layer.placements.len() {
                            layer.placements.remove(reference.placement_idx);
                        }
                    }
                }
                for (q0rg_id, layer_id) in affected_layers {
                    crate::tools::preserve_blank_keyframe_after_content_delete(
                        &mut self.state.project,
                        q0rg_id,
                        layer_id,
                        self.session.current_frame,
                    );
                }
                self.state.mark_dirty();
                self.session.selection = Selection::None;
                true
            }
            Selection::RawParts {
                fills,
                strokes,
                stroke_segments,
                objects,
            } => {
                // Raw style parts are materialized before deletion so removing
                // an outline does not also erase its fill (and vice versa).
                // Display-object mixing is deliberately not created by the
                // selector; reject legacy/invalid mixed state defensively.
                if !objects.is_empty()
                    || (fills.is_empty() && strokes.is_empty() && stroke_segments.is_empty())
                {
                    return false;
                }
                self.history.snapshot(&self.state.project);
                let Some(paths) = crate::tools::materialize_raw_style_parts_with_segments(
                    self,
                    &fills,
                    &strokes,
                    &stroke_segments,
                ) else {
                    return false;
                };
                let changed = crate::selection_edit::remove_materialized_paths_and_objects(
                    &mut self.state.project,
                    &paths,
                    &[],
                    self.session.current_frame,
                );
                if changed {
                    self.state.mark_dirty();
                    self.session.selection = Selection::None;
                    self.textures.invalidate();
                }
                changed
            }
            Selection::Q0rg(_) | Selection::None => false,
        }
    }

    fn timeline_selection_is_multi(&self) -> bool {
        self.session.timeline_selection.is_some_and(|selection| {
            selection.anchor_layer_id != selection.focus_layer_id
                || selection.anchor_frame != selection.focus_frame
        })
    }

    fn materialize_timeline_selection(&mut self) {
        let Some(selection) = self.session.timeline_selection else {
            return;
        };
        if self.timeline_selection_touches_locked_layer(selection) {
            self.session.status = "locked timeline frames cannot be changed".to_string();
            return;
        }
        let before = self.state.project.clone();
        match crate::timeline_edit::materialize_selected_keyframes(
            &mut self.state.project,
            self.session.current_q0rg_id,
            selection,
        ) {
            Some(created) if created > 0 => {
                self.history.snapshot(&before);
                self.state.mark_dirty();
                self.session.selection = Selection::None;
                self.textures.invalidate();
                self.session.status = format!("inserted {created} selected keyframe(s)");
            }
            Some(_) => {
                self.session.status = "selected timeline cells are already keyframes".to_string();
            }
            None => {
                self.session.status = "could not insert selected keyframes".to_string();
            }
        }
    }

    fn blank_timeline_selection(&mut self) {
        if let Some(selection) = self.session.timeline_selection {
            if self.timeline_selection_touches_audio(selection) {
                if self.timeline_selection_touches_locked_layer(selection) {
                    self.session.status = "unlock selected layers before cutting audio".into();
                    return;
                }
                let cells = crate::timeline_edit::selected_frame_cells_including_virtual(
                    &self.state.project,
                    self.session.current_q0rg_id,
                    selection,
                )
                .unwrap_or_default();
                self.history.snapshot(&self.state.project);
                let mut changed = false;
                for (layer_id, frame) in cells {
                    changed |= crate::audio::keyframe_audio(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        layer_id,
                        frame,
                        false,
                    );
                    if let Some(layer) = self
                        .state
                        .project
                        .q0rgs
                        .iter_mut()
                        .find(|q| q.q0rg_id == self.session.current_q0rg_id)
                        .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == layer_id))
                    {
                        let before = layer.placements.len();
                        layer
                            .placements
                            .retain(|placement| placement.frame != frame);
                        changed |= layer.placements.len() != before;
                        if !layer.has_keyframe(frame) {
                            layer.ensure_explicit_keyframe(frame);
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.state.mark_dirty();
                }
                self.session.selection = Selection::None;
                if self.session.playing {
                    let _ = self.refresh_audio_playback();
                }
                self.session.status = "audio cut at selected blank keys".into();
                return;
            }
        }
        match self.delete_timeline_selection() {
            Some((cell_count, _, false)) => {
                self.session.status =
                    format!("{cell_count} selected timeline frame(s) already blank");
            }
            Some((cell_count, removed_items, true)) if removed_items > 0 => {
                self.session.status = format!(
                    "inserted {cell_count} blank keyframe(s), removed {removed_items} item(s)"
                );
            }
            Some((cell_count, _, true)) => {
                self.session.status = format!("inserted {cell_count} blank keyframe(s)");
            }
            None => {
                self.session.status = "could not insert selected blank keyframes".to_string();
            }
        }
    }

    fn move_camera_frames(&mut self, selection: CameraTimelineSelection, target_frame: u16) {
        if !self.camera_track_is_selected() {
            return;
        }
        let first = selection.first_frame();
        let last = selection.last_frame();
        let width = last.saturating_sub(first).saturating_add(1);
        let target_last = target_frame.saturating_add(width.saturating_sub(1));
        let frame_count = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == self.state.project.meta.entry_q0rg_id)
            .map(|q0rg| q0rg.frame_count)
            .unwrap_or(1);
        if target_last >= frame_count {
            self.session.status = "camera frames cannot be moved past the timeline end".to_string();
            return;
        }

        let selected_old = self
            .state
            .project
            .runtime
            .camera
            .keys
            .iter()
            .filter(|key| (first..=last).contains(&key.frame))
            .map(|key| key.frame)
            .collect::<std::collections::BTreeSet<_>>();
        if selected_old.is_empty() {
            self.session.status = "no camera keyframes in selection".to_string();
            return;
        }
        let delta = i32::from(target_frame) - i32::from(first);
        let remap = |frame: u16| -> Option<u16> {
            if selected_old.contains(&frame) {
                u16::try_from(i32::from(frame) + delta).ok()
            } else {
                Some(frame)
            }
        };
        let moved_frames = selected_old
            .iter()
            .filter_map(|frame| remap(*frame))
            .collect::<std::collections::BTreeSet<_>>();
        if self
            .state
            .project
            .runtime
            .camera
            .keys
            .iter()
            .any(|key| !selected_old.contains(&key.frame) && moved_frames.contains(&key.frame))
        {
            self.session.status = "camera keyframes cannot overwrite existing keys".to_string();
            return;
        }

        let before = self.state.project.clone();
        for key in &mut self.state.project.runtime.camera.keys {
            if selected_old.contains(&key.frame) {
                key.frame = remap(key.frame).expect("validated camera frame remap");
            }
            if let Some(to_frame) = key.tween.to_frame() {
                let mapped_to = remap(to_frame).unwrap_or(to_frame);
                key.tween = if mapped_to > key.frame {
                    key.tween.with_to_frame(mapped_to)
                } else {
                    Tween::None
                };
            }
        }
        self.state
            .project
            .runtime
            .camera
            .keys
            .sort_by_key(|key| key.frame);
        if q0s_format::v2::validate(&self.state.project).is_err() {
            self.state.project = before;
            self.session.status = "could not move camera frames".to_string();
            return;
        }
        self.history.snapshot(&before);
        self.session.camera_timeline_selection = Some(CameraTimelineSelection {
            anchor_frame: target_frame,
            focus_frame: target_last,
        });
        self.session.current_frame = target_frame;
        self.session.pending_timeline_frame = None;
        self.state.mark_dirty();
        self.session.status = "moved camera frames".to_string();
    }

    pub(crate) fn camera_track_is_selected(&self) -> bool {
        self.session.camera_track_selected
            && self.session.current_q0rg_id == self.state.project.meta.entry_q0rg_id
            && self.state.project.runtime.camera.enabled
    }

    fn insert_camera_frame_smart(&mut self) {
        let entry_id = self.state.project.meta.entry_q0rg_id;
        if self.session.current_q0rg_id != entry_id {
            self.session.camera_track_selected = false;
            return;
        }
        let frame = self
            .session
            .pending_timeline_frame
            .take()
            .unwrap_or(self.session.current_frame);
        let old_count = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == entry_id)
            .map(|q| q.frame_count)
            .unwrap_or(0);
        let Some(new_count) = (if frame >= old_count {
            frame.checked_add(1)
        } else {
            old_count.checked_add(1)
        }) else {
            self.session.status = "timeline is already at its maximum length".to_string();
            return;
        };

        let before = self.state.project.clone();
        if frame < old_count {
            for key in &mut self.state.project.runtime.camera.keys {
                if key.frame >= frame {
                    key.frame = key.frame.saturating_add(1);
                }
                if let Some(to_frame) = key.tween.to_frame() {
                    if to_frame >= frame {
                        key.tween = key.tween.with_to_frame(to_frame.saturating_add(1));
                    }
                }
            }
            self.state
                .project
                .runtime
                .camera
                .keys
                .sort_by_key(|key| key.frame);
        }
        if let Some(entry) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == entry_id)
        {
            entry.frame_count = new_count;
        }
        self.history.snapshot(&before);
        self.state.mark_dirty();
        self.session.current_frame = frame.min(new_count.saturating_sub(1));
        self.session.pending_timeline_frame = None;
        self.session.timeline_selection = None;
        self.session.timeline_layer_selection = None;
        self.session.selection = Selection::None;
        self.session.status = if frame < old_count {
            format!("inserted camera hold frame at {}", frame + 1)
        } else {
            format!("extended camera timeline through frame {}", frame + 1)
        };
    }

    fn insert_camera_keyframe_smart(&mut self) {
        let entry_id = self.state.project.meta.entry_q0rg_id;
        if self.session.current_q0rg_id != entry_id {
            self.session.camera_track_selected = false;
            return;
        }
        let current = self
            .session
            .pending_timeline_frame
            .take()
            .unwrap_or(self.session.current_frame);
        let has_exact = self.state.project.runtime.camera.has_keyframe(current);
        let target = if has_exact {
            current.saturating_add(1)
        } else {
            current
        };
        let Some(new_count) = target.checked_add(1) else {
            self.session.status = "timeline is already at its maximum length".to_string();
            return;
        };
        if self.state.project.runtime.camera.has_keyframe(target) {
            self.session.current_frame = target;
            self.session.status = format!("camera keyframe already exists at frame {}", target + 1);
            return;
        }

        let state = self.state.project.camera_state_at(target);
        let before = self.state.project.clone();
        let old_count = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == entry_id)
            .map(|q| q.frame_count)
            .unwrap_or(0);
        for key in &mut self.state.project.runtime.camera.keys {
            if key.frame < target
                && key
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| target <= to_frame)
            {
                key.tween = key.tween.with_to_frame(target);
            }
        }
        self.state
            .project
            .runtime
            .camera
            .keys
            .push(q0s_format::v2::CameraKey {
                frame: target,
                state,
                tween: Tween::None,
            });
        self.state
            .project
            .runtime
            .camera
            .keys
            .sort_by_key(|key| key.frame);
        if target >= old_count {
            if let Some(entry) = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == entry_id)
            {
                entry.frame_count = new_count;
            }
        }
        self.history.snapshot(&before);
        self.state.mark_dirty();
        self.session.current_frame = target;
        self.session.pending_timeline_frame = None;
        self.session.timeline_selection = None;
        self.session.camera_timeline_selection = Some(CameraTimelineSelection::single(target));
        self.session.timeline_layer_selection = None;
        self.session.selection = Selection::None;
        self.session.status = format!("inserted camera keyframe at frame {}", target + 1);
    }

    fn delete_camera_keyframe_at_current(&mut self) {
        let selection = self
            .session
            .camera_timeline_selection
            .unwrap_or_else(|| CameraTimelineSelection::single(self.session.current_frame));
        let first = selection.first_frame();
        let last = selection.last_frame();
        let removed_frames = self
            .state
            .project
            .runtime
            .camera
            .keys
            .iter()
            .filter(|key| (first..=last).contains(&key.frame))
            .map(|key| key.frame)
            .collect::<std::collections::BTreeSet<_>>();
        if removed_frames.is_empty() {
            self.session.status = "no camera keyframes in selection".to_string();
            return;
        }
        let before = self.state.project.clone();
        self.state
            .project
            .runtime
            .camera
            .keys
            .retain(|key| !removed_frames.contains(&key.frame));
        for key in &mut self.state.project.runtime.camera.keys {
            if key
                .tween
                .to_frame()
                .is_some_and(|frame| removed_frames.contains(&frame))
            {
                key.tween = Tween::None;
            }
        }
        self.history.snapshot(&before);
        self.state.mark_dirty();
        self.session.status = if removed_frames.len() == 1 {
            format!("removed camera keyframe at frame {}", first + 1)
        } else {
            format!("removed {} camera keyframes", removed_frames.len())
        };
    }

    fn toggle_camera_tween(&mut self) {
        if self.session.pending_timeline_frame.is_some() {
            self.session.status = "create the future frame before camera tweening".to_string();
            return;
        }
        let frame = self.session.current_frame;
        let mut ordered = self
            .state
            .project
            .runtime
            .camera
            .keys
            .iter()
            .map(|key| key.frame)
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        let Some(source_frame) = ordered
            .iter()
            .copied()
            .filter(|candidate| *candidate <= frame)
            .max()
        else {
            self.session.status = "camera tween needs a keyframe before this frame".to_string();
            return;
        };
        let Some(target_frame) = ordered
            .iter()
            .copied()
            .find(|candidate| *candidate > source_frame)
        else {
            self.session.status = "camera tween needs a later camera keyframe".to_string();
            return;
        };
        let before = self.state.project.clone();
        let Some(source) = self
            .state
            .project
            .runtime
            .camera
            .keys
            .iter_mut()
            .find(|key| key.frame == source_frame)
        else {
            return;
        };
        let removing = source.tween.to_frame() == Some(target_frame);
        source.tween = if removing {
            Tween::None
        } else {
            Tween::Linear {
                to_frame: target_frame,
            }
        };
        self.history.snapshot(&before);
        self.state.mark_dirty();
        self.session.status = if removing {
            "removed camera tween".to_string()
        } else {
            format!(
                "created camera tween from frame {} to {}",
                source_frame + 1,
                target_frame + 1
            )
        };
    }

    /// f5 / insert frame. add one hold frame after the selected cells on each
    /// selected layer; unselected rows keep their positions and lengths.
    fn insert_frame_smart(&mut self) {
        let q0rg_id = self.session.current_q0rg_id;
        let fallback_layer_id = self.session.current_layer_id;
        let has_explicit_multi_selection = self
            .session
            .timeline_cell_selection
            .as_ref()
            .is_some_and(|(selected_q0rg, cells)| *selected_q0rg == q0rg_id && !cells.is_empty())
            || self.session.timeline_selection.is_some_and(|selection| {
                selection.anchor_layer_id != selection.focus_layer_id
                    || selection.anchor_frame != selection.focus_frame
            });

        if !has_explicit_multi_selection && self.session.pending_timeline_frame.is_some() {
            let target = self
                .session
                .pending_timeline_frame
                .take()
                .unwrap_or_default();
            if self
                .state
                .project
                .layer_is_folder(q0rg_id, fallback_layer_id)
            {
                self.session.status = "folders cannot contain frames".to_string();
                return;
            }
            if self
                .state
                .project
                .layer_is_locked(q0rg_id, fallback_layer_id)
            {
                self.session.status = "locked timeline frames cannot be changed".to_string();
                return;
            }
            let before = self.state.project.clone();
            let Some(new_count) = crate::timeline_edit::extend_layer_through_frame(
                &mut self.state.project,
                q0rg_id,
                fallback_layer_id,
                target,
                false,
            ) else {
                self.session.status = "timeline is already at its maximum length".to_string();
                return;
            };
            if self.state.project != before {
                self.history.snapshot(&before);
                self.state.mark_dirty();
                self.textures.invalidate();
            }
            self.session.current_layer_id = fallback_layer_id;
            self.session.current_frame = target;
            self.session.timeline_selection =
                Some(TimelineSelection::single(fallback_layer_id, target));
            self.session.timeline_cell_selection = None;
            self.session.selection = Selection::None;
            self.session.status = format!(
                "extended selected layer through frame {} of {new_count}",
                target + 1
            );
            return;
        }

        let mut selected_cells = self
            .session
            .timeline_cell_selection
            .as_ref()
            .filter(|(selected_q0rg, cells)| *selected_q0rg == q0rg_id && !cells.is_empty())
            .map(|(_, cells)| cells.clone())
            .unwrap_or_default();
        if selected_cells.is_empty() {
            if let Some(selection) = self.session.timeline_selection {
                selected_cells = crate::timeline_edit::selected_frame_cells_including_virtual(
                    &self.state.project,
                    q0rg_id,
                    selection,
                )
                .unwrap_or_default();
            }
        }
        if selected_cells.is_empty() {
            selected_cells.push((fallback_layer_id, self.session.current_frame));
        }

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
        selected_cells.retain(|(layer_id, _)| {
            q0rg.layers.iter().any(|layer| {
                layer.layer_id == *layer_id
                    && !self.state.project.layer_is_folder(q0rg_id, *layer_id)
            })
        });
        selected_cells.sort_unstable();
        selected_cells.dedup();
        if selected_cells.is_empty() {
            self.session.status = "select existing frame cells before inserting a hold".to_string();
            return;
        }
        let selected_layers = selected_cells
            .iter()
            .map(|(layer_id, _)| *layer_id)
            .collect::<std::collections::BTreeSet<_>>();
        if selected_layers
            .iter()
            .any(|layer_id| self.state.project.layer_is_locked(q0rg_id, *layer_id))
        {
            self.session.status = "unlock selected layers before inserting hold frames".to_string();
            return;
        }
        if selected_cells.iter().any(|(layer_id, frame)| {
            crate::audio::audio_clip_at_frame(&self.state.project, q0rg_id, *layer_id, *frame)
                .is_some()
        }) {
            self.session.status = "audio frames cannot contain hold frames".to_string();
            return;
        }

        let before = self.state.project.clone();
        match crate::timeline_edit::insert_hold_frames_after_selected_cells(
            &mut self.state.project,
            q0rg_id,
            &selected_cells,
        ) {
            Some((new_count, inserted_cells)) if !inserted_cells.is_empty() => {
                self.history.snapshot(&before);
                self.state.mark_dirty();
                self.session.pending_timeline_frame = None;
                let active_inserted = inserted_cells
                    .iter()
                    .find(|(layer_id, _)| *layer_id == fallback_layer_id)
                    .copied()
                    .unwrap_or(inserted_cells[0]);
                self.session.current_layer_id = active_inserted.0;
                self.session.current_frame = active_inserted.1;
                if inserted_cells.len() == 1 {
                    self.session.timeline_selection = Some(TimelineSelection::single(
                        active_inserted.0,
                        active_inserted.1,
                    ));
                    self.session.timeline_cell_selection = None;
                } else {
                    self.session.timeline_selection = None;
                    self.session.timeline_cell_selection = Some((q0rg_id, inserted_cells.clone()));
                }
                self.session.selection = Selection::None;
                self.textures.invalidate();
                self.session.status = format!(
                    "inserted hold frames for selected cells on {} layer(s); timeline has {new_count} frames",
                    inserted_cells.len()
                );
            }
            Some(_) => {
                self.session.status = "no existing frame cells selected".to_string();
            }
            None => {
                self.session.status = "could not insert the selected hold frames".to_string();
            }
        }
    }
    pub(crate) fn audio_keyframe_at(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        frame: u16,
        continue_audio: bool,
    ) -> bool {
        if self.state.project.layer_is_locked(q0rg_id, layer_id) {
            return false;
        }
        self.history.snapshot(&self.state.project);
        let changed = crate::audio::keyframe_audio(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
            continue_audio,
        );
        if changed {
            self.state.mark_dirty();
            self.session.selection = Selection::None;
            self.session.status = if continue_audio {
                "audio split; source continues in the new clip"
            } else {
                "audio stops at the blank key"
            }
            .into();
            if self.session.playing {
                let _ = self.refresh_audio_playback();
            }
        }
        changed
    }

    pub(crate) fn scrub_audio_at(&mut self, frame: u16) {
        if self.session.playing
            || !self.session.audio_scrubbing
            || !crate::audio::has_audible_timeline_audio(
                &self.state.project,
                self.session.current_q0rg_id,
            )
        {
            return;
        }
        if self.audio_playback.is_none() {
            match crate::audio::EditorAudioPlayback::new() {
                Ok(audio) => self.audio_playback = Some(audio),
                Err(error) => {
                    self.session.status = error;
                    return;
                }
            }
        }
        if let Some(audio) = self.audio_playback.as_mut() {
            if let Err(error) =
                audio.scrub(&self.state.project, self.session.current_q0rg_id, frame)
            {
                self.session.status = error;
            }
        }
    }

    pub(crate) fn refresh_audio_playback(&mut self) -> Result<(), String> {
        let has_audio = crate::audio::has_audible_timeline_audio(
            &self.state.project,
            self.session.current_q0rg_id,
        );
        if !has_audio {
            if let Some(audio) = self.audio_playback.as_mut() {
                audio.stop();
            }
            return Ok(());
        }
        if self.audio_playback.is_none() {
            self.audio_playback = Some(crate::audio::EditorAudioPlayback::new()?);
        }
        self.audio_playback
            .as_mut()
            .expect("editor audio initialized")
            .sync(
                &self.state.project,
                self.session.current_q0rg_id,
                self.session.current_frame,
                self.session.playing,
            )
    }

    pub(crate) fn stop_audio_playback(&mut self) {
        if let Some(audio) = self.audio_playback.as_mut() {
            audio.stop();
        }
    }

    fn go_to_frame(&mut self, requested: u16) {
        let previous_frame = self.session.current_frame;
        self.session.pending_timeline_frame = None;
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
        if self.session.current_frame != previous_frame {
            self.scrub_audio_at(self.session.current_frame);
        }
    }

    /// F6 / Insert Keyframe with Flash-like playhead semantics.
    ///
    /// On a held frame, bake the visible state at the current playhead and
    /// stay there. On an existing keyframe, create the next keyframe to the
    /// right and move the playhead onto it.
    fn insert_keyframe_smart(&mut self) {
        let q0rg_id = self.session.current_q0rg_id;
        let selected_cells = self
            .session
            .timeline_cell_selection
            .as_ref()
            .filter(|(selected_q0rg, cells)| *selected_q0rg == q0rg_id && !cells.is_empty())
            .map(|(_, cells)| cells.clone())
            .or_else(|| {
                self.session.timeline_selection.and_then(|selection| {
                    (selection.anchor_layer_id != selection.focus_layer_id
                        || selection.anchor_frame != selection.focus_frame)
                        .then(|| {
                            crate::timeline_edit::selected_frame_cells_including_virtual(
                                &self.state.project,
                                q0rg_id,
                                selection,
                            )
                            .unwrap_or_default()
                        })
                })
            });
        if let Some(mut cells) = selected_cells {
            cells.sort_unstable();
            cells.dedup();
            if cells.is_empty() {
                self.session.status =
                    "select timeline cells before inserting keyframes".to_string();
                return;
            }
            let selected_layers = cells
                .iter()
                .map(|(layer_id, _)| *layer_id)
                .collect::<std::collections::BTreeSet<_>>();
            if selected_layers
                .iter()
                .any(|layer_id| self.state.project.layer_is_locked(q0rg_id, *layer_id))
            {
                self.session.status =
                    "unlock selected layers before inserting keyframes".to_string();
                return;
            }

            let before = self.state.project.clone();
            match crate::timeline_edit::materialize_keyframes_at_cells(
                &mut self.state.project,
                q0rg_id,
                &cells,
            ) {
                Some(created) if created > 0 => {
                    self.history.snapshot(&before);
                    self.state.mark_dirty();
                    let previous_frame = self.session.current_frame;
                    let focused_cell = self
                        .session
                        .timeline_selection
                        .map(|selection| (selection.focus_layer_id, selection.focus_frame))
                        .filter(|cell| cells.contains(cell))
                        .or_else(|| {
                            self.session
                                .pending_timeline_frame
                                .map(|frame| (self.session.current_layer_id, frame))
                                .filter(|cell| cells.contains(cell))
                        })
                        .or_else(|| {
                            cells
                                .iter()
                                .rev()
                                .find(|(layer_id, _)| *layer_id == self.session.current_layer_id)
                                .copied()
                        })
                        .or_else(|| cells.last().copied());
                    let mut selection_frames = std::collections::BTreeMap::new();
                    for (selected_layer, selected_frame) in &cells {
                        selection_frames
                            .entry(*selected_layer)
                            .and_modify(|nearest: &mut u16| {
                                if selected_frame.abs_diff(previous_frame)
                                    < nearest.abs_diff(previous_frame)
                                {
                                    *nearest = *selected_frame;
                                }
                            })
                            .or_insert(*selected_frame);
                    }
                    if let Some((focused_layer, focused_frame)) = focused_cell {
                        selection_frames.insert(focused_layer, focused_frame);
                    }
                    remap_selection_after_keyframes(
                        &before,
                        &self.state.project,
                        &mut self.session.selection,
                        &mut self.session.transform_pivot,
                        q0rg_id,
                        &selection_frames,
                    );
                    if let Some((layer_id, frame)) = focused_cell {
                        self.session.current_layer_id = layer_id;
                        self.session.current_frame = frame;
                    }
                    self.session.pending_timeline_frame = None;
                    self.textures.invalidate();
                    self.session.status = format!("inserted {created} selected keyframe(s)");
                }
                Some(_) => {
                    self.session.status =
                        "selected timeline cells are already keyframes".to_string();
                }
                None => {
                    self.session.status = "could not insert selected keyframes".to_string();
                }
            }
            return;
        }

        if let Some(target) = self.session.pending_timeline_frame.take() {
            let inserted = self.insert_keyframe_at(target);
            let target_exists = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == self.session.current_q0rg_id)
                .map(|q0rg| target < q0rg.frame_count)
                .unwrap_or(false);
            if inserted > 0 || target_exists {
                self.session.current_frame = target;
            }
            return;
        }
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
        if let Some(target) = self.session.pending_timeline_frame.take() {
            let inserted = self.insert_blank_keyframe_at(target);
            let target_exists = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == self.session.current_q0rg_id)
                .map(|q0rg| target < q0rg.frame_count)
                .unwrap_or(false);
            if inserted || target_exists {
                self.session.current_frame = target;
            }
            return;
        }
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
        if self.state.project.layer_is_locked(q0rg_id, layer_id) {
            self.session.status = "layer is locked".to_string();
            return false;
        }
        if crate::audio::audio_clip_at_frame(&self.state.project, q0rg_id, layer_id, frame)
            .is_some()
        {
            return self.audio_keyframe_at(q0rg_id, layer_id, frame, false);
        }
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

        let before = self.state.project.clone();
        if crate::timeline_edit::extend_layer_through_frame(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
            true,
        )
        .is_none()
        {
            self.session.status = "timeline is already at its maximum length".to_string();
            return false;
        }
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
            return false;
        };
        layer.ensure_explicit_keyframe(frame);
        self.history.snapshot(&before);
        self.state.mark_dirty();
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
        let selection_before = self.session.selection.clone();
        if self.state.project.layer_is_locked(q0rg_id, layer_id) {
            self.session.status = "layer is locked".to_string();
            return 0;
        }
        if crate::audio::audio_clip_at_frame(&self.state.project, q0rg_id, layer_id, frame)
            .is_some()
        {
            return usize::from(self.audio_keyframe_at(q0rg_id, layer_id, frame, true));
        }
        let active = match self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
        {
            Some(layer) => crate::timeline_edit::keyframe_source_states_at(layer, frame),
            None => Vec::new(),
        };
        if active.is_empty() {
            let inserted = self.insert_blank_keyframe_at(frame);
            if inserted {
                self.session.selection = selection_before;
            }
            return usize::from(inserted);
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
                .filter_map(|state| {
                    let src = layer.placements.get(state.index)?;
                    (src.frame != frame).then_some((
                        state.index,
                        Placement {
                            instance_id: src.instance_id,
                            frame,
                            target: src.target,
                            transform: state.transform,
                            tween: Tween::None,
                            fx: state.fx,
                        },
                    ))
                })
                .collect()
        };
        if insertions.is_empty() {
            self.session.status = format!("keyframes already exist at frame {}", frame + 1);
            return 0;
        }

        let before = self.state.project.clone();
        if crate::timeline_edit::extend_layer_through_frame(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
            true,
        )
        .is_none()
        {
            self.session.status = "timeline is already at its maximum length".to_string();
            return 0;
        }
        if let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        {
            if let Some(layer) = q0rg.layers.iter_mut().find(|l| l.layer_id == layer_id) {
                for (idx, _) in &insertions {
                    if let Some(source) = layer.placements.get_mut(*idx) {
                        if source
                            .tween
                            .to_frame()
                            .is_some_and(|to_frame| source.frame < frame && frame <= to_frame)
                        {
                            source.tween = source.tween.with_to_frame(frame);
                        }
                    }
                }
                for (idx, placement) in insertions.iter().rev() {
                    let insert_at = idx.saturating_add(1).min(layer.placements.len());
                    layer.placements.insert(insert_at, placement.clone());
                }
            }
        }
        self.history.snapshot(&before);
        let new_count = insertions.len();
        self.state.mark_dirty();
        remap_selection_after_keyframes(
            &before,
            &self.state.project,
            &mut self.session.selection,
            &mut self.session.transform_pivot,
            q0rg_id,
            &std::collections::BTreeMap::from([(layer_id, frame)]),
        );
        self.session.status = format!("inserted {new_count} keyframe(s) at frame {}", frame + 1);
        new_count
    }

    /// Demote selected keyframes back to ordinary held frames. This is distinct
    /// from Delete, which clears contents and intentionally leaves blank keys.
    fn clear_keyframe(&mut self) {
        if self.session.pending_timeline_frame.is_some() {
            self.session.status =
                "future frame is not created; press F5, F6, or F7 first".to_string();
            return;
        }
        let selection = self.session.timeline_selection.unwrap_or_else(|| {
            TimelineSelection::single(self.session.current_layer_id, self.session.current_frame)
        });
        if self.timeline_selection_touches_locked_layer(selection) {
            self.session.status = "locked keyframes cannot be cleared".to_string();
            return;
        }
        let before = self.state.project.clone();
        match crate::timeline_edit::remove_selected_keyframes(
            &mut self.state.project,
            self.session.current_q0rg_id,
            selection,
        ) {
            Some(removed) if removed > 0 => {
                self.history.snapshot(&before);
                self.state.mark_dirty();
                self.session.selection = Selection::None;
                self.textures.invalidate();
                self.session.status = format!("cleared {removed} keyframe(s)");
            }
            Some(_) => {
                self.session.status =
                    "no removable keyframes selected; frame 1 must remain a keyframe".to_string();
            }
            None => {
                self.session.status = "clear keyframe: selection not found".to_string();
            }
        }
    }

    /// Remove the selected columns of time from the whole q0rg. Everything to
    /// the right shifts left, including blank keys and tween destinations.
    fn remove_frame(&mut self) {
        if self.current_q0rg_has_locked_artwork_layer() {
            self.session.status = "unlock layers before removing a global frame column".to_string();
            return;
        }
        if self.session.pending_timeline_frame.is_some() {
            self.session.status =
                "future frame is not created; press F5, F6, or F7 first".to_string();
            return;
        }
        let selection = self.session.timeline_selection.unwrap_or_else(|| {
            TimelineSelection::single(self.session.current_layer_id, self.session.current_frame)
        });
        let first_frame = selection.anchor_frame.min(selection.focus_frame);
        let anchor_layer = selection.anchor_layer_id;
        let before = self.state.project.clone();
        match crate::timeline_edit::remove_selected_frame_columns(
            &mut self.state.project,
            self.session.current_q0rg_id,
            selection,
        ) {
            Some(removed) if removed > 0 => {
                self.history.snapshot(&before);
                let new_count = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == self.session.current_q0rg_id)
                    .map(|q0rg| q0rg.frame_count)
                    .unwrap_or(1)
                    .max(1);
                self.session.current_layer_id = anchor_layer;
                self.session.current_frame = first_frame.min(new_count - 1);
                self.session.pending_timeline_frame = None;
                self.session.timeline_selection = Some(TimelineSelection::single(
                    anchor_layer,
                    self.session.current_frame,
                ));
                self.session.selection = Selection::None;
                self.state.mark_dirty();
                self.textures.invalidate();
                self.session.status = format!("removed {removed} frame(s), {new_count} total");
            }
            Some(_) => {
                self.session.status = "can't remove the only timeline frame".to_string();
            }
            None => {
                self.session.status = "remove frames: selection not found".to_string();
            }
        }
    }

    /// Create motion tweens from non-keyframe timeline cells. A valid cell must
    /// sit strictly between two non-empty keyframes on the same drawable layer.
    /// No destination keyframe is ever invented implicitly.
    fn toggle_motion_tween(&mut self) {
        if self.session.pending_timeline_frame.is_some() {
            self.session.tween_warning = Some(
                "This future frame does not exist yet. Create it with F5, F6, or F7 first."
                    .to_string(),
            );
            return;
        }

        let selection = self.session.timeline_selection.unwrap_or_else(|| {
            TimelineSelection::single(self.session.current_layer_id, self.session.current_frame)
        });
        if self.timeline_selection_touches_locked_layer(selection) {
            self.session.tween_warning =
                Some("Unlock the selected layer before editing its tween.".to_string());
            self.session.status = "layer is locked".to_string();
            return;
        }
        if self.timeline_selection_touches_audio(selection) {
            let message = "audio frames cannot contain motion tweens".to_string();
            self.session.tween_warning = Some(message.clone());
            self.session.status = message;
            return;
        }
        let q0rg_id = self.session.current_q0rg_id;
        let visible_layer_ids =
            crate::panels::timeline::visible_layer_ids(&self.state.project, q0rg_id);
        let before = self.state.project.clone();

        match crate::easing::create_tweens_for_selection(
            &mut self.state.project,
            q0rg_id,
            selection,
            &visible_layer_ids,
        ) {
            Ok(targets) => {
                self.history.snapshot(&before);
                self.state.mark_dirty();
                self.session.selection = Selection::None;
                self.session.status = if targets.len() == 1 {
                    "created motion tween".to_string()
                } else {
                    format!("created {} motion tween tracks", targets.len())
                };
            }
            Err(error) => {
                self.session.tween_warning = Some(error.message().to_string());
                self.session.status = error.message().to_string();
            }
        }
    }

    fn render_project_tree_dialog(&mut self, ctx: &Context) {
        if !self.project_active
            || self.show_home
            || self.active_document_is_q0lang()
            || ctx.wants_keyboard_input()
        {
            return;
        }
        let mut open = self.show_project_tree;
        egui::Window::new("Project")
            .id(egui::Id::new("q0editor_project_tree_window"))
            .open(&mut open)
            .default_size([560.0, 600.0])
            .min_size([420.0, 360.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                panels::project_tree::render(self, ui);
            });
        self.show_project_tree = open;
    }

    fn render_credits_dialog(&mut self, ctx: &Context) {
        let mut open = self.session.show_credits;
        let screen = ctx.screen_rect();
        let initial_pos = egui::pos2(
            (screen.center().x - 380.0).max(16.0),
            (screen.center().y - 335.0).max(40.0),
        );
        egui::Window::new("Credits")
            .open(&mut open)
            .default_pos(initial_pos)
            .default_size([760.0, 590.0])
            .min_size([500.0, 440.0])
            .max_height((screen.height() - 90.0).max(360.0))
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
            .default_size([640.0, 580.0])
            .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
            .default_pos(ctx.screen_rect().center() - egui::vec2(336.0, 330.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.label(egui::RichText::new("Make it yours.").size(27.0).strong());
                ui.label(egui::RichText::new("Appearance, canvas and editor preferences")
                    .color(self.settings.theme.text_dim.to_color32()));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.label("Keyboard shortcuts");
                    if ui.button("Manage shortcuts...").clicked() {
                        self.session.show_shortcuts = true;
                    }
                });
                ui.add_space(10.0);
                crate::theme::section_heading(ui, "Appearance", "Choose a palette. Every panel follows your theme.");
                ui.horizontal_wrapped(|ui| {
                    for preset in Theme::PRESETS {
                        let selected = self.settings.theme.accent == preset.accent.into()
                            && self.settings.theme.window == preset.window.into()
                            && self.settings.theme.panel == preset.panel.into();
                        if crate::theme::preset_button(ui, preset, selected).clicked() {
                            self.settings.theme = Theme::from_builtin(preset);
                            self.settings.theme.apply(ctx);
                            self.settings.save();
                            self.session.status = format!("theme: {}", preset.name);
                        }
                    }
                });
                ui.add_space(12.0);
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
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Choose canvas photo...").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                                    .pick_file()
                                {
                                    match crate::bitmap_import::decode_bitmap_path(&path, u16::MAX) {
                                        Ok(_) => {
                                            t.canvas_background_image = Some(path.clone());
                                            self.session.status = format!(
                                                "canvas photo: {}",
                                                path.file_name()
                                                    .and_then(|name| name.to_str())
                                                    .unwrap_or("image")
                                            );
                                        }
                                        Err(error) => {
                                            self.session.status =
                                                format!("canvas photo failed: {error}");
                                        }
                                    }
                                }
                            }
                            if let Some(path) = t.canvas_background_image.as_ref() {
                                ui.label(
                                    path.file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("selected image"),
                                );
                                if ui.button("Clear photo").clicked() {
                                    t.canvas_background_image = None;
                                    t.canvas_background_darkening = 0;
                                }
                            }
                        });
                        ui.add(
                            egui::Slider::new(&mut t.canvas_background_darkening, 0..=100)
                                .text("Photo darkening")
                                .suffix("%"),
                        );
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
                    let q0lang_association_state =
                        match crate::assoc::is_q0lang_registered_for_current_user() {
                            Ok(true) => "active for this q0editor",
                            Ok(false) => "not active",
                            Err(_) => "status unavailable",
                        };
                    ui.label(
                        egui::RichText::new(format!(
                            ".q0l / .q0lang source files: {q0lang_association_state}. Both extensions are the same q0lang source format."
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
                    ui.horizontal(|ui| {
                        if ui.button("Register .q0l + .q0lang").clicked() {
                            match crate::assoc::register_q0lang_for_current_user() {
                                Ok(()) => {
                                    self.session.status =
                                        ".q0l and .q0lang files now open with q0editor".to_string()
                                }
                                Err(error) => {
                                    self.session.status =
                                        format!("q0lang association failed: {error}")
                                }
                            }
                        }
                        if ui.button("Remove q0lang associations").clicked() {
                            match crate::assoc::unregister_q0lang_for_current_user() {
                                Ok(()) => {
                                    self.session.status =
                                        ".q0l and .q0lang associations removed".to_string()
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
        crate::ui::hidden_palette_note(
            ctx,
            self.session.show_settings,
            self.settings.theme.accent.to_color32(),
        );
    }

    fn render_shortcuts_dialog(&mut self, ctx: &Context) {
        use crate::hotkeys::{ShortcutChord, ShortcutCommand};

        let mut open = true;
        let mut close_requested = false;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .default_size([560.0, 540.0])
            .default_pos(ctx.screen_rect().center() - egui::vec2(280.0, 270.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("Choose a command, then press the key combination you want to use.");
                ui.label(
                    egui::RichText::new("Assigning a shortcut moves it from its previous command.")
                        .small()
                        .color(self.settings.theme.text_dim.to_color32()),
                );
                ui.horizontal(|ui| {
                    if ui.button("Reset to factory defaults").clicked() {
                        self.settings.hotkeys.reset();
                        self.settings.save();
                        self.session.shortcut_capture = None;
                        self.session.shortcut_feedback = Some("Factory shortcuts restored".into());
                    }
                    if ui.button("Close").clicked() {
                        close_requested = true;
                    }
                });
                if let Some(feedback) = &self.session.shortcut_feedback {
                    ui.label(
                        egui::RichText::new(feedback)
                            .small()
                            .color(self.settings.theme.accent.to_color32()),
                    );
                }
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_source("keyboard_shortcuts_list")
                    .max_height(430.0)
                    .show(ui, |ui| {
                        egui::Grid::new("keyboard_shortcuts_grid")
                            .num_columns(3)
                            .spacing([12.0, 7.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for command in ShortcutCommand::ALL.iter().copied() {
                                    ui.label(command.label());
                                    if self.session.shortcut_capture == Some(command) {
                                        ui.colored_label(
                                            self.settings.theme.accent.to_color32(),
                                            "Press a key... (Esc cancels)",
                                        );
                                    } else {
                                        ui.label(self.settings.hotkeys.display(command));
                                    }
                                    if self.session.shortcut_capture == Some(command) {
                                        if ui.small_button("Cancel").clicked() {
                                            self.session.shortcut_capture = None;
                                        }
                                    } else {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Assign").clicked() {
                                                self.session.shortcut_capture = Some(command);
                                                self.session.shortcut_feedback = None;
                                            }
                                            if ui.small_button("Clear").clicked() {
                                                self.settings.hotkeys.set(command, Vec::new());
                                                self.settings.save();
                                                self.session.shortcut_feedback = Some(format!(
                                                    "{} is now unassigned",
                                                    command.label()
                                                ));
                                            }
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });

        if !open || close_requested {
            self.session.show_shortcuts = false;
            self.session.shortcut_capture = None;
            return;
        }

        let capture = if self.session.show_shortcuts {
            ctx.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    // egui-winit reports these native clipboard combinations
                    // instead of Key(X/C/V), including while a shortcut is captured.
                    egui::Event::Cut => Some((egui::Key::X, input.modifiers)),
                    egui::Event::Copy => Some((egui::Key::C, input.modifiers)),
                    egui::Event::Paste(_) => Some((egui::Key::V, input.modifiers)),
                    _ => None,
                })
            })
        } else {
            None
        };
        if let (Some(command), Some((key, modifiers))) = (self.session.shortcut_capture, capture) {
            if key == egui::Key::Escape && !modifiers.command && !modifiers.shift && !modifiers.alt
            {
                self.session.shortcut_capture = None;
                self.session.shortcut_feedback = Some("Shortcut assignment cancelled".into());
            } else if let Some(chord) = ShortcutChord::from_modifiers(key, modifiers) {
                let moved = self.settings.hotkeys.assign(command, chord);
                self.settings.save();
                self.session.shortcut_capture = None;
                self.session.shortcut_feedback = Some(if moved.is_empty() {
                    format!("{} assigned to {}", command.label(), chord.display())
                } else {
                    format!(
                        "{} assigned to {}; replaced {}",
                        command.label(),
                        chord.display(),
                        moved
                            .iter()
                            .map(|item| item.label())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                });
            } else {
                self.session.shortcut_feedback =
                    Some("That key cannot be used for a shortcut".into());
            }
        }
    }

    fn save_to(&mut self, path: &std::path::Path) -> bool {
        if self.another_project_tab_uses_path(path) {
            self.session.status =
                "save failed: this q1s file is already open in another tab".to_string();
            return false;
        }
        let mut project_to_save = self.state.project.clone();
        self.rebase_project_dependencies_for_save(&mut project_to_save, path);
        match file_io::save_project(path, &project_to_save) {
            Ok(_) => {
                self.state.project = project_to_save;
                self.state.mark_dirty();
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

    fn start_fla_import(
        &mut self,
        ctx: &Context,
        path: &std::path::Path,
        purpose: FlaImportPurpose,
    ) -> bool {
        if self.fla_import_job.is_some() {
            self.session.status = "another fla import is already running".to_string();
            return false;
        }
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before opening an fla project"
                    .to_string();
            return false;
        }

        let worker_path = path.to_path_buf();
        let started_at = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let thread_path = worker_path.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("q0editor-fla-import".to_string())
            .spawn(move || {
                let result = q0fla::import_path(&thread_path).map_err(|error| error.to_string());
                let _ = sender.send(result);
            })
        {
            self.session.status = format!("could not start fla import: {error}");
            return false;
        }

        self.fla_import_job = Some(FlaImportJob {
            path: worker_path,
            purpose,
            receiver,
            completed: None,
            started_at,
        });
        self.session.status = format!("importing {} in the background...", path.display());
        ctx.request_repaint_after(Duration::from_millis(50));
        true
    }

    fn poll_fla_import(&mut self, ctx: &Context) {
        let completion = self.fla_import_job.as_ref().and_then(|job| {
            if job.completed.is_some() {
                return None;
            }
            let result = match job.receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    Err("fla import worker stopped before returning a result".to_string())
                }
            };
            Some((result, job.started_at.elapsed()))
        });
        if let Some((result, elapsed)) = completion {
            if let Some(job) = self.fla_import_job.as_mut() {
                job.completed = Some(result);
            }
            self.perf.record(crate::perf::PerfSpan::FlaImport, elapsed);
        }

        let ready = self
            .fla_import_job
            .as_ref()
            .is_some_and(|job| job.completed.is_some());
        if !ready || self.active_workspace_busy() {
            if ready {
                self.session.status =
                    "fla import finished; waiting for the active import or export to finish"
                        .to_string();
            }
            if self.fla_import_job.is_some() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        }

        let job = self
            .fla_import_job
            .take()
            .expect("completed fla import must still have a job");
        let result = job
            .completed
            .expect("completed fla import must contain its result");
        match result {
            Ok(imported) => match job.purpose {
                FlaImportPurpose::Open => {
                    let summary = imported.report.summary();
                    let mut workspace = self.workspace_for_project(imported.project, None);
                    // Opening FLA/XFL creates an unsaved q1s project; it never
                    // writes a converted project beside the source implicitly.
                    workspace.state.mark_dirty();
                    if !self.activate_new_workspace(workspace) {
                        return;
                    }
                    self.new_project_dialog = None;
                    self.session.status = format!(
                        "q0fla imported {} as an unsaved q1s project; {summary}",
                        job.path.display()
                    );
                }
                FlaImportPurpose::Convert => {
                    if let Some(destination) = file_io::pick_q0fla_save_path(&job.path) {
                        match file_io::save_project(&destination, &imported.project) {
                            Ok(()) => {
                                self.session.status = format!(
                                    "q0fla converted {} -> {}; {}",
                                    job.path.display(),
                                    destination.display(),
                                    imported.report.summary()
                                );
                            }
                            Err(error) => {
                                self.session.status = format!("q0fla save failed: {error}");
                            }
                        }
                    }
                }
            },
            Err(error) => {
                self.session.status = format!("q0fla import failed: {error}");
            }
        }
    }

    fn open_from_path(&mut self, ctx: &Context, path: &std::path::Path) -> bool {
        if let Some(tab_id) = self.find_document_tab_by_path(path) {
            self.switch_to_document_tab(tab_id);
            self.session.status = format!("already open: {}", path.display());
            return true;
        }

        if file_io::is_q0lang_path(path) {
            match file_io::load_q0lang_document(path) {
                Ok(text) => {
                    let document = Q0langDocument {
                        path: path.to_path_buf(),
                        text,
                        dirty: false,
                        search: String::new(),
                        replace: String::new(),
                        show_find: false,
                        show_replace: false,
                    };
                    if !self.activate_q0lang_document(document) {
                        return false;
                    }
                    self.settings.push_recent(path);
                    self.session.status = match self.settings.try_save() {
                        Ok(()) => format!("opened {}", path.display()),
                        Err(error) => format!(
                            "opened {}; recent list was not saved: {error}",
                            path.display()
                        ),
                    };
                    return true;
                }
                Err(error) => {
                    self.session.status = format!("open failed: {error}");
                    return false;
                }
            }
        }

        if q0fla::is_import_path(path) {
            return self.start_fla_import(ctx, path, FlaImportPurpose::Open);
        }

        let is_project = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(file_io::Q1S_EXTENSION));
        if !is_project {
            self.session.status = format!(
                "open failed: unsupported document type {} (expected .q1s, .q0l or .q0lang)",
                path.display()
            );
            return false;
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
            self.session.status =
                "drop a .q1s, .fla, .xfl, .q0l or .q0lang document from Explorer".to_string();
            return;
        };
        let is_project = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(file_io::Q1S_EXTENSION));
        if is_project || q0fla::is_import_path(path) || file_io::is_q0lang_path(path) {
            self.queue(Action::OpenProjectFromPath(path.clone()));
        } else if self.show_home || !self.project_active {
            self.session.status = "open or create a project before importing media".to_string();
        } else if crate::bitmap_import::is_supported_bitmap_path(path)
            || file_io::is_supported_video_import_path(path)
            || crate::audio::is_supported_audio_path(path)
        {
            self.queue(Action::ImportMediaFromPath(path.clone()));
        } else {
            self.session.status = format!(
                "unsupported dropped file: {} (expected .q1s, .fla, .xfl, .q0l, .q0lang, png, jpg, webp, wav, mp3, ogg, flac, mp4 or q0v)",
                path.display()
            );
        }
    }

    fn save_current_project(&mut self) -> bool {
        if let Some(path) = self.state.file_path.clone() {
            return self.save_to(&path);
        }

        let suggested = PathBuf::from(unsaved_project_file_name(&self.state.project.meta.name));
        let Some(path) = file_io::pick_save_path(Some(&suggested)) else {
            return false;
        };
        self.save_to(&path)
    }

    fn save_q0lang_to(&mut self, path: &std::path::Path) -> bool {
        if !file_io::is_q0lang_path(path) {
            self.session.status = "save failed: q0lang source must use .q0l or .q0lang".to_string();
            return false;
        }
        if self.another_document_tab_uses_path(path) {
            self.session.status =
                "save failed: this file is already open in another tab".to_string();
            return false;
        }
        let Some(text) = self
            .active_q0lang_document()
            .map(|document| document.text.clone())
        else {
            return false;
        };
        match file_io::save_q0lang_document(path, &text) {
            Ok(()) => {
                if let Some(document) = self.active_q0lang_document_mut() {
                    document.path = path.to_path_buf();
                    document.dirty = false;
                }
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
            Err(error) => {
                self.session.status = format!("save failed: {error}");
                false
            }
        }
    }

    fn save_current_q0lang(&mut self) -> bool {
        let Some(path) = self
            .active_q0lang_document()
            .map(|document| document.path.clone())
        else {
            return false;
        };
        self.save_q0lang_to(&path)
    }

    fn save_current_document(&mut self) -> bool {
        if self.active_document_is_q0lang() {
            self.save_current_q0lang()
        } else if self.project_active {
            self.save_current_project()
        } else {
            false
        }
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
            .default_width(510.0)
            .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Every story starts with a stage.")
                        .strong()
                        .size(24.0),
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
                        [410.0, 32.0],
                        egui::TextEdit::singleline(&mut draft.name).hint_text("untitled"),
                    );
                });

                ui.add_space(8.0);
                crate::theme::section_heading(ui, "Stage preset", "Pick a format, then fine-tune the dimensions below.");
                ui.horizontal_wrapped(|ui| {
                    for (name, width, height) in [
                        ("Classic", 640, 480), ("HD", 1280, 720),
                        ("Full HD", 1920, 1080), ("Portrait", 1080, 1920),
                    ] {
                        let selected = draft.width == width && draft.height == height;
                        if ui.add_sized([117.0, 54.0], egui::Button::new(
                            format!("{name}\n{width} × {height}")).selected(selected).rounding(6.0)).clicked() {
                            draft.width = width;
                            draft.height = height;
                        }
                    }
                });
                ui.add_space(12.0);
                ui.add_space(6.0);
                egui::Grid::new("new_project_settings_grid")
                    .num_columns(4)
                    .spacing([18.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Width");
                        ui.add(
                            egui::DragValue::new(&mut draft.width)
                                .clamp_range(1..=8192)
                                .suffix(" px"),
                        );

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
                    if crate::theme::primary_button(ui, "Create project", self.settings.theme.accent.to_color32()).clicked() {
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
            Action::CloseDocumentTab(_) => "close this document tab",
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
            if self.save_current_document() {
                if let Some(action) = self.unsaved_action.take() {
                    self.handle(ctx, action);
                }
            }
        } else if discard {
            if let Some(action) = self.unsaved_action.take() {
                match action {
                    Action::CloseDocumentTab(_) | Action::Exit => {
                        if self.project_active {
                            self.state.dirty = false;
                        } else if let Some(document) = self.active_q0lang_document_mut() {
                            document.dirty = false;
                        }
                        self.handle(ctx, action);
                    }
                    _ => {}
                }
            }
        } else if cancel {
            self.unsaved_action = None;
        }
    }

    fn duplicate_symbol_instance(&mut self, q0rg_id: u16, layer_id: u16, placement_idx: usize) {
        if !self.project_active || self.show_home || self.active_document_is_q0lang() {
            return;
        }
        if self.state.project.layer_is_locked(q0rg_id, layer_id) {
            self.session.status = "layer is locked".to_string();
            return;
        }
        let frame = self.session.current_frame;
        let source_id = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| {
                if !q0s_format::raster::active_placement_states_at(layer, frame)
                    .iter()
                    .any(|state| state.index == placement_idx)
                {
                    return None;
                }
                match layer.placements.get(placement_idx)?.target {
                    Target::Q0rg(id) => Some(id),
                    Target::Asset(_) => None,
                }
            });
        let Some(source_id) = source_id else {
            self.session.status = "select a symbol on this frame first".to_string();
            return;
        };
        let (mut candidate, copied_id) =
            match crate::library_transfer::duplicate_symbol(&self.state.project, source_id) {
                Ok(result) => result,
                Err(error) => {
                    self.session.status = error;
                    return;
                }
            };
        let Some(new_idx) = crate::tools::materialize_placement_keyframe_for_edit(
            &mut candidate,
            q0rg_id,
            layer_id,
            placement_idx,
            frame,
        ) else {
            self.session.status =
                "could not create a keyframe for the independent symbol".to_string();
            return;
        };
        let Some(placement) = candidate
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter_mut().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get_mut(new_idx))
        else {
            return;
        };
        placement.target = Target::Q0rg(copied_id);
        if let Err(error) = q0s_format::v2::validate(&candidate) {
            self.session.status = format!("could not duplicate symbol: {error}");
            return;
        }
        let name = candidate
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == copied_id)
            .map(|q| q.name.clone())
            .unwrap_or_default();
        self.history.snapshot(&self.state.project);
        self.state.project = candidate;
        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx: new_idx,
        };
        if let Some(tab_id) = self.active_project_tab_id() {
            let selection = LibrarySelection {
                tab_id,
                item: LibraryItem::Q0rg(copied_id),
            };
            self.library_selection = Some(selection);
            self.library_multi_selection = vec![selection];
            self.library_selection_anchor = Some(selection);
        }
        self.session.status = format!("independent symbol: {name}");
    }

    pub(crate) fn open_replace_q0rg_dialog(
        &mut self,
        parent_q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
    ) {
        if !self.project_active || self.show_home || self.active_document_is_q0lang() {
            return;
        }
        if self.state.project.layer_is_locked(parent_q0rg_id, layer_id) {
            self.session.status = "layer is locked".to_string();
            return;
        }

        let frame = self.session.current_frame;
        let selected = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| {
                let placement = layer.placements.get(placement_idx)?;
                let child_id = match placement.target {
                    Target::Q0rg(id) => id,
                    Target::Asset(_) => return None,
                };
                q0s_format::raster::active_placement_states_at(layer, frame)
                    .iter()
                    .any(|active| active.index == placement_idx)
                    .then_some((placement.instance_id, child_id))
            });
        let Some((instance_id, source_q0rg_id)) = selected else {
            self.session.status = "selected symbol is no longer on this frame".to_string();
            return;
        };
        let source_q0rg_name = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == source_q0rg_id)
            .map(|q0rg| q0rg.name.clone())
            .unwrap_or_else(|| format!("q0rg {source_q0rg_id}"));
        let source_folder = self
            .state
            .project
            .runtime
            .library_item_folders
            .get(&q0s_format::v2::LibraryItemReference::Q0rg(source_q0rg_id))
            .cloned();

        self.pending_q0rg_replacement = Some(ReplaceQ0rgDialog {
            tab_id: self.active_project_tab_id(),
            parent_q0rg_id,
            layer_id,
            placement_idx,
            instance_id,
            source_q0rg_id,
            source_q0rg_name,
            source_folder,
            frame,
            search: String::new(),
            scroll_to_source: true,
        });
    }

    fn replace_q0rg_instance(&mut self, dialog: &ReplaceQ0rgDialog, replacement_id: u16) {
        if !self.project_active
            || self.show_home
            || self.active_document_is_q0lang()
            || self.active_project_tab_id() != dialog.tab_id
        {
            return;
        }
        if self
            .state
            .project
            .layer_is_locked(dialog.parent_q0rg_id, dialog.layer_id)
        {
            self.pending_q0rg_replacement = None;
            self.session.status = "layer is locked".to_string();
            return;
        }
        if replacement_id == dialog.source_q0rg_id
            || !self
                .state
                .project
                .q0rgs
                .iter()
                .any(|q0rg| q0rg.q0rg_id == replacement_id)
            || self.would_create_q0rg_cycle(dialog.parent_q0rg_id, replacement_id)
        {
            self.pending_q0rg_replacement = None;
            self.session.status = "that symbol cannot replace this instance".to_string();
            return;
        }

        let resolved = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == dialog.parent_q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == dialog.layer_id)
            })
            .and_then(|layer| {
                let active = q0s_format::raster::active_placement_states_at(layer, dialog.frame);
                let matches_source = |index: usize| {
                    layer.placements.get(index).is_some_and(|placement| {
                        placement.target == Target::Q0rg(dialog.source_q0rg_id)
                            && (dialog.instance_id == 0
                                || placement.instance_id == dialog.instance_id)
                    }) && active.iter().any(|state| state.index == index)
                };
                let placement_idx = if matches_source(dialog.placement_idx) {
                    Some(dialog.placement_idx)
                } else {
                    let matches = active
                        .iter()
                        .filter(|state| matches_source(state.index))
                        .map(|state| state.index)
                        .collect::<Vec<_>>();
                    if matches.len() == 1 {
                        Some(matches[0])
                    } else {
                        None
                    }
                }?;
                let state = active
                    .iter()
                    .find(|state| state.index == placement_idx)
                    .copied()?;
                let source_frame = layer.placements.get(placement_idx)?.frame;
                Some((placement_idx, source_frame, state))
            });
        let Some((placement_idx, source_frame, active)) = resolved else {
            self.pending_q0rg_replacement = None;
            self.session.status = "selected symbol changed before replacement".to_string();
            return;
        };

        let mut fx = active.fx;
        let replacement_max_frame = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == replacement_id)
            .map(|q0rg| q0rg.frame_count.max(1) - 1)
            .unwrap_or(0);
        fx.q0rg_playback.first_frame = fx.q0rg_playback.first_frame.min(replacement_max_frame);
        fx.q0rg_playback.last_frame = fx.q0rg_playback.last_frame.map(|last| {
            last.min(replacement_max_frame)
                .max(fx.q0rg_playback.first_frame)
        });
        fx.q0rg_playback.single_frame = fx.q0rg_playback.single_frame.min(replacement_max_frame);

        let old_child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
            &self.state.project,
            dialog.source_q0rg_id,
            dialog.frame,
            source_frame,
            active.fx.q0rg_playback,
        );
        let new_child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
            &self.state.project,
            replacement_id,
            dialog.frame,
            dialog.frame,
            fx.q0rg_playback,
        );
        let old_bounds = crate::render::q0rg_frame_bbox(
            &self.state.project,
            dialog.source_q0rg_id,
            old_child_frame,
        );
        let new_bounds =
            crate::render::q0rg_frame_bbox(&self.state.project, replacement_id, new_child_frame);
        let mut transform = active.transform;
        if let (Some(old_bounds), Some(new_bounds)) = (old_bounds, new_bounds) {
            let old_size = transformed_bbox_size(old_bounds, transform);
            let new_size = transformed_bbox_size(new_bounds, transform);
            let old_major = old_size.0.max(old_size.1);
            let new_major = new_size.0.max(new_size.1);
            let ratio = old_major / new_major;
            if old_major.is_finite()
                && new_major.is_finite()
                && old_major > 0.0
                && new_major > 0.0
                && ratio.is_finite()
                && ratio > 0.0
                && (transform.sx * ratio).is_finite()
                && (transform.sy * ratio).is_finite()
            {
                let old_center = Affine::from_transform(transform).apply(bbox_center(old_bounds));
                transform.sx *= ratio;
                transform.sy *= ratio;
                let new_center = Affine::from_transform(transform).apply(bbox_center(new_bounds));
                transform.tx += old_center.x - new_center.x;
                transform.ty += old_center.y - new_center.y;
            }
        }

        self.history.snapshot(&self.state.project);
        let Some(new_idx) = crate::tools::materialize_placement_keyframe_for_edit(
            &mut self.state.project,
            dialog.parent_q0rg_id,
            dialog.layer_id,
            placement_idx,
            dialog.frame,
        ) else {
            self.pending_q0rg_replacement = None;
            self.session.status = "could not create a keyframe for this replacement".to_string();
            return;
        };
        let Some(placement) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == dialog.parent_q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == dialog.layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(new_idx))
        else {
            self.pending_q0rg_replacement = None;
            self.session.status = "could not apply the symbol replacement".to_string();
            return;
        };
        placement.target = Target::Q0rg(replacement_id);
        placement.transform = transform;
        placement.fx = fx;

        self.state.mark_dirty();
        self.textures.invalidate();
        self.session.selection = Selection::Placement {
            q0rg_id: dialog.parent_q0rg_id,
            layer_id: dialog.layer_id,
            placement_idx: new_idx,
        };
        let replacement_name = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == replacement_id)
            .map(|q0rg| q0rg.name.as_str())
            .unwrap_or("q0rg");
        self.session.status = format!(
            "replaced with {replacement_name} on frame {}",
            dialog.frame + 1
        );
        self.pending_q0rg_replacement = None;
    }

    fn render_replace_q0rg_dialog(&mut self, ctx: &Context) {
        let Some(dialog) = self.pending_q0rg_replacement.clone() else {
            return;
        };
        if self.active_project_tab_id() != dialog.tab_id {
            return;
        }
        if !self
            .state
            .project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == dialog.source_q0rg_id)
        {
            self.pending_q0rg_replacement = None;
            return;
        }

        let mut candidates = self
            .state
            .project
            .q0rgs
            .iter()
            .filter(|q0rg| !self.would_create_q0rg_cycle(dialog.parent_q0rg_id, q0rg.q0rg_id))
            .map(|q0rg| {
                (
                    q0rg.q0rg_id,
                    q0rg.name.clone(),
                    self.state
                        .project
                        .runtime
                        .library_item_folders
                        .get(&q0s_format::v2::LibraryItemReference::Q0rg(q0rg.q0rg_id))
                        .cloned(),
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let left_in_source_folder = left.2.as_deref() == dialog.source_folder.as_deref();
            let right_in_source_folder = right.2.as_deref() == dialog.source_folder.as_deref();
            right_in_source_folder
                .cmp(&left_in_source_folder)
                .then_with(|| {
                    left.2
                        .as_deref()
                        .unwrap_or("")
                        .cmp(right.2.as_deref().unwrap_or(""))
                })
                .then_with(|| {
                    left.1
                        .to_lowercase()
                        .cmp(&right.1.to_lowercase())
                        .then_with(|| left.0.cmp(&right.0))
                })
        });

        let project = &self.state.project;
        let textures = &mut self.textures;
        let render_revision = (dialog.tab_id.unwrap_or(0), self.state.render_revision());
        let preview_light = self.settings.theme.library_preview_light.to_color32();
        let preview_dark = self.settings.theme.library_preview_dark.to_color32();
        let mut search = dialog.search.clone();
        let mut open = true;
        let mut cancel_requested = false;
        let mut picked = None;
        let mut source_located = false;
        egui::Window::new("Replace q0rg")
            .open(&mut open)
            .default_size([560.0, 540.0])
            .min_size([240.0, 240.0])
            .default_pos(ctx.screen_rect().center() - egui::vec2(280.0, 270.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(format!("replacing: {}", dialog.source_q0rg_name));
                crate::ui::search_field(ui, &mut search, "Search symbols...");
                ui.label(
                    egui::RichText::new(format!("replacing on frame {}", dialog.frame + 1))
                        .small(),
                );
                ui.separator();

                let needle = search.trim().to_lowercase();
                let visible = candidates
                    .iter()
                    .filter(|(_, name, folder)| {
                        needle.is_empty()
                            || name.to_lowercase().contains(&needle)
                            || folder
                                .as_deref()
                                .is_some_and(|folder| folder.to_lowercase().contains(&needle))
                    })
                    .collect::<Vec<_>>();
                let mut groups = Vec::<(Option<String>, Vec<(u16, String)>)>::new();
                for (q0rg_id, name, folder) in &visible {
                    if let Some((last_folder, items)) = groups.last_mut() {
                        if last_folder == folder {
                            items.push((*q0rg_id, name.clone()));
                            continue;
                        }
                    }
                    groups.push((folder.clone(), vec![(*q0rg_id, name.clone())]));
                }
                egui::ScrollArea::vertical()
                    .id_source(("replace_q0rg_cards", dialog.tab_id))
                    .max_height(440.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.label(if candidates.is_empty() {
                                "No compatible q0rg symbols"
                            } else {
                                "No matching symbols"
                            });
                        } else {
                            for (folder, items) in groups {
                                let title = folder
                                    .as_deref()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| "Unfoldered".to_string());
                                egui::CollapsingHeader::new(title)
                                    .id_source(("replace_q0rg_folder", dialog.tab_id, folder.clone()))
                                    .open((dialog.scroll_to_source && folder.as_deref() == dialog.source_folder.as_deref()).then_some(true))
                                    .default_open(
                                        folder.as_deref() == dialog.source_folder.as_deref(),
                                    )
                                    .show(ui, |ui| {
                                        let width = ui.available_width().max(96.0);
                                        let gap = ui.spacing().item_spacing.x.max(4.0);
                                        let min_card_size = 64.0;
                                        let columns = (((width + gap) / (min_card_size + gap)).floor()
                                            as usize)
                                            .max(1);
                                        let card_size = ((width - gap * columns.saturating_sub(1) as f32)
                                            / columns as f32)
                                            .max(48.0);
                                        egui::Grid::new(("replace_q0rg_grid", dialog.tab_id, folder.clone()))
                                            .num_columns(columns)
                                            .spacing(egui::vec2(gap, gap))
                                            .show(ui, |ui| {
                                                for (index, (q0rg_id, name)) in items.iter().enumerate() {
                                                    let selected = *q0rg_id == dialog.source_q0rg_id;
                                                    let (response, preview_rect) = crate::ui::preview_card(ui, name, selected, card_size);
                                                    let visuals = ui.visuals();
                                                    if ui.is_rect_visible(preview_rect) {
                                                        let preview_painter = ui.painter_at(preview_rect);
                                                        crate::panels::library::draw_q0rg_frame_preview_background(
                                                            &preview_painter, project, *q0rg_id, 0, preview_rect,
                                                            preview_light, preview_dark,
                                                        );
                                                        if let Some(bounds) = crate::render::q0rg_frame_bbox(project, *q0rg_id, 0) {
                                                            let view = crate::panels::library::preview_view(preview_rect, bounds);
                                                            crate::render::render_stage_with_revision(
                                                                &preview_painter, project, *q0rg_id, 0, &view,
                                                                textures, ui.ctx(), render_revision,
                                                            );
                                                        } else {
                                                            preview_painter.text(
                                                                preview_rect.center(), egui::Align2::CENTER_CENTER,
                                                                "empty", egui::FontId::proportional(10.0),
                                                                visuals.weak_text_color(),
                                                            );
                                                        }
                                                    }
                                                    if response.clicked() && *q0rg_id != dialog.source_q0rg_id {
                                                        picked = Some(*q0rg_id);
                                                    }
                                                    if dialog.scroll_to_source && *q0rg_id == dialog.source_q0rg_id {
                                                        ui.scroll_to_rect(response.rect, Some(egui::Align::Center));
                                                        source_located = true;
                                                    }
                                                    if (index + 1) % columns == 0 {
                                                        ui.end_row();
                                                    }
                                                }
                                                if items.len() % columns != 0 {
                                                    ui.end_row();
                                                }
                                            });
                                    });
                            }
                        }
                    });
                ui.separator();
                if ui.button("Cancel").clicked() {
                    cancel_requested = true;
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) && !ui.ctx().wants_keyboard_input() {
                    cancel_requested = true;
                }
            });

        if let Some(replacement_id) = picked {
            self.pending_q0rg_replacement = None;
            self.replace_q0rg_instance(&dialog, replacement_id);
        } else if !open || cancel_requested {
            self.pending_q0rg_replacement = None;
        } else if let Some(pending) = self.pending_q0rg_replacement.as_mut() {
            pending.search = search;
            if source_located {
                pending.scroll_to_source = false;
            }
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

        if let Some((q0rg_id, frames, keyframes, tweens, audio_clips, frame_scripts)) =
            self.pending_frame_truncate
        {
            let frame_labels = self
                .state
                .project
                .runtime
                .frame_labels
                .keys()
                .filter(|key| key.q0rg_id == q0rg_id && key.frame >= frames)
                .count();
            let camera_keys = if q0rg_id == self.state.project.meta.entry_q0rg_id {
                self.state
                    .project
                    .runtime
                    .camera
                    .keys
                    .iter()
                    .filter(|key| key.frame >= frames)
                    .count()
            } else {
                0
            };
            let camera_tweens = if q0rg_id == self.state.project.meta.entry_q0rg_id {
                self.state
                    .project
                    .runtime
                    .camera
                    .keys
                    .iter()
                    .filter(|key| {
                        key.tween
                            .to_frame()
                            .is_some_and(|to_frame| to_frame >= frames)
                    })
                    .count()
            } else {
                0
            };
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Shorten symbol?")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Reduce the symbol to {frames} frame(s)?"));
                    ui.label(format!(
                        "This removes {keyframes} keyframe(s), {tweens} tween(s), {audio_clips} audio clip(s), {frame_scripts} frame script(s), {frame_labels} frame label(s), {camera_keys} camera key(s), and {camera_tweens} camera tween(s)."
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

    fn initialize_preview_q0lang_if_needed(&mut self) {
        if self.session.preview_q0lang_initialized {
            return;
        }
        self.session.preview_q0lang_initialized = true;
        let source = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == self.session.current_q0rg_id)
            .map(|q0rg| q0rg.script.clone())
            .unwrap_or_default();
        let mut transition_budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        let entries_before = self.session.preview_frame_script_entries;
        self.run_preview_q0lang_source_with_budget(&source, &mut transition_budget);
        if self.session.preview_frame_script_entries == entries_before {
            self.run_current_preview_frame_scripts(&mut transition_budget);
        }
    }

    fn run_preview_q0lang_source_with_budget(
        &mut self,
        source: &str,
        transition_budget: &mut usize,
    ) {
        let report = self.session.preview_q0lang_runtime.execute_source(source);
        self.session
            .preview_script_diagnostics
            .extend(report.diagnostics);
        self.apply_preview_q0lang_actions(&report.actions, transition_budget);
    }

    fn run_current_preview_frame_scripts(&mut self, transition_budget: &mut usize) {
        let q0rg_id = self.session.current_q0rg_id;
        let frame = self.session.current_frame;
        self.session.preview_frame_script_entries =
            self.session.preview_frame_script_entries.saturating_add(1);
        let mut scripts = self
            .state
            .project
            .runtime
            .frame_scripts
            .iter()
            .enumerate()
            .filter(|(_, script)| script.q0rg_id == q0rg_id && script.frame == frame)
            .map(|(insertion_order, script)| {
                (script.layer_id, insertion_order, script.source.clone())
            })
            .collect::<Vec<_>>();
        scripts.sort_by_key(|(layer_id, insertion_order, _)| (*layer_id, *insertion_order));
        for (_, _, source) in scripts {
            self.run_preview_q0lang_source_with_budget(&source, transition_budget);
        }
    }

    fn resolve_preview_timeline_target(&self, target: &TimelineTarget) -> Option<u16> {
        match target {
            TimelineTarget::Frame(frame) => Some(*frame),
            TimelineTarget::Label(label) => self
                .state
                .project
                .runtime
                .resolve_frame_label(self.session.current_q0rg_id, label)
                .or_else(|| label.parse::<u16>().ok()),
        }
    }

    fn preview_runtime_transition_to_frame(
        &mut self,
        requested: u16,
        transition_budget: &mut usize,
    ) {
        if *transition_budget == 0 {
            self.session.preview_script_diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!(
                    "frame-script transition budget exceeded ({MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS})"
                ),
            });
            return;
        }
        *transition_budget -= 1;
        let last_frame = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == self.session.current_q0rg_id)
            .map(|q0rg| q0rg.frame_count.saturating_sub(1))
            .unwrap_or(0);
        self.session.current_frame = requested.min(last_frame);
        self.session.pending_timeline_frame = None;
        self.session.preview_audio_resync_needed = true;
        self.run_current_preview_frame_scripts(transition_budget);
    }

    fn apply_preview_q0lang_actions(
        &mut self,
        actions: &[RuntimeAction],
        transition_budget: &mut usize,
    ) {
        for action in actions {
            match action {
                RuntimeAction::GoRun(target) => {
                    if let Some(frame) = self.resolve_preview_timeline_target(target) {
                        self.session.playing = true;
                        self.preview_runtime_transition_to_frame(frame, transition_budget);
                    } else {
                        self.report_missing_preview_frame_label(target);
                    }
                }
                RuntimeAction::GoStop(target) => {
                    if let Some(frame) = self.resolve_preview_timeline_target(target) {
                        self.session.playing = false;
                        self.preview_runtime_transition_to_frame(frame, transition_budget);
                    } else {
                        self.report_missing_preview_frame_label(target);
                    }
                }
                RuntimeAction::ShellCommand { .. }
                | RuntimeAction::SceneSwitch { .. }
                | RuntimeAction::RigSetPosition { .. }
                | RuntimeAction::RigSetValue { .. }
                | RuntimeAction::RigReset { .. }
                | RuntimeAction::RigSetPose { .. }
                | RuntimeAction::RigResetPose { .. } => {
                    // Preview intentionally exposes no shell/filesystem capability and
                    // does not mutate authored rig data. Runtime scene overrides are
                    // introduced by the host-api phase rather than by writing the project.
                }
            }
        }
    }

    fn report_missing_preview_frame_label(&mut self, target: &TimelineTarget) {
        if let TimelineTarget::Label(label) = target {
            self.session
                .preview_script_diagnostics
                .push(RuntimeDiagnostic {
                    line: 0,
                    message: format!("unknown frame label `{label}` in the current symbol"),
                });
        }
    }

    fn tick_playback(&mut self) {
        if !self.session.playing {
            self.session.last_tick = Instant::now();
            return;
        }
        if let Some(audio) = self.audio_playback.as_mut() {
            audio.pump(&self.state.project, self.session.current_q0rg_id);
        }
        let now = Instant::now();
        let dt = (now - self.session.last_tick).as_secs_f32();
        let fps = self.state.project.meta.fps.max(1) as f32;
        let frames = (dt * fps) as u32;
        if frames < 1 {
            return;
        }
        self.session.last_tick += Duration::from_secs_f32(frames as f32 / fps);
        let total = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| q.frame_count.max(1))
            .unwrap_or(1);
        let (loop_start, loop_end) = self
            .session
            .preview_loop_ranges
            .get(&self.session.current_q0rg_id)
            .filter(|range| range.0)
            .map(|range| {
                let start = range.1.min(total - 1);
                (start, range.2.max(start).min(total - 1))
            })
            .unwrap_or((0, total - 1));
        let mut wrapped = false;
        let mut transition_budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        for _ in 0..frames {
            let next = self.session.current_frame.saturating_add(1);
            self.session.current_frame =
                if next > loop_end || self.session.current_frame < loop_start {
                    wrapped = true;
                    loop_start
                } else {
                    next
                };
            self.run_current_preview_frame_scripts(&mut transition_budget);
            if !self.session.playing {
                break;
            }
        }
        if wrapped {
            self.session.preview_audio_resync_needed = true;
        }
        if self.session.preview_audio_resync_needed {
            self.session.preview_audio_resync_needed = false;
            if let Err(error) = self.refresh_audio_playback() {
                self.session.status = format!("frame-code audio warning: {error}");
            }
        }
    }
}

fn color_row(ui: &mut egui::Ui, label: &str, target: &mut crate::settings::ColorRgb) {
    let response = ui.label(label);
    if label == "Window stroke" {
        crate::ui::hidden_palette_note_trigger(ui, &response);
    }
    let mut c = [target.r, target.g, target.b];
    if ui.color_edit_button_srgb(&mut c).changed() {
        target.r = c[0];
        target.g = c[1];
        target.b = c[2];
    }
    ui.end_row();
}

impl App for EditorApp {
    fn raw_input_hook(&mut self, ctx: &Context, raw_input: &mut egui::RawInput) {
        let native_down = crate::system_clipboard::ctrl_v_down();
        let native_pressed = native_down && !self.native_paste_key_down;
        self.native_paste_key_down = native_down;

        if !self.project_active
            || self.show_home
            || self.active_document_is_q0lang()
            || ctx.wants_keyboard_input()
        {
            return;
        }
        if raw_input.modifiers.shift {
            return;
        }
        let egui_paste = raw_input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_)));
        if !native_pressed && !egui_paste {
            return;
        }
        if let Ok(Some(bitmap)) = crate::system_clipboard::read_bitmap() {
            self.pending_clipboard_bitmap = Some(bitmap);
            // If the clipboard contains both image and text/html, image paste has
            // priority on the stage. Do not also paste the internal/text payload.
            raw_input
                .events
                .retain(|event| !matches!(event, egui::Event::Paste(_)));
        }
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.perf.begin_frame();
        let startup_started = Instant::now();
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
        self.perf
            .record(crate::perf::PerfSpan::AppStartup, startup_started.elapsed());

        let ingress_started = Instant::now();
        self.poll_external_open_requests(ctx);
        self.poll_media_import();
        self.poll_fla_import(ctx);
        self.process_pending_clipboard_bitmap();
        self.perf
            .record(crate::perf::PerfSpan::AppIngress, ingress_started.elapsed());

        let prelude_started = Instant::now();
        if ctx.input(|input| input.viewport().close_requested()) && !self.close_requested {
            if self.media_import_job.is_some() || self.fla_import_job.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.session.status =
                    "an import is active; wait for it to finish before closing q0editor"
                        .to_string();
            } else if self.q0enc.has_active_job() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.session.status =
                    "q0enc export is active; cancel it before closing q0editor".to_string();
            } else if self.has_dirty_documents() {
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
        if self.project_active {
            self.tick_playback();
            self.session
                .clear_inactive_frame_selection(&self.state.project);
        }
        self.perf
            .record(crate::perf::PerfSpan::AppPrelude, prelude_started.elapsed());

        let menu_started = Instant::now();
        egui::TopBottomPanel::top("menu_bar")
            .resizable(false)
            .show(ctx, |ui| {
                panels::menu::render(self, ui);
            });
        panels::tabs::render(self, ctx);
        self.perf
            .record(crate::perf::PerfSpan::MenuAndTabs, menu_started.elapsed());

        if self.show_home {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(self.settings.theme.deep_bg.to_color32()))
                .show(ctx, |ui| {
                    panels::home::render(self, ui);
                });
        } else if self.active_document_is_q0lang() {
            egui::CentralPanel::default().show(ctx, |ui| {
                crate::q0lang::editor::render_standalone(self, ui);
            });
        } else {
            if self.timeline_visible {
                let timeline_started = Instant::now();
                egui::TopBottomPanel::bottom("timeline")
                    .resizable(true)
                    .default_height(192.0)
                    .min_height(120.0)
                    .show(ctx, |ui| {
                        crate::ui::panel_contents(ui, |ui| panels::timeline::render(self, ui));
                    });
                self.perf
                    .record(crate::perf::PerfSpan::Timeline, timeline_started.elapsed());
                self.session
                    .clear_inactive_frame_selection(&self.state.project);
            }

            let toolbar_started = Instant::now();
            egui::SidePanel::left("toolbar")
                .resizable(false)
                .exact_width(48.0)
                .show(ctx, |ui| {
                    crate::ui::panel_contents(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_source("tools_scroll")
                            .show(ui, |ui| panels::toolbar::render(self, ui));
                    });
                });
            self.perf
                .record(crate::perf::PerfSpan::Toolbar, toolbar_started.elapsed());

            egui::SidePanel::right("right_dock")
                .resizable(true)
                .default_width(290.0)
                .min_width(200.0)
                .show(ctx, |ui| {
                    crate::ui::panel_contents(ui, |ui| {
                        let half = ui.available_height() * 0.5;
                        egui::TopBottomPanel::top("dock_library")
                            .resizable(true)
                            .default_height(half)
                            .show_inside(ui, |ui| {
                                // The Library owns its own item-list ScrollArea. Wrapping the
                                // whole panel in another vertical ScrollArea makes the preview
                                // height depend on the outer scroll offset and can create a
                                // resize feedback loop where both areas keep clamping each other.
                                let started = Instant::now();
                                crate::ui::panel_contents(ui, |ui| {
                                    panels::library::render(self, ui);
                                });
                                self.perf
                                    .record(crate::perf::PerfSpan::Library, started.elapsed());
                            });
                        egui::CentralPanel::default().show_inside(ui, |ui| {
                            let properties_started = Instant::now();
                            crate::ui::panel_contents(ui, |ui| {
                                panels::properties::render(self, ui);
                            });
                            self.perf.record(
                                crate::perf::PerfSpan::Properties,
                                properties_started.elapsed(),
                            );
                        });
                    });
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                panels::stage::render(self, ui);
            });

            let auxiliary_started = Instant::now();
            panels::q0enc::render(self, ctx);
            panels::stage::render_animation_preview(self, ctx);
            crate::q0lang::render(self, ctx);
            crate::q0lang::frame_editor::render(self, ctx);
            crate::easing::render_editor(self, ctx);
            crate::easing::render_warning(self, ctx);
            self.perf.record(
                crate::perf::PerfSpan::AuxiliaryPanels,
                auxiliary_started.elapsed(),
            );
        }

        let dialogs_started = Instant::now();
        panels::q0fla::render(self, ctx);

        if self.show_project_tree {
            self.render_project_tree_dialog(ctx);
        }
        if self.session.show_credits {
            self.render_credits_dialog(ctx);
        }
        if self.session.show_settings {
            self.render_settings_dialog(ctx);
        }
        if self.session.show_shortcuts {
            self.render_shortcuts_dialog(ctx);
        }
        self.render_new_project_dialog(ctx);
        self.render_replace_q0rg_dialog(ctx);
        self.render_destructive_change_dialogs(ctx);
        self.render_unsaved_changes_dialog(ctx);
        self.perf
            .record(crate::perf::PerfSpan::Dialogs, dialogs_started.elapsed());

        // Update window title
        let title = if self.show_home {
            "q0editor - home".to_string()
        } else if let Some(document) = self.active_q0lang_document() {
            let name = document
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("script.q0l");
            format!(
                "q0editor - {name}{}",
                if document.dirty { " *" } else { "" }
            )
        } else {
            self.state.title()
        };
        if self.last_window_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = Some(title);
        }

        let action_drain_started = Instant::now();
        self.drain_actions(ctx);
        for status in self.q0enc.poll_events() {
            self.session.status = status;
        }
        if self.project_active {
            self.session
                .clear_inactive_frame_selection(&self.state.project);
        }
        self.perf.record(
            crate::perf::PerfSpan::ActionDrain,
            action_drain_started.elapsed(),
        );

        if self.media_import_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.session.playing || self.q0enc.queue_running {
            ctx.request_repaint();
        }
        self.perf.finish_frame();
    }
}

#[derive(Debug, Clone)]
struct BreakApartPiece {
    placement: Placement,
    raw_path_count: usize,
}

pub(crate) fn affine_to_transform(affine: Affine) -> Option<Transform2D> {
    let sx = affine.a11.hypot(affine.a21);
    let determinant = affine.a11 * affine.a22 - affine.a12 * affine.a21;
    if !sx.is_finite() || sx <= 1.0e-7 || !determinant.is_finite() || determinant.abs() <= 1.0e-9 {
        return None;
    }
    let rotation = affine.a21.atan2(affine.a11);
    let (sin, cos) = rotation.sin_cos();
    let sheared_x = cos * affine.a12 + sin * affine.a22;
    let sy = -sin * affine.a12 + cos * affine.a22;
    if !sy.is_finite() || sy.abs() <= 1.0e-7 {
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
        clip_polygon: None,
    }
}

#[cfg(feature = "appearance-mask-eraser")]
fn baked_vector_appearance(
    project: &ProjectV2,
    source: &VectorAsset,
    transform: Affine,
) -> Option<q0s_format::v2::VectorAppearance> {
    let mut appearance = project.asset_appearances.get(&source.asset_id)?.clone();
    crate::appearance::freeze_material_source_from_current_body(source, &mut appearance);
    Some(crate::appearance::transform_appearance(
        &appearance,
        transform,
    ))
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
    #[cfg(feature = "appearance-mask-eraser")]
    use geo::{Area, BooleanOps};
    use image::ImageEncoder;
    use std::io::Cursor;

    #[test]
    fn switching_from_lasso_to_select_keeps_stage_selection() {
        let mut app = EditorApp::default();
        let selection = Selection::Placement {
            q0rg_id: app.session.current_q0rg_id,
            layer_id: app.session.current_layer_id,
            placement_idx: 0,
        };
        app.session.current_tool = Tool::Lasso;
        app.session.selection = selection.clone();
        app.session.tool_state = ToolState::LassoDrawing {
            points: vec![Vec2::new(10.0, 10.0)],
        };

        app.handle(&Context::default(), Action::SelectTool(Tool::Select));

        assert_eq!(app.session.current_tool, Tool::Select);
        assert_eq!(app.session.selection, selection);
        assert!(matches!(app.session.tool_state, ToolState::Idle));
    }

    #[test]
    fn switching_from_subselect_to_select_promotes_points_to_whole_raw_path() {
        let mut app = EditorApp::default();
        let path = PathRef {
            q0rg_id: app.session.current_q0rg_id,
            layer_id: app.session.current_layer_id,
            placement_idx: 3,
            path_idx: 7,
        };
        app.session.current_tool = Tool::Subselect;
        app.session.selection = Selection::SubselectPoints {
            path,
            anchor_indices: vec![1, 2],
            bounds_min: Vec2::new(10.0, 20.0),
            bounds_max: Vec2::new(80.0, 60.0),
        };
        app.session.tool_state = ToolState::DraggingPathPoints {
            path,
            anchor_indices: vec![1, 2],
            start_cursor: Vec2::new(20.0, 30.0),
            start_path: q0s_format::v2::Path {
                anchors: Vec::new(),
                closed: false,
            },
            start_bounds_min: Vec2::new(10.0, 20.0),
            start_bounds_max: Vec2::new(80.0, 60.0),
        };

        app.handle(&Context::default(), Action::SelectTool(Tool::Select));

        assert_eq!(app.session.current_tool, Tool::Select);
        assert_eq!(
            app.session.selection,
            Selection::Path {
                q0rg_id: path.q0rg_id,
                layer_id: path.layer_id,
                placement_idx: path.placement_idx,
                path_idx: path.path_idx,
            }
        );
        assert!(matches!(app.session.tool_state, ToolState::Idle));
    }

    #[test]
    fn select_tool_keeps_real_partial_raw_selection_partial() {
        let mut app = EditorApp::default();
        let selection = Selection::PathPoints {
            path: PathRef {
                q0rg_id: app.session.current_q0rg_id,
                layer_id: app.session.current_layer_id,
                placement_idx: 0,
                path_idx: 0,
            },
            anchor_indices: vec![1, 2],
            bounds_min: Vec2::new(10.0, 20.0),
            bounds_max: Vec2::new(80.0, 60.0),
        };
        app.session.current_tool = Tool::Select;
        app.session.selection = selection.clone();

        app.handle(&Context::default(), Action::SelectTool(Tool::Select));

        assert_eq!(app.session.selection, selection);
    }
    fn app_test_nonce() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    }

    fn test_audio_q0v_bytes(sample_rate: u32, sample_frames: u32) -> Vec<u8> {
        let spec = q0video::q0v::Q0vSpec {
            width: 0,
            height: 0,
            fps: sample_rate,
            timeline_frames: sample_frames,
            video: false,
            audio: true,
            audio_sample_rate: sample_rate,
            audio_channels: 2,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(Cursor::new(Vec::new()), spec)
            .expect("create audio q0v fixture");
        let mut pcm = Vec::with_capacity(sample_frames as usize * 2);
        for sample in 0..sample_frames {
            let value = if sample % 2 == 0 { 12_000 } else { -12_000 };
            pcm.extend_from_slice(&[value, -value]);
        }
        writer
            .write_audio_pcm_i16(&pcm)
            .expect("write audio q0v fixture");
        writer
            .finish()
            .expect("finish audio q0v fixture")
            .into_inner()
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

    #[cfg(feature = "appearance-mask-eraser")]
    fn test_glow_appearance() -> q0s_format::v2::VectorAppearance {
        q0s_format::v2::VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.65,
            },
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        }
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
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
    fn deleting_last_stage_object_turns_content_key_into_blank_key() {
        let mut app = app_with_one_timeline_object(12);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.session.current_frame = 0;
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.placements.is_empty());
        assert!(layer.is_blank_keyframe(0));
        assert_eq!(layer.keyframe_frames(), vec![0]);
    }

    #[test]
    fn deleting_last_tween_target_from_stage_keeps_blank_key_and_clears_incoming_tween() {
        let mut app = app_with_one_timeline_object(12);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.state.project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 5 };
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 5,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 50.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 5;
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 1,
        };

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(5));
        assert!(matches!(layer.placements[0].tween, Tween::None));
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
    }

    #[test]
    fn deleting_all_stage_objects_on_held_frame_creates_blank_key_without_mutating_source() {
        let mut app = app_with_one_timeline_object(12);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(78),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 5;
        app.session.selection = Selection::Multi(vec![
            crate::state::PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            },
            crate::state::PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 1,
            },
        ]);

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 2);
        assert!(layer.is_blank_keyframe(5));
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 0).count(), 2);
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 5).count(), 0);
    }

    #[test]
    fn deleting_last_raw_fill_turns_content_key_into_blank_key() {
        let mut app = held_raw_app();
        app.session.current_frame = 0;
        app.state.project.assets.truncate(1);
        app.state.project.q0rgs[0].layers[0].placements.truncate(1);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.session.selection = crate::tools::selection_at_point_pub(
            &app.state.project,
            1,
            0,
            q0s_format::v2::Vec2::new(10.0, 10.0),
        )
        .expect("raw selection");

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.placements.is_empty());
        assert!(layer.is_blank_keyframe(0));
        assert_eq!(layer.keyframe_frames(), vec![0]);
        q0s_format::v2::validate(&app.state.project).expect("blank raw frame remains valid");
    }

    #[test]
    fn deleting_last_held_raw_fill_creates_blank_key_without_mutating_source() {
        let mut app = held_raw_app();
        app.state.project.assets.truncate(1);
        app.state.project.q0rgs[0].layers[0].placements.truncate(1);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.session.selection = crate::tools::selection_at_point_pub(
            &app.state.project,
            1,
            5,
            q0s_format::v2::Vec2::new(10.0, 10.0),
        )
        .expect("held raw selection");

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 1);
        assert!(layer.is_blank_keyframe(5));
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 0).count(), 1);
        q0s_format::v2::validate(&app.state.project).expect("held raw delete remains valid");
    }

    #[test]
    fn deleting_held_raw_area_creates_blank_key_without_mutating_source() {
        let mut app = held_raw_app();
        app.state.project.assets.truncate(1);
        app.state.project.q0rgs[0].layers[0].placements.truncate(1);
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.session.selection = Selection::RawArea {
            placements: vec![crate::state::PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            }],
            objects: Vec::new(),
            bounds_min: Vec2::new(-1.0, -1.0),
            bounds_max: Vec2::new(21.0, 21.0),
            clip_polygon: None,
        };

        assert!(app.delete_selection());

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 1);
        assert!(layer.is_blank_keyframe(5));
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
        q0s_format::v2::validate(&app.state.project).expect("held raw area delete remains valid");
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
            raw_vectors: vec![crate::state::RawVectorClipboard {
                vector: q0s_format::v2::VectorAsset {
                    asset_id: 0,
                    paths: vec![test_square(40.0, 60.0)],
                    fill: Some(q0s_format::v2::Rgba {
                        r: 200,
                        g: 30,
                        b: 40,
                        a: 128,
                    }),
                    stroke: None,
                },
                appearance: None,
            }],
            artwork_center: None,
            timeline: None,
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
        assert_eq!(app.document_tabs.len(), 1);
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
        let first_tab = app.active_document_tab.expect("first project tab");
        app.state.mark_dirty();
        app.session.current_frame = 7;

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Next".to_string(),
                ..NewProjectSpec::default()
            }),
        );

        assert_eq!(app.document_tabs.len(), 2);
        assert_eq!(app.state.project.meta.name, "Next");
        assert!(app.switch_to_document_tab(first_tab));
        assert_eq!(app.state.project.meta.name, "Unsaved work");
        assert!(app.state.dirty);
        assert_eq!(app.session.current_frame, 7);
    }

    #[test]
    fn saved_document_tabs_use_file_stems_instead_of_stale_meta_names() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec::default()),
        );
        app.state.file_path = Some(PathBuf::from(r"F:\projects\APPROVED.q1s"));

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec::default()),
        );
        app.state.file_path = Some(PathBuf::from(r"F:\projects\advanced smoke\smoke-lag.q1s"));

        let summaries = app.document_tab_summaries();
        let titles = summaries
            .iter()
            .map(|tab| tab.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["APPROVED", "smoke-lag"]);
    }

    #[test]
    fn library_picker_uses_q1s_file_names_and_keeps_all_libraries_distinct() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "All libraries".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        app.state.file_path = Some(PathBuf::from(r"F:\projects\All libraries.q1s"));

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Other".to_string(),
                ..NewProjectSpec::default()
            }),
        );

        let libraries = app.project_library_summaries();
        let labels = libraries
            .iter()
            .map(|library| library.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["All libraries.q1s", "Other.q1s"]);
        assert!(!labels.contains(&"All libraries"));
    }

    #[test]
    fn cross_project_library_drag_imports_then_places_as_one_undoable_change() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Source library".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let source_tab = app.active_document_tab.expect("source project tab");
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(10.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 180,
                g: 80,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        app.state
            .project
            .asset_names
            .insert(77, "Shared Hand".to_string());
        let source_before = app.state.project.clone();

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Destination".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let destination_before = app.state.project.clone();
        let drop = Vec2::new(240.0, 170.0);
        app.handle(
            &Context::default(),
            Action::PlaceLibraryDragAt(
                LibraryDragPayload {
                    source_tab_id: source_tab,
                    item: LibraryItem::Asset(77),
                },
                drop,
            ),
        );

        assert_eq!(
            app.project_for_document_tab(source_tab),
            Some(&source_before),
            "dropping from another library must not mutate that open project"
        );
        assert_eq!(
            app.state.project.assets.len(),
            destination_before.assets.len() + 1
        );
        let imported_id = app
            .state
            .project
            .asset_names
            .iter()
            .find_map(|(&id, name)| (name == "Shared Hand").then_some(id))
            .expect("imported asset name");
        assert_ne!(
            imported_id, 77,
            "source ids are project-local and must be remapped"
        );
        let layer = &app.state.project.q0rgs[0].layers[0];
        let placement = layer.placements.last().expect("dropped placement");
        assert_eq!(placement.target, Target::Asset(imported_id));
        q0s_format::v2::validate(&app.state.project).expect("cross-project drop project");

        app.handle(&Context::default(), Action::Undo);
        assert_eq!(
            app.state.project, destination_before,
            "one undo must remove both the placed object and imported library dependencies"
        );
    }

    #[test]
    fn rejected_cross_project_library_drop_rolls_back_imported_dependencies() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Source library".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let source_tab = app.active_document_tab.expect("source project tab");
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 91,
            paths: vec![test_square(0.0, 0.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 90,
                g: 120,
                b: 150,
                a: 255,
            }),
            stroke: None,
        }));

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Locked destination".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let key = LayerKey::new(app.session.current_q0rg_id, app.session.current_layer_id);
        app.state.project.layer_metadata.insert(
            key,
            LayerMetadata {
                locked: true,
                ..Default::default()
            },
        );
        let before = app.state.project.clone();
        app.handle(
            &Context::default(),
            Action::PlaceLibraryDragAt(
                LibraryDragPayload {
                    source_tab_id: source_tab,
                    item: LibraryItem::Asset(91),
                },
                Vec2::new(100.0, 100.0),
            ),
        );

        assert_eq!(
            app.state.project, before,
            "a rejected drop must not leave the copied asset in the destination library"
        );
        assert!(app.session.status.contains("unlock"));
    }

    #[test]
    fn duplicate_symbol_names_get_hash_suffixes_instead_of_failing() {
        let mut app = EditorApp::default();
        for _ in 0..3 {
            app.handle(&Context::default(), Action::AddQ0rg);
        }

        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Q0rg(2), "Hero".to_string()),
        );
        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Q0rg(3), "Hero".to_string()),
        );
        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Q0rg(4), "Hero".to_string()),
        );

        let names = app
            .state
            .project
            .q0rgs
            .iter()
            .filter(|q0rg| q0rg.q0rg_id >= 2)
            .map(|q0rg| q0rg.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Hero", "Hero #2", "Hero #3"]);
    }
    #[test]
    fn home_and_document_tabs_preserve_independent_document_state() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Alpha".to_string(),
                frame_count: 40,
                ..NewProjectSpec::default()
            }),
        );
        let alpha = app.active_document_tab.expect("alpha tab");
        app.session.current_frame = 12;
        app.state.mark_dirty();

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                frame_count: 80,
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_document_tab.expect("beta tab");
        app.session.current_frame = 3;

        assert!(app.switch_to_home());
        assert!(app.home_visible());
        assert_eq!(app.document_tab_summaries().len(), 2);
        assert!(app.switch_to_document_tab(alpha));
        assert_eq!(app.state.project.meta.name, "Alpha");
        assert_eq!(app.session.current_frame, 12);
        assert!(app.state.dirty);
        assert!(app.switch_to_document_tab(beta));
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
        let alpha = app.active_document_tab.expect("alpha tab");
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_document_tab.expect("beta tab");

        app.cycle_document_tab(1);
        assert!(app.home_visible());
        app.cycle_document_tab(1);
        assert_eq!(app.active_document_tab, Some(alpha));
        app.cycle_document_tab(1);
        assert_eq!(app.active_document_tab, Some(beta));
        app.cycle_document_tab(-1);
        assert_eq!(app.active_document_tab, Some(alpha));
    }

    #[test]
    fn closing_a_dirty_tab_waits_for_save_or_discard() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec::default()),
        );
        let tab_id = app.active_document_tab.expect("project tab");
        app.state.mark_dirty();

        app.handle(&Context::default(), Action::CloseDocumentTab(tab_id));

        assert_eq!(app.document_tabs.len(), 1);
        assert!(matches!(
            app.unsaved_action,
            Some(Action::CloseDocumentTab(id)) if id == tab_id
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
        let alpha = app.active_document_tab.expect("alpha tab");
        app.state.mark_dirty();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_document_tab.expect("beta tab");
        app.state.mark_dirty();

        app.handle(&Context::default(), Action::Exit);
        assert_eq!(app.active_document_tab, Some(alpha));
        assert!(matches!(app.unsaved_action, Some(Action::Exit)));

        app.unsaved_action = None;
        app.state.dirty = false;
        app.handle(&Context::default(), Action::Exit);
        assert_eq!(app.active_document_tab, Some(beta));
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
        app.state.mark_dirty();
        let missing_parent =
            std::env::temp_dir().join(format!("q0editor-missing-parent-{}", std::process::id()));
        let path = missing_parent.join("project.q1s");

        assert!(!app.save_to(&path));
        assert!(app.state.file_path.is_none());
        assert!(app.state.dirty);
    }

    #[test]
    fn f5_without_timeline_selection_inserts_before_playhead_on_current_layer() {
        let mut app = app_with_one_timeline_object(6);
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 4,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 10.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        let mut second_layer = app.state.project.q0rgs[0].layers[0].clone();
        second_layer.layer_id = 2;
        second_layer.name = "Layer 2".to_string();
        second_layer.placements[1].transform.tx = 20.0;
        app.state.project.q0rgs[0].layers.push(second_layer);
        app.session.current_layer_id = 2;
        app.session.current_frame = 4;
        app.session.timeline_selection = None;

        app.handle(&Context::default(), Action::InsertFrame);

        let q0rg = &app.state.project.q0rgs[0];
        assert_eq!(q0rg.frame_count, 7);
        assert!(q0rg.layers[0]
            .placements
            .iter()
            .any(|placement| placement.frame == 4 && placement.transform.tx == 10.0));
        assert!(q0rg.layers[1]
            .placements
            .iter()
            .any(|placement| placement.frame == 5 && placement.transform.tx == 20.0));
        assert_eq!(app.session.current_layer_id, 2);
        assert_eq!(app.session.current_frame, 4);
        assert_eq!(
            app.session.timeline_selection,
            Some(TimelineSelection::single(2, 4))
        );
        assert!(app.state.dirty);
        assert!(app.history.can_undo());
    }

    #[test]
    fn f5_uses_selected_timeline_cell_instead_of_playhead_fallback() {
        let mut app = app_with_one_timeline_object(6);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 4,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 40.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 1;
        app.session.timeline_selection = Some(TimelineSelection::single(layer_id, 4));

        app.handle(&Context::default(), Action::InsertFrame);

        let q0rg = &app.state.project.q0rgs[0];
        assert_eq!(q0rg.frame_count, 7);
        assert!(q0rg.layers[0]
            .placements
            .iter()
            .any(|placement| placement.frame == 5 && placement.transform.tx == 40.0));
        assert_eq!(
            crate::render::active_placements_at(&q0rg.layers[0], 4).len(),
            1
        );
        assert_eq!(app.session.current_frame, 4);
    }

    #[test]
    fn f5_on_virtual_frame_extends_through_the_clicked_destination() {
        let mut app = EditorApp::default();
        app.state.dirty = false;
        app.session.pending_timeline_frame = Some(39);

        app.handle(&Context::default(), Action::InsertFrame);

        let q0rg = &app.state.project.q0rgs[0];
        let layer = &q0rg.layers[0];
        assert_eq!(q0rg.frame_count, 40);
        assert_eq!(app.session.current_frame, 39);
        assert_eq!(app.session.pending_timeline_frame, None);
        assert_eq!(layer.keyframe_frames(), vec![0]);
        assert!(layer.is_blank_keyframe(0));
        assert!(app.state.dirty);
        assert!(app.history.can_undo());
    }

    #[test]
    fn f6_on_virtual_frame_extends_and_bakes_the_held_contents() {
        let mut app = app_with_one_timeline_object(4);
        app.session.pending_timeline_frame = Some(9);

        app.handle(&Context::default(), Action::InsertKeyframe);

        let q0rg = &app.state.project.q0rgs[0];
        let layer = &q0rg.layers[0];
        assert_eq!(q0rg.frame_count, 10);
        assert_eq!(app.session.current_frame, 9);
        assert_eq!(app.session.pending_timeline_frame, None);
        assert!(layer
            .placements
            .iter()
            .any(|placement| placement.frame == 9));
        assert_eq!(crate::render::active_placements_at(layer, 9).len(), 1);
    }
    #[test]
    fn frame_clipboard_pastes_at_a_virtual_timeline_destination() {
        let mut app = app_with_one_timeline_object(4);
        let layer_id = app.session.current_layer_id;
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 0));
        app.handle(&Context::default(), Action::CopySelection);
        app.session.pending_timeline_frame = Some(9);
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 9));

        app.handle(&Context::default(), Action::Paste);

        let q0rg = &app.state.project.q0rgs[0];
        let layer = &q0rg.layers[0];
        assert_eq!(q0rg.frame_count, 10);
        assert_eq!(app.session.current_frame, 9);
        assert_eq!(app.session.pending_timeline_frame, None);
        assert!(layer
            .placements
            .iter()
            .any(|placement| placement.frame == 9));
    }

    #[test]
    fn deleting_a_virtual_frame_never_mutates_the_last_real_frame() {
        let mut app = app_with_one_timeline_object(4);
        let before = app.state.project.clone();
        let layer_id = app.session.current_layer_id;
        app.session.pending_timeline_frame = Some(9);
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 9));

        app.handle(&Context::default(), Action::DeleteSelection);

        assert_eq!(app.state.project, before);
        assert!(app.session.status.contains("future frame is not created"));
    }

    fn app_with_one_timeline_object(frame_count: u16) -> EditorApp {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].frame_count = frame_count;
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(77),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.current_q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.dirty = false;
        app
    }

    #[test]
    fn tween_command_uses_interior_timeline_cell_not_stage_selection() {
        let mut app = app_with_one_timeline_object(8);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 6,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 60.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 3;
        app.session.timeline_selection = Some(TimelineSelection::single(layer_id, 3));
        app.session.selection = Selection::None;

        app.handle(&Context::default(), Action::CreateMotionTween);

        assert_eq!(
            app.state.project.q0rgs[0].layers[0].placements[0].tween,
            Tween::Linear { to_frame: 6 }
        );
        assert!(app.state.dirty);
        assert!(app.history.can_undo());
        assert!(app.session.tween_warning.is_none());
    }

    #[test]
    fn tween_command_on_keyframe_shows_warning_without_mutating_project() {
        let mut app = app_with_one_timeline_object(8);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 6,
                target: Target::Asset(77),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 0;
        app.session.timeline_selection = Some(TimelineSelection::single(layer_id, 0));
        let before = app.state.project.clone();

        app.handle(&Context::default(), Action::CreateMotionTween);

        assert_eq!(app.state.project, before);
        assert!(!app.state.dirty);
        assert!(!app.history.can_undo());
        assert!(app
            .session
            .tween_warning
            .as_deref()
            .is_some_and(|warning| warning.contains("cannot be created on a keyframe")));
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
    fn clear_keyframe_on_a_held_frame_is_a_noop() {
        let mut app = app_with_one_timeline_object(12);
        app.session.current_frame = 5;

        app.handle(&Context::default(), Action::ClearKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(!layer.has_keyframe(5));
        assert_eq!(crate::render::active_placements_at(layer, 5).len(), 1);
        assert!(!app.state.dirty);
        assert!(!app.history.can_undo());
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
            timeline_frame_count: None,
            name: "Layer 2".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(88),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
                instance_id: 0,
                frame: 5,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 5));

        app.handle(&Context::default(), Action::DeleteSelection);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(5));
        assert!(matches!(layer.placements[0].tween, Tween::None));
    }

    #[test]
    fn duplicate_timeline_range_places_a_selected_copy_after_the_source() {
        let mut app = app_with_one_timeline_object(6);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .push(2);
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 1,
            focus_layer_id: layer_id,
            focus_frame: 2,
        });

        app.handle(&Context::default(), Action::DuplicateSelection);

        let selection = app
            .session
            .timeline_selection
            .expect("duplicated selection");
        assert_eq!((selection.anchor_frame, selection.focus_frame), (3, 4));
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.has_keyframe(3));
        assert!(layer.is_blank_keyframe(4));
        assert!(app.history.can_undo());
    }

    #[test]
    fn f6_inside_a_tween_retargets_the_incoming_tween_to_the_new_key() {
        let mut app = app_with_one_timeline_object(8);
        app.state.project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 6 };
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 6,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 60.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 3;

        app.handle(&Context::default(), Action::InsertKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(layer.placements[0].tween, Tween::Linear { to_frame: 3 });
        assert!(layer.has_keyframe(3));
    }

    #[test]
    fn f6_materializes_every_cell_in_a_multi_frame_selection() {
        let mut app = app_with_one_timeline_object(8);
        let layer_id = app.session.current_layer_id;
        app.session.current_frame = 3;
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 2,
            focus_layer_id: layer_id,
            focus_frame: 3,
        });

        app.handle(&Context::default(), Action::InsertKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.has_keyframe(2));
        assert!(layer.has_keyframe(3));
    }

    #[test]
    fn f7_blanks_every_cell_in_a_multi_frame_selection() {
        let mut app = app_with_one_timeline_object(8);
        let layer_id = app.session.current_layer_id;
        app.session.current_frame = 3;
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 2,
            focus_layer_id: layer_id,
            focus_frame: 3,
        });

        app.handle(&Context::default(), Action::InsertBlankKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(2));
        assert!(layer.is_blank_keyframe(3));
        assert!(crate::render::active_placements_at(layer, 2).is_empty());
        assert!(crate::render::active_placements_at(layer, 3).is_empty());
    }

    #[test]
    fn clear_keyframe_demotes_selected_keys_but_never_the_root_key() {
        let mut app = app_with_one_timeline_object(8);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .push(4);
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 3,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 30.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 4;
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 0,
            focus_layer_id: layer_id,
            focus_frame: 4,
        });

        app.handle(&Context::default(), Action::ClearKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.has_keyframe(0));
        assert!(!layer.has_keyframe(3));
        assert!(!layer.has_keyframe(4));
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 1);
    }

    #[test]
    fn remove_frame_cuts_the_selected_time_range_instead_of_only_the_tail() {
        let mut app = app_with_one_timeline_object(10);
        let layer_id = app.session.current_layer_id;
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 7,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 70.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.current_frame = 4;
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 3,
            focus_layer_id: layer_id,
            focus_frame: 4,
        });

        app.handle(&Context::default(), Action::RemoveFrame);

        let q0rg = &app.state.project.q0rgs[0];
        assert_eq!(q0rg.frame_count, 8);
        assert!(q0rg.layers[0]
            .placements
            .iter()
            .any(|placement| placement.frame == 5 && placement.transform.tx == 70.0));
        assert_eq!(app.session.current_frame, 3);
    }

    #[test]
    fn removing_a_frame_range_is_undoable_as_one_operation() {
        let mut app = app_with_one_timeline_object(8);
        let before = app.state.project.clone();
        let layer_id = app.session.current_layer_id;
        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 2,
            focus_layer_id: layer_id,
            focus_frame: 4,
        });

        app.handle(&Context::default(), Action::RemoveFrame);
        assert_ne!(app.state.project, before);
        app.handle(&Context::default(), Action::Undo);

        assert_eq!(app.state.project, before);
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
    fn clipboard_bitmap_paste_centres_display_object_on_stage_cursor() {
        let mut encoded = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut encoded)
            .encode(
                &[
                    255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255, 32, 64,
                    96, 255, 255, 255,
                ],
                4,
                2,
                image::ColorType::Rgb8,
            )
            .expect("encode clipboard bmp fixture");

        let mut app = EditorApp {
            project_active: true,
            show_home: false,
            ..Default::default()
        };
        app.session.stage_cursor = Some(Vec2::new(300.0, 200.0));
        let before_assets = app.state.project.assets.len();
        app.paste_bitmap_from_clipboard(crate::system_clipboard::ClipboardBitmap { encoded });

        assert_eq!(app.state.project.assets.len(), before_assets + 1);
        let Asset::Bitmap(bitmap) = app
            .state
            .project
            .assets
            .last()
            .expect("clipboard bitmap asset")
        else {
            panic!("clipboard paste must create a bitmap asset");
        };
        assert_eq!((bitmap.width, bitmap.height), (4, 2));
        let layer = &app.state.project.q0rgs[0].layers[0];
        let placement = layer.placements.last().expect("clipboard bitmap placement");
        assert_eq!(placement.target, Target::Asset(bitmap.asset_id));
        assert!((placement.transform.tx - 298.0).abs() < 1.0e-6);
        assert!((placement.transform.ty - 199.0).abs() < 1.0e-6);
        assert!(matches!(
            app.session.selection,
            Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx
            } if placement_idx == layer.placements.len() - 1
        ));
        assert_eq!(app.session.status, "pasted bitmap from clipboard (4x2)");
        assert!(app.history.can_undo());
        q0s_format::v2::validate(&app.state.project).expect("clipboard bitmap paste remains valid");
    }
    #[test]
    fn project_tree_window_is_independent_from_settings_and_credits() {
        let mut app = EditorApp::default();
        app.session.show_settings = true;
        app.session.show_credits = true;
        assert!(!app.project_tree_open());

        app.handle(&Context::default(), Action::ToggleProjectTree);
        assert!(app.project_tree_open());
        assert!(app.session.show_settings);
        assert!(app.session.show_credits);

        app.handle(&Context::default(), Action::ToggleProjectTree);
        assert!(!app.project_tree_open());
        assert!(app.session.show_settings);
        assert!(app.session.show_credits);
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
    fn quick_xfl_open_creates_an_unsaved_dirty_q1s_workspace() {
        let root = std::env::temp_dir().join(format!(
            "q0editor-q0fla-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create xfl fixture dir");
        let marker = root.join("animate-source.xfl");
        std::fs::write(&marker, b"PROXY-CS5").expect("write xfl marker");
        std::fs::write(
            root.join("DOMDocument.xml"),
            br#"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="960" height="540" frameRate="25"><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Ink"><frames><DOMFrame index="0" duration="10"/></frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#,
        )
        .expect("write DOMDocument.xml");

        let mut app = EditorApp::default();
        let ctx = Context::default();
        assert!(app.open_from_path(&ctx, &marker));
        for _ in 0..100 {
            app.poll_fla_import(&ctx);
            if app.fla_import_job.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.project_active);
        assert!(!app.show_home);
        assert!(
            app.state.file_path.is_none(),
            "quick import must not claim the source FLA/XFL as its q1s save path"
        );
        assert!(
            app.state.dirty,
            "imported project must immediately require a q1s save"
        );
        assert_eq!(app.state.project.meta.stage_width, 960);
        assert_eq!(app.state.project.meta.stage_height, 540);
        assert_eq!(app.state.project.meta.fps, 25);
        assert_eq!(app.state.project.q0rgs[0].frame_count, 10);
        assert_eq!(app.state.project.q0rgs[0].layers[0].name, "Ink");
        let active = app
            .document_tab_summaries()
            .into_iter()
            .find(|tab| tab.active)
            .expect("active imported tab");
        assert_eq!(active.title, "animate-source.q1s");
        assert!(active.dirty);
        assert!(app.state.title().contains("animate-source.q1s*"));
        assert!(!root.join("animate-source.q1s").exists());

        let _ = std::fs::remove_dir_all(root);
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
    fn affine_decomposition_round_trips_reflection() {
        let source = Transform2D {
            tx: 18.0,
            ty: -9.0,
            sx: -1.4,
            sy: 0.75,
            rotation: 0.31,
            skew_x: 0.16,
            skew_y: 0.0,
        };
        let affine = Affine::from_transform(source);
        let decomposed = affine_to_transform(affine).expect("reflection decomposition");
        let rebuilt = Affine::from_transform(decomposed);
        for point in [
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, -5.0),
            Vec2::new(-3.0, 11.0),
        ] {
            let expected = affine.apply(point);
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
                instance_id: 0,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 50.0,
                    ty: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        #[cfg(feature = "appearance-mask-eraser")]
        let expected_glow = {
            app.state
                .project
                .asset_appearances
                .insert(77, test_glow_appearance());
            let Asset::Vector(source) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == 77)
                .unwrap()
            else {
                unreachable!()
            };
            let appearance = app.state.project.asset_appearances.get(&77).unwrap();
            crate::appearance::transform_surface(
                &crate::appearance::visible_material_surface_for_vector(source, Some(appearance)),
                Affine::from_transform(
                    app.state.project.q0rgs[0].layers[0].placements[0].transform,
                ),
            )
        };
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
        #[cfg(feature = "appearance-mask-eraser")]
        {
            let appearance = app
                .state
                .project
                .asset_appearances
                .get(&new_id)
                .expect("break apart must preserve vector glow metadata");
            assert!(matches!(
                appearance.material,
                q0s_format::v2::VectorMaterial::SoftHalo { .. }
            ));
            let actual_glow =
                crate::appearance::visible_material_surface_for_vector(vector, Some(appearance));
            let mismatch = expected_glow.difference(&actual_glow).unsigned_area()
                + actual_glow.difference(&expected_glow).unsigned_area();
            assert!(
                mismatch < 0.01,
                "break apart changed the visible glow support: symmetric difference area={mismatch}"
            );
        }
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
    fn undoing_selected_placement_scale_keeps_the_object_selected() {
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
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        app.history.snapshot(&app.state.project);
        app.state.project.q0rgs[0].layers[0].placements[0]
            .transform
            .sx = 2.0;
        app.state.project.q0rgs[0].layers[0].placements[0]
            .transform
            .sy = 1.5;

        app.handle(&Context::default(), Action::Undo);

        assert_eq!(
            app.state.project.q0rgs[0].layers[0].placements[0].transform,
            Transform2D::IDENTITY,
        );
        assert_eq!(
            app.session.selection,
            Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            },
            "undoing a transform must not deselect the same display object",
        );
    }

    #[test]
    fn undoing_selected_raw_scale_keeps_the_path_selected() {
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
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        let selected = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        app.session.selection = Selection::Path {
            q0rg_id: selected.q0rg_id,
            layer_id: selected.layer_id,
            placement_idx: selected.placement_idx,
            path_idx: selected.path_idx,
        };
        app.history.snapshot(&app.state.project);
        let Asset::Vector(vector) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == 77)
            .unwrap()
        else {
            unreachable!()
        };
        for anchor in &mut vector.paths[0].anchors {
            anchor.point.x *= 2.0;
            anchor.point.y *= 1.5;
            if let Some(handle) = &mut anchor.in_handle {
                handle.x *= 2.0;
                handle.y *= 1.5;
            }
            if let Some(handle) = &mut anchor.out_handle {
                handle.x *= 2.0;
                handle.y *= 1.5;
            }
        }

        app.handle(&Context::default(), Action::Undo);

        assert_eq!(
            app.session.selection,
            Selection::Path {
                q0rg_id: selected.q0rg_id,
                layer_id: selected.layer_id,
                placement_idx: selected.placement_idx,
                path_idx: selected.path_idx,
            },
            "undoing a raw geometry transform must keep the same path selected",
        );
        let Asset::Vector(restored) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == 77)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(restored.paths[0], test_square(0.0, 20.0));
    }
    #[test]
    fn undo_after_break_apart_does_not_reinterpret_path_selection_as_holes() {
        let mut app = EditorApp::default();
        let mut hole = test_square(5.0, 15.0);
        hole.anchors.reverse();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0), hole],
            fill: Some(q0s_format::v2::Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 50.0,
                    ty: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };

        app.handle(&Context::default(), Action::BreakApartSelection);
        assert!(matches!(app.session.selection, Selection::Paths(_)));

        app.handle(&Context::default(), Action::Undo);

        assert!(
            matches!(app.session.selection, Selection::None),
            "undo must invalidate path indices whose backing placement/topology changed"
        );
        let restored = &app.state.project.q0rgs[0].layers[0].placements[0];
        assert_eq!(restored.target, Target::Asset(77));
        assert_eq!(restored.transform.tx, 50.0);
        assert_eq!(restored.transform.ty, 25.0);
    }

    #[test]
    fn break_apart_refuses_to_silently_destroy_instance_fx() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: q0s_format::v2::PlacementFx {
                    glow: Some(q0s_format::v2::GlowFx {
                        color: q0s_format::v2::Rgba {
                            r: 255,
                            g: 30,
                            b: 10,
                            a: 255,
                        },
                        radius_x: 10.0,
                        radius_y: 10.0,
                        strength: 1.25,
                        quality: 3,
                    }),
                    ..Default::default()
                },
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let before = app.state.project.clone();

        assert!(!app.can_break_apart_selection());
        app.handle(&Context::default(), Action::BreakApartSelection);

        assert_eq!(app.state.project, before);
        assert!(app.session.status.contains("FX"));
        assert!(app.session.status.contains("losslessly"));
    }

    #[test]
    fn break_apart_refuses_when_child_symbol_contains_active_fx() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "FX child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Art".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(77),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: q0s_format::v2::PlacementFx {
                        blur: Some(q0s_format::v2::BlurFx {
                            radius_x: 6.0,
                            radius_y: 6.0,
                            quality: 3,
                        }),
                        ..Default::default()
                    },
                }],
            }],
        });
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let before = app.state.project.clone();

        assert!(!app.can_break_apart_selection());
        app.handle(&Context::default(), Action::BreakApartSelection);

        assert_eq!(app.state.project, before);
        assert!(app.session.status.contains("active FX"));
        assert!(app.session.status.contains("never silently destroyed"));
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
                timeline_frame_count: None,
                name: "Art".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(77),
                    transform: Transform2D {
                        tx: 10.0,
                        ty: 5.0,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        });
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 50.0,
                    ty: 20.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        #[cfg(feature = "appearance-mask-eraser")]
        let expected_glow = {
            app.state
                .project
                .asset_appearances
                .insert(77, test_glow_appearance());
            let Asset::Vector(source) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == 77)
                .unwrap()
            else {
                unreachable!()
            };
            let appearance = app.state.project.asset_appearances.get(&77).unwrap();
            let composed = Affine::compose(
                Affine::from_transform(
                    app.state.project.q0rgs[0].layers[0].placements[0].transform,
                ),
                Affine::from_transform(
                    app.state.project.q0rgs[1].layers[0].placements[0].transform,
                ),
            );
            crate::appearance::transform_surface(
                &crate::appearance::visible_material_surface_for_vector(source, Some(appearance)),
                composed,
            )
        };
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
        #[cfg(feature = "appearance-mask-eraser")]
        {
            let appearance = app
                .state
                .project
                .asset_appearances
                .get(&new_id)
                .expect("symbol break apart must preserve child vector glow metadata");
            let actual_glow =
                crate::appearance::visible_material_surface_for_vector(vector, Some(appearance));
            let mismatch = expected_glow.difference(&actual_glow).unsigned_area()
                + actual_glow.difference(&expected_glow).unsigned_area();
            assert!(
                mismatch < 0.01,
                "symbol break apart changed the visible glow support: symmetric difference area={mismatch}"
            );
        }
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));
    }

    #[test]
    fn placing_vector_from_library_reuses_the_existing_asset() {
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
        let asset_count_before = app.state.project.assets.len();
        let drop = Vec2::new(100.0, 80.0);

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemAt(LibraryItem::Asset(77), drop),
        );

        assert_eq!(
            app.state.project.assets.len(),
            asset_count_before,
            "placing a library vector must not create another library vector"
        );
        let layer = &app.state.project.q0rgs[0].layers[0];
        let placement = layer.placements.last().expect("placed vector instance");
        assert_eq!(placement.target, Target::Asset(77));
        assert!((placement.transform.tx - 90.0).abs() < 0.001);
        assert!((placement.transform.ty - 70.0).abs() < 0.001);
        assert!(matches!(
            app.session.selection,
            Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0
            }
        ));
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
                instance_id: 0,
                frame: 12,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });

        app.handle(&Context::default(), Action::SetQ0rgFrameCount(1, 8));

        assert_eq!(app.state.project.q0rgs[0].frame_count, 24);
        assert!(matches!(
            app.pending_frame_truncate,
            Some((1, 8, 1, 0, 0, 0))
        ));
        assert!(!app.state.dirty);
    }

    #[test]
    fn frame_label_add_rename_remove_is_validated_and_undoable() {
        let mut app = EditorApp::default();
        let ctx = Context::default();
        app.handle(&ctx, Action::SetFrameLabel(1, 3, Some(" intro ".into())));
        assert_eq!(app.state.project.runtime.frame_label(1, 3), Some("intro"));
        app.handle(&ctx, Action::SetFrameLabel(1, 5, Some("intro".into())));
        assert_eq!(app.state.project.runtime.frame_label(1, 5), None);
        assert!(app.session.status.contains("already exists"));
        app.handle(&ctx, Action::SetFrameLabel(1, 3, Some("ending".into())));
        assert_eq!(
            app.state.project.runtime.resolve_frame_label(1, "intro"),
            None
        );
        assert_eq!(
            app.state.project.runtime.resolve_frame_label(1, "ending"),
            Some(3)
        );
        app.handle(&ctx, Action::SetFrameLabel(1, 3, None));
        assert_eq!(app.state.project.runtime.frame_label(1, 3), None);
        app.handle(&ctx, Action::Undo);
        assert_eq!(app.state.project.runtime.frame_label(1, 3), Some("ending"));
        app.handle(&ctx, Action::Undo);
        assert_eq!(app.state.project.runtime.frame_label(1, 3), Some("intro"));
    }

    #[test]
    fn shortening_over_labels_waits_for_confirmation_then_removes_them() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_labels
            .insert(q0s_format::v2::FrameLabelKey::new(1, 12), "outro".into());
        app.handle(&Context::default(), Action::SetQ0rgFrameCount(1, 8));
        assert!(app.pending_frame_truncate.is_some());
        assert_eq!(app.state.project.runtime.frame_label(1, 12), Some("outro"));
        app.apply_frame_count(1, 8);
        assert!(app.state.project.runtime.frame_labels.is_empty());
        app.handle(&Context::default(), Action::Undo);
        assert_eq!(app.state.project.runtime.frame_label(1, 12), Some("outro"));
    }

    #[test]
    fn preview_resolves_labels_in_current_symbol_and_reports_missing_targets() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_labels
            .insert(q0s_format::v2::FrameLabelKey::new(1, 4), "intro".into());
        let mut budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        app.run_preview_q0lang_source_with_budget("gostop! \"intro\"", &mut budget);
        assert_eq!(app.session.current_frame, 4);
        assert!(!app.session.playing);
        assert!(app.session.preview_script_diagnostics.is_empty());
        app.run_preview_q0lang_source_with_budget("gorun! \"unknown\"", &mut budget);
        assert_eq!(app.session.current_frame, 4);
        assert!(!app.session.playing);
        assert!(app
            .session
            .preview_script_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown frame label")));
    }
    #[test]
    fn deleting_symbol_removes_instances_and_is_undoable() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddQ0rg);
        assert!(app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 2));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
        assert!(app.state.project.q0rgs[0]
            .layers
            .iter()
            .filter(|layer| layer.layer_id != folder_id)
            .all(|layer| layer.is_blank_keyframe(0)));
        assert!(crate::tools::materialize_layer_keyframe_for_edit(
            &mut app.state.project,
            1,
            folder_id,
            0,
        )
        .is_none());

        q0s_format::v2::validate(&app.state.project).expect("folder project");
        app.request_layer_delete(1, folder_id);
        assert_eq!(app.pending_layer_delete, Some((1, folder_id, 3, 2)));
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
                timeline_frame_count: None,
                name: "a low".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "a high".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 10,
                timeline_frame_count: None,
                name: "folder a".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "b child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 20,
                timeline_frame_count: None,
                name: "folder b".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 5,
                timeline_frame_count: None,
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
                    mask_layer_id: None,
                    collapsed: false,
                    hidden: false,
                    locked: false,
                },
            );
        }
        for (child_id, folder_id) in [(1, 10), (2, 10), (3, 20)] {
            app.state.project.layer_metadata.insert(
                LayerKey::new(1, child_id),
                LayerMetadata {
                    kind: LayerKind::Normal,
                    parent_folder_id: Some(folder_id),
                    mask_layer_id: None,
                    collapsed: false,
                    hidden: false,
                    locked: false,
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
    fn nested_folders_can_be_created_moved_as_subtrees_and_cannot_form_cycles() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddLayer);
        let outside_layer = app.session.current_layer_id;

        app.session.current_layer_id = 1;
        app.handle(&Context::default(), Action::AddLayerFolder);
        let outer_folder = app.session.current_layer_id;
        app.handle(&Context::default(), Action::AddLayerFolder);
        let inner_folder = app.session.current_layer_id;
        app.handle(&Context::default(), Action::AddLayer);
        let nested_leaf = app.session.current_layer_id;
        app.handle(
            &Context::default(),
            Action::DropLayer(1, nested_leaf, LayerDropTarget::IntoFolder(inner_folder)),
        );

        assert_eq!(
            app.state.project.layer_parent_folder(1, inner_folder),
            Some(outer_folder)
        );
        assert_eq!(
            app.state.project.layer_parent_folder(1, nested_leaf),
            Some(inner_folder)
        );
        assert_eq!(app.state.project.layer_folder_depth(1, nested_leaf), 2);
        q0s_format::v2::validate(&app.state.project).expect("created nested folder tree");

        app.handle(
            &Context::default(),
            Action::DropLayer(
                1,
                outer_folder,
                LayerDropTarget::Before {
                    layer_id: outside_layer,
                    parent_folder_id: None,
                },
            ),
        );
        assert_eq!(
            app.state.project.layer_parent_folder(1, inner_folder),
            Some(outer_folder)
        );
        assert_eq!(
            app.state.project.layer_parent_folder(1, nested_leaf),
            Some(inner_folder)
        );
        let outer_range = app
            .layer_row_block_range(1, outer_folder)
            .expect("outer subtree range");
        assert_eq!(outer_range.1 - outer_range.0, 3);
        q0s_format::v2::validate(&app.state.project).expect("moved nested folder subtree");

        let before_cycle_attempt = app.state.project.clone();
        app.handle(
            &Context::default(),
            Action::DropLayer(1, outer_folder, LayerDropTarget::IntoFolder(inner_folder)),
        );
        assert_eq!(app.state.project, before_cycle_attempt);
        q0s_format::v2::validate(&app.state.project).expect("cycle attempt rejected");
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
    fn delete_selection_removes_subselect_point_keeps_two_point_open_path_and_invalidates_render() {
        let mut app = EditorApp::default();
        let q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        let layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        let asset_id = 61000;
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(0.0, 0.0),
                        in_handle: None,
                        out_handle: Some(Vec2::new(2.0, 50.0)),
                    },
                    Anchor {
                        point: Vec2::new(10.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, 0.0),
                        in_handle: Some(Vec2::new(18.0, -50.0)),
                        out_handle: None,
                    },
                ],
                closed: false,
            }],
            fill: None,
            stroke: Some(q0s_format::v2::Stroke {
                color: q0s_format::v2::Rgba {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 255,
                },
                width: 2.0,
                cap: q0s_format::geom::CapShape::Round,
            }),
        }));
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.current_q0rg_id = q0rg_id;
        app.session.current_layer_id = layer_id;
        app.session.current_frame = 0;

        let ctx = Context::default();
        let rect = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 100.0));
        let view = crate::render::StageView {
            origin: egui::pos2(20.0, 20.0),
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("subselect-delete-cache-before"),
                ));
                crate::render::render_stage(
                    &painter,
                    &app.state.project,
                    q0rg_id,
                    0,
                    &view,
                    &mut app.textures,
                    ctx,
                );
            },
        );
        let builds_before = app.textures.vector_render_geometry_build_count();
        assert_eq!(builds_before, 1);

        app.session.selection = Selection::SubselectPoints {
            path: PathRef {
                q0rg_id,
                layer_id,
                placement_idx: 0,
                path_idx: 0,
            },
            anchor_indices: vec![1],
            bounds_min: Vec2::new(10.0, 0.0),
            bounds_max: Vec2::new(10.0, 0.0),
        };
        app.handle(&ctx, Action::DeleteSelection);

        let path = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => &vector.paths[0],
            _ => unreachable!(),
        };
        assert_eq!(path.anchors.len(), 2);
        assert_eq!(path.anchors[0].point, Vec2::new(0.0, 0.0));
        assert_eq!(path.anchors[1].point, Vec2::new(20.0, 0.0));
        assert_eq!(path.anchors[0].out_handle, None);
        assert_eq!(path.anchors[1].in_handle, None);
        assert!(matches!(app.session.selection, Selection::None));
        q0s_format::v2::validate(&app.state.project).expect("two-point open path remains valid");

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("subselect-delete-cache-after"),
                ));
                crate::render::render_stage(
                    &painter,
                    &app.state.project,
                    q0rg_id,
                    0,
                    &view,
                    &mut app.textures,
                    ctx,
                );
            },
        );
        assert_eq!(
            app.textures.vector_render_geometry_build_count(),
            builds_before + 1,
            "Delete mutated authored anchors but left the stale vector render cache warm"
        );
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
        assert!(app.state.project.q0rgs[0]
            .layers
            .iter()
            .filter(|layer| layer.layer_id != folder_id)
            .all(|layer| layer.is_blank_keyframe(0)));
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
    fn audio_only_library_item_cannot_be_placed_on_stage() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: (test_audio_q0v_bytes(48_000, 4_800)).into(),
        }));
        let before: usize = app.state.project.q0rgs[0]
            .layers
            .iter()
            .map(|layer| layer.placements.len())
            .sum();
        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemAt(LibraryItem::Asset(77), Vec2::new(120.0, 80.0)),
        );
        let after: usize = app.state.project.q0rgs[0]
            .layers
            .iter()
            .map(|layer| layer.placements.len())
            .sum();
        assert_eq!(after, before, "audio must never become a stage object");
        assert!(matches!(app.session.selection, Selection::None));
        assert!(app.session.status.contains("timeline"));
    }

    #[test]
    fn audio_timeline_drop_stays_on_target_layer_without_creating_keyframes() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 24;
        app.state.project.q0rgs[0].frame_count = 4;
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: (test_audio_q0v_bytes(48_000, 48_000)).into(),
        }));
        let layer_count_before = app.state.project.q0rgs[0].layers.len();

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(77), 1, 2),
        );

        assert_eq!(app.state.project.q0rgs[0].layers.len(), layer_count_before);
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(layer.layer_id, 1);
        assert!(layer.placements.is_empty());
        assert!(layer.explicit_keyframes.is_empty());
        assert_eq!(app.state.project.audio_clips.len(), 1);
        let clip = app.state.project.audio_clips[0];
        assert_eq!(clip.q0rg_id, 1);
        assert_eq!(clip.layer_id, 1);
        assert_eq!(clip.start_frame, 2);
        assert_eq!(clip.asset_id, 77);
        assert!(matches!(app.session.selection, Selection::None));
        assert_eq!(
            app.session.timeline_selection,
            Some(crate::state::TimelineSelection::single(1, 2))
        );
        assert_eq!(app.state.project.q0rgs[0].frame_count, 26);
        q0s_format::v2::validate(&app.state.project).expect("audio timeline project");
    }

    #[test]
    fn audio_timeline_range_rejects_visual_and_blank_keyframes() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 24;
        app.state.project.q0rgs[0].frame_count = 30;
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: (test_audio_q0v_bytes(48_000, 48_000)).into(),
        }));
        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(77), 1, 2),
        );

        app.session.current_layer_id = 1;
        app.session.current_frame = 8;
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(1, 8));
        app.handle(&Context::default(), Action::InsertKeyframe);
        app.handle(&Context::default(), Action::InsertBlankKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(!layer.has_keyframe(8));
        assert!(layer
            .placements
            .iter()
            .all(|placement| placement.frame != 8));
        assert!(app.session.status.contains("audio frames"));
        q0s_format::v2::validate(&app.state.project).expect("audio keyframe guard project");
    }

    #[test]
    fn frame_clipboard_cannot_paste_visual_content_into_audio_range() {
        let mut app = app_with_one_timeline_object(30);
        app.state.project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "audio row".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: None,
            stroke: None,
        }));
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 78,
            bytes: (test_audio_q0v_bytes(48_000, 48_000)).into(),
        }));
        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(78), 2, 2),
        );

        app.session.current_layer_id = 1;
        app.session.current_frame = 0;
        app.session.timeline_selection = Some(TimelineSelection::single(1, 0));
        app.handle(&Context::default(), Action::CopySelection);

        app.session.current_layer_id = 2;
        app.session.current_frame = 8;
        app.session.timeline_selection = Some(TimelineSelection::single(2, 8));
        app.handle(&Context::default(), Action::Paste);

        let audio_layer = app.state.project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == 2)
            .expect("audio layer");
        assert!(audio_layer.placements.is_empty());
        assert!(!audio_layer.has_keyframe(8));
        assert!(app.session.status.contains("audio range"));
        assert_eq!(app.state.project.audio_clips.len(), 1);
        q0s_format::v2::validate(&app.state.project).expect("audio clipboard guard project");
    }

    #[test]
    fn remove_frame_columns_shift_or_remove_timeline_audio_clips_safely() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 24;
        app.state.project.q0rgs[0].frame_count = 60;
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: (test_audio_q0v_bytes(48_000, 48_000)).into(),
        }));
        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(77), 1, 20),
        );
        assert_eq!(app.state.project.audio_clips[0].start_frame, 20);

        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 4,
        });
        app.session.current_frame = 4;
        app.handle(&Context::default(), Action::RemoveFrame);
        assert_eq!(app.state.project.audio_clips.len(), 1);
        assert_eq!(app.state.project.audio_clips[0].start_frame, 17);
        q0s_format::v2::validate(&app.state.project).expect("shifted audio clip project");

        app.session.timeline_selection = Some(TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 18,
            focus_layer_id: 1,
            focus_frame: 19,
        });
        app.session.current_frame = 19;
        app.handle(&Context::default(), Action::RemoveFrame);
        assert!(app.state.project.audio_clips.is_empty());
        q0s_format::v2::validate(&app.state.project).expect("cut audio clip project");
    }

    #[test]
    fn q0v_timeline_drop_creates_an_independent_finite_video_layer() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 20;
        app.state.project.q0rgs[0].frame_count = 20;
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: (test_q0v_bytes(10, 4)).into(),
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
            bytes: (test_q0v_bytes(24, 5)).into(),
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
    #[test]
    fn layer_visibility_and_lock_actions_persist_state_and_protect_frame_edits() {
        let mut app = EditorApp::default();
        let q0rg_id = app.session.current_q0rg_id;
        let layer_id = app.session.current_layer_id;

        assert!(app.state.project.layer_is_visible(q0rg_id, layer_id));
        assert!(!app.state.project.layer_is_locked(q0rg_id, layer_id));

        app.handle(
            &Context::default(),
            Action::ToggleLayerVisibility(q0rg_id, layer_id),
        );
        assert!(app.state.project.layer_metadata(q0rg_id, layer_id).hidden);
        assert!(!app.state.project.layer_is_visible(q0rg_id, layer_id));

        app.handle(
            &Context::default(),
            Action::ToggleLayerVisibility(q0rg_id, layer_id),
        );
        assert!(app.state.project.layer_is_visible(q0rg_id, layer_id));

        app.handle(
            &Context::default(),
            Action::ToggleLayerLock(q0rg_id, layer_id),
        );
        assert!(app.state.project.layer_is_locked(q0rg_id, layer_id));
        let before = app.state.project.clone();
        app.handle(&Context::default(), Action::InsertBlankKeyframe);
        assert_eq!(
            app.state.project, before,
            "locked layer must reject frame edits"
        );
        assert_eq!(app.session.status, "layer is locked");

        app.handle(
            &Context::default(),
            Action::ToggleLayerLock(q0rg_id, layer_id),
        );
        assert!(!app.state.project.layer_is_locked(q0rg_id, layer_id));
    }

    fn preview_runtime_var(app: &EditorApp, name: &str) -> Option<String> {
        app.session
            .preview_q0lang_runtime
            .vars
            .get(name)
            .map(|value| value.display_lossy())
    }

    #[test]
    fn editor_preview_runs_frame_zero_after_runtime_init() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_scripts
            .push(q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 0,
                source: "init = 7\n".into(),
            });
        app.initialize_preview_q0lang_if_needed();
        assert_eq!(preview_runtime_var(&app, "init").as_deref(), Some("7"));
        assert_eq!(app.session.preview_frame_script_entries, 1);
    }

    #[test]
    fn editor_scrub_does_not_execute_frame_code() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_scripts
            .push(q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 5,
                source: "scrubbed = 1\n".into(),
            });
        app.initialize_preview_q0lang_if_needed();
        app.go_to_frame(5);
        assert_eq!(app.session.current_frame, 5);
        assert!(preview_runtime_var(&app, "scrubbed").is_none());
    }

    #[test]
    fn editor_playback_executes_each_entered_frame_script() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_scripts
            .push(q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 1,
                source: "entered = 1\n".into(),
            });
        app.session.playing = true;
        app.initialize_preview_q0lang_if_needed();
        app.session.last_tick = Instant::now() - Duration::from_secs_f32(1.1 / 24.0);
        app.tick_playback();
        assert_eq!(app.session.current_frame, 1);
        assert_eq!(preview_runtime_var(&app, "entered").as_deref(), Some("1"));
    }

    #[test]
    fn editor_runtime_goto_executes_destination_and_skips_intermediate_scripts() {
        let mut app = EditorApp::default();
        app.state.project.runtime.frame_scripts.extend([
            q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 1,
                source: "gorun! 3\n".into(),
            },
            q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 2,
                source: "middle = 1\n".into(),
            },
            q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 3,
                source: "destination = 1\n".into(),
            },
        ]);
        let mut budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        app.preview_runtime_transition_to_frame(1, &mut budget);
        assert_eq!(app.session.current_frame, 3);
        assert!(preview_runtime_var(&app, "middle").is_none());
        assert_eq!(
            preview_runtime_var(&app, "destination").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn editor_preview_self_jump_stops_at_transition_budget() {
        let mut app = EditorApp::default();
        app.state
            .project
            .runtime
            .frame_scripts
            .push(q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 1,
                source: "gorun! 1\n".into(),
            });
        let mut budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        app.preview_runtime_transition_to_frame(1, &mut budget);
        assert_eq!(app.session.current_frame, 1);
        assert!(app
            .session
            .preview_script_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("transition budget exceeded")));
    }

    #[test]
    fn editor_preview_orders_same_frame_scripts_by_layer() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "second".into(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        app.state.project.runtime.frame_scripts.extend([
            q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 2,
                frame: 1,
                source: "order = 2\n".into(),
            },
            q0s_format::v2::FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 1,
                source: "order = 1\n".into(),
            },
        ]);
        let mut budget = MAX_PREVIEW_FRAME_SCRIPT_TRANSITIONS;
        app.preview_runtime_transition_to_frame(1, &mut budget);
        assert_eq!(preview_runtime_var(&app, "order").as_deref(), Some("2"));
    }

    #[test]
    fn camera_f6_tween_delete_and_undo_never_mutate_artwork_layers() {
        let mut app = EditorApp::default();
        let ctx = Context::default();
        let artwork_before = app.state.project.q0rgs.clone();
        app.state.project.runtime.camera.enabled = true;
        app.session.camera_track_selected = true;

        app.handle(&ctx, Action::InsertKeyframe);
        assert!(app.state.project.runtime.camera.has_keyframe(0));

        app.session.current_frame = 10;
        app.handle(&ctx, Action::InsertKeyframe);
        assert!(app.state.project.runtime.camera.has_keyframe(10));

        app.session.current_frame = 5;
        app.handle(&ctx, Action::CreateMotionTween);
        let source = app
            .state
            .project
            .runtime
            .camera
            .keys
            .iter()
            .find(|key| key.frame == 0)
            .expect("camera source key");
        assert_eq!(source.tween.to_frame(), Some(10));

        app.session.current_frame = 10;
        app.handle(&ctx, Action::DeleteSelection);
        assert!(!app.state.project.runtime.camera.has_keyframe(10));
        assert_eq!(app.state.project.runtime.camera.keys[0].tween, Tween::None);
        assert_eq!(app.state.project.q0rgs, artwork_before);

        app.handle(&ctx, Action::Undo);
        assert!(app.state.project.runtime.camera.has_keyframe(10));
        assert_eq!(app.state.project.q0rgs, artwork_before);
    }

    #[test]
    fn camera_frame_range_moves_and_clears_without_becoming_an_artwork_layer() {
        let mut app = EditorApp::default();
        let ctx = Context::default();
        app.state.project.q0rgs[0].frame_count = 20;
        let artwork_before = app.state.project.q0rgs.clone();
        app.state.project.runtime.camera.enabled = true;
        app.state.project.runtime.camera.keys = vec![
            q0s_format::v2::CameraKey {
                frame: 2,
                state: app.state.project.default_camera_state(),
                tween: Tween::Linear { to_frame: 6 },
            },
            q0s_format::v2::CameraKey {
                frame: 6,
                state: q0s_format::v2::CameraState {
                    center_x: 400.0,
                    ..app.state.project.default_camera_state()
                },
                tween: Tween::None,
            },
        ];
        app.session.camera_track_selected = true;
        let selection = CameraTimelineSelection {
            anchor_frame: 2,
            focus_frame: 6,
        };
        app.session.camera_timeline_selection = Some(selection);

        app.handle(&ctx, Action::MoveCameraFrames(selection, 8));

        assert_eq!(
            app.state
                .project
                .runtime
                .camera
                .keys
                .iter()
                .map(|key| key.frame)
                .collect::<Vec<_>>(),
            vec![8, 12]
        );
        assert_eq!(
            app.state.project.runtime.camera.keys[0].tween.to_frame(),
            Some(12)
        );
        assert_eq!(app.state.project.q0rgs, artwork_before);
        assert_eq!(
            app.session.camera_timeline_selection,
            Some(CameraTimelineSelection {
                anchor_frame: 8,
                focus_frame: 12,
            })
        );

        app.handle(&ctx, Action::DeleteSelection);
        assert!(app.state.project.runtime.camera.keys.is_empty());
        assert_eq!(app.state.project.q0rgs, artwork_before);
        q0s_format::v2::validate(&app.state.project).unwrap();
    }
}
