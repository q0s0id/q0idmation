//! Application-wide panel docking. Layouts contain UI state only, never project data.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{Context, Id, Rect, Ui, WidgetText};
use egui_dock::{
    DockArea, DockState, Node, NodeIndex, Split, Surface, SurfaceIndex, TabIndex, TabViewer, Tree,
};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::settings::Settings;
use crate::ui::FitToScreen;

const MAX_PRESETS: usize = 32;
const SAVE_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Panel {
    Stage,
    Tools,
    Timeline,
    Library,
    Properties,
}

impl Panel {
    const ALL: [Self; 5] = [
        Self::Stage,
        Self::Tools,
        Self::Timeline,
        Self::Library,
        Self::Properties,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Tools => "Tools",
            Self::Timeline => "Timeline",
            Self::Library => "Library",
            Self::Properties => "Properties",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PanelTab {
    panel: Panel,
    id: u64,
}
impl PanelTab {
    fn primary(panel: Panel) -> Self {
        Self {
            panel,
            id: panel as u64 + 1,
        }
    }
}
const MAX_PANEL_TABS: usize = 64;

#[derive(Clone)]
struct PanelViewState {
    library_search: String,
    library_sort: bool,
    timeline_scroll: f32,
}
impl PanelViewState {
    fn capture(session: &crate::state::Session) -> Self {
        Self {
            library_search: session.library_search.clone(),
            library_sort: session.library_sort_ascending,
            timeline_scroll: session.timeline_horizontal_scroll,
        }
    }
    fn install(&self, session: &mut crate::state::Session, panel: Panel) {
        match panel {
            Panel::Library => {
                session.library_search.clone_from(&self.library_search);
                session.library_sort_ascending = self.library_sort;
            }
            Panel::Timeline => session.timeline_horizontal_scroll = self.timeline_scroll,
            _ => {}
        }
    }
}

#[derive(Clone)]
struct StagePaneState {
    viewport: crate::state::Viewport,
    q0rg: u16,
    layer: u16,
    frame: u16,
    breadcrumb: Vec<u16>,
    edit_in_place: Vec<crate::state::EditInPlaceContext>,
    selection: crate::state::Selection,
    pivot: Option<crate::state::TransformPivot>,
    subselect_more: Vec<(crate::state::PathRef, Vec<usize>)>,
    cursor: Option<q0s_format::v2::Vec2>,
    outline_mode: bool,
}
impl StagePaneState {
    fn capture(session: &crate::state::Session) -> Self {
        Self {
            viewport: session.viewport,
            q0rg: session.current_q0rg_id,
            layer: session.current_layer_id,
            frame: session.current_frame,
            breadcrumb: session.breadcrumb.clone(),
            edit_in_place: session.edit_in_place.clone(),
            selection: session.selection.clone(),
            pivot: session.transform_pivot.clone(),
            subselect_more: session.subselect_more.clone(),
            cursor: session.stage_cursor,
            outline_mode: session.outline_mode,
        }
    }
    fn install(&self, session: &mut crate::state::Session, project: &q0s_format::v2::ProjectV2) {
        let q0rg = project.q0rgs.iter().find(|q| q.q0rg_id == self.q0rg);
        if let Some(q0rg) = q0rg {
            session.current_q0rg_id = self.q0rg;
            session.current_layer_id = q0rg
                .layers
                .iter()
                .find(|l| l.layer_id == self.layer)
                .or_else(|| q0rg.layers.first())
                .map_or(1, |l| l.layer_id);
            session.current_frame = self.frame.min(q0rg.frame_count.saturating_sub(1));
            session.breadcrumb.clone_from(&self.breadcrumb);
            session.edit_in_place.clone_from(&self.edit_in_place);
            session.selection.clone_from(&self.selection);
            session.transform_pivot.clone_from(&self.pivot);
            session.subselect_more.clone_from(&self.subselect_more);
        } else {
            session.current_q0rg_id = project.meta.entry_q0rg_id;
            session.current_frame = 0;
            session.current_layer_id = project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == project.meta.entry_q0rg_id)
                .and_then(|q| q.layers.first())
                .map_or(1, |layer| layer.layer_id);
            session.breadcrumb.clear();
            session.edit_in_place.clear();
            session.selection = crate::state::Selection::None;
            session.transform_pivot = None;
            session.subselect_more.clear();
        }
        session.viewport = self.viewport;
        session.stage_cursor = self.cursor;
        session.outline_mode = self.outline_mode;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutPreset {
    pub name: String,
    pub layout: serde_json::Value,
}

// Persist a small, versioned topology instead of the dock library's transient
// rectangles, focus indices and drag state (including infinite unset rectangles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SavedLayout {
    version: u8,
    main: Option<LayoutNode>,
    floating: Vec<FloatingPanel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LayoutNode {
    Tabs {
        panels: Vec<Panel>,
        active: usize,
    },
    Split {
        vertical: bool,
        fraction: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FloatingPanel {
    tree: LayoutNode,
    position: [f32; 2],
    size: [f32; 2],
}

impl SavedLayout {
    fn decode(value: &serde_json::Value) -> Option<Self> {
        let layout: Self = serde_json::from_value(value.clone()).ok()?;
        if layout.version != 1 || layout.floating.len() > MAX_PANEL_TABS {
            return None;
        }
        let mut count = 0;
        if let Some(node) = &layout.main {
            if !node.valid(0, &mut count) {
                return None;
            }
        }
        for window in &layout.floating {
            if !window
                .position
                .iter()
                .chain(&window.size)
                .all(|v| v.is_finite() && v.abs() <= 100_000.0)
                || window.size.iter().any(|v| *v < 32.0)
                || !window.tree.valid(0, &mut count)
            {
                return None;
            }
        }
        Some(layout)
    }

    fn dock(&self) -> DockState<PanelTab> {
        let mut dock = DockState::new(Vec::new());
        let mut next_id = 1;
        if let Some(node) = &self.main {
            *dock.main_surface_mut() = node.tree(&mut next_id);
        }
        for window in &self.floating {
            let index = dock.add_window(vec![PanelTab::primary(Panel::Stage)]);
            dock[index] = window.tree.tree(&mut next_id);
            if let Some(state) = dock.get_window_state_mut(index) {
                state
                    .set_position(window.position.into())
                    .set_size(window.size.into());
            }
        }
        decorate(&mut dock);
        dock
    }
}

impl LayoutNode {
    fn valid(&self, depth: usize, count: &mut usize) -> bool {
        if depth > 16 {
            return false;
        }
        match self {
            Self::Tabs { panels, active } => {
                *count += panels.len();
                !panels.is_empty() && *count <= MAX_PANEL_TABS && *active < panels.len()
            }
            Self::Split {
                fraction,
                first,
                second,
                ..
            } => {
                fraction.is_finite()
                    && (0.01..=0.99).contains(fraction)
                    && first.valid(depth + 1, count)
                    && second.valid(depth + 1, count)
            }
        }
    }

    fn capture(tree: &Tree<PanelTab>, index: NodeIndex) -> Option<Self> {
        match tree.iter().nth(index.0)? {
            Node::Leaf { tabs, active, .. } => Some(Self::Tabs {
                panels: tabs.iter().map(|tab| tab.panel).collect(),
                active: active.0,
            }),
            Node::Horizontal { fraction, .. } | Node::Vertical { fraction, .. } => {
                Some(Self::Split {
                    vertical: matches!(tree[index], Node::Vertical { .. }),
                    fraction: fraction.clamp(0.01, 0.99),
                    first: Box::new(Self::capture(tree, NodeIndex(index.0 * 2 + 1))?),
                    second: Box::new(Self::capture(tree, NodeIndex(index.0 * 2 + 2))?),
                })
            }
            Node::Empty => None,
        }
    }

    fn tree(&self, next_id: &mut u64) -> Tree<PanelTab> {
        let mut tree = Tree::new(vec![PanelTab::primary(Panel::Stage)]);
        self.fill(&mut tree, NodeIndex::root(), next_id);
        tree
    }

    fn fill(&self, tree: &mut Tree<PanelTab>, index: NodeIndex, next_id: &mut u64) {
        match self {
            Self::Tabs { panels, active } => {
                tree[index] = Node::leaf_with(
                    panels
                        .iter()
                        .map(|panel| {
                            let tab = PanelTab {
                                panel: *panel,
                                id: *next_id,
                            };
                            *next_id += 1;
                            tab
                        })
                        .collect(),
                );
                tree.set_active_tab(index, TabIndex(*active));
                tree.set_focused_node(index);
            }
            Self::Split {
                vertical,
                fraction,
                first,
                second,
            } => {
                let [a, b] = tree.split(
                    index,
                    if *vertical {
                        Split::Below
                    } else {
                        Split::Right
                    },
                    *fraction,
                    Node::leaf(PanelTab::primary(Panel::Stage)),
                );
                first.fill(tree, a, next_id);
                second.fill(tree, b, next_id);
            }
        }
    }
}

fn decorate(dock: &mut DockState<PanelTab>) {
    dock.translations.tab_context_menu.close_button = "Hide panel".into();
    dock.translations.tab_context_menu.eject_button = "Float panel".into();
}

fn default_dock() -> DockState<PanelTab> {
    let mut dock = DockState::new(vec![PanelTab::primary(Panel::Stage)]);
    let tree = dock.main_surface_mut();
    let [top, _] = tree.split_below(
        NodeIndex::root(),
        0.76,
        vec![PanelTab::primary(Panel::Timeline)],
    );
    let [content, _] = tree.split_left(top, 0.045, vec![PanelTab::primary(Panel::Tools)]);
    let [_, right] = tree.split_right(content, 0.75, vec![PanelTab::primary(Panel::Library)]);
    tree.split_below(right, 0.48, vec![PanelTab::primary(Panel::Properties)]);
    decorate(&mut dock);
    dock
}

pub(crate) struct WorkspaceLayout {
    dock: DockState<PanelTab>,
    last_observed: Option<SavedLayout>,
    changed_at: Option<Instant>,
    restore_windows: bool,
    manager_open: bool,
    preset_name: String,
    selected_preset: Option<usize>,
    message: String,
    hidden: HashMap<Panel, (DockState<PanelTab>, Vec<Option<LayoutNode>>)>,
    stage_views: HashMap<(u64, u64), StagePaneState>,
    panel_views: HashMap<(u64, u64), PanelViewState>,
    active_stage: Option<u64>,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            dock: default_dock(),
            last_observed: None,
            changed_at: None,
            restore_windows: false,
            manager_open: false,
            preset_name: String::new(),
            selected_preset: None,
            message: String::new(),
            hidden: HashMap::new(),
            stage_views: HashMap::new(),
            panel_views: HashMap::new(),
            active_stage: None,
        }
    }
}

impl WorkspaceLayout {
    fn restore_default(&mut self) {
        self.dock = default_dock();
        self.last_observed = None;
        self.changed_at = Some(Instant::now());
        self.restore_windows = false;
        self.hidden.clear();
        self.stage_views.clear();
        self.panel_views.clear();
        self.active_stage = None;
    }

    pub(crate) fn from_settings(settings: &Settings) -> Self {
        let saved = SavedLayout::decode(&settings.workspace_layout);
        Self {
            dock: saved.as_ref().map_or_else(default_dock, SavedLayout::dock),
            last_observed: saved,
            restore_windows: true,
            ..Self::default()
        }
    }

    fn find_panel(&self, panel: Panel) -> Option<(SurfaceIndex, NodeIndex, TabIndex)> {
        self.dock.iter_all_tabs().find_map(|(_, tab)| {
            (tab.panel == panel)
                .then(|| self.dock.find_tab(tab))
                .flatten()
        })
    }

    fn new_panel(&mut self, panel: Panel, source: Option<PanelTab>) {
        if self.dock.iter_all_tabs().count() >= MAX_PANEL_TABS {
            return;
        }
        let id = self
            .dock
            .iter_all_tabs()
            .map(|(_, tab)| tab.id)
            .chain(self.stage_views.keys().map(|(_, id)| *id))
            .max()
            .unwrap_or(0)
            + 1;
        let tab = PanelTab { panel, id };
        if let Some((surface, node, _)) = source.and_then(|source| self.dock.find_tab(&source)) {
            self.dock[surface].set_focused_node(node);
            self.dock[surface].push_to_focused_leaf(tab);
        } else {
            self.dock.main_surface_mut().push_to_focused_leaf(tab);
        }
        if let Some(source) = source {
            let views: Vec<_> = self
                .stage_views
                .iter()
                .filter(|((_, id), _)| *id == source.id)
                .map(|((project, _), viewport)| ((*project, id), viewport.clone()))
                .collect();
            self.stage_views.extend(views);
            let views: Vec<_> = self
                .panel_views
                .iter()
                .filter(|((_, id), _)| *id == source.id)
                .map(|((project, _), view)| ((*project, id), view.clone()))
                .collect();
            self.panel_views.extend(views);
        }
        self.changed_at = Some(Instant::now());
    }

    pub(crate) fn toggle(&mut self, panel: Panel) {
        if let Some(location) = self.find_panel(panel) {
            let before = self.dock.clone();
            self.dock.remove_tab(location);
            while let Some(location) = self.find_panel(panel) {
                self.dock.remove_tab(location);
            }
            self.hidden.insert(panel, (before, topology(&self.dock)));
        } else {
            if let Some((before, after)) = self.hidden.remove(&panel) {
                if topology(&self.dock) == after {
                    self.dock = before;
                    self.changed_at = Some(Instant::now());
                    return;
                }
            }
            let tab = PanelTab {
                panel,
                id: self
                    .dock
                    .iter_all_tabs()
                    .map(|(_, tab)| tab.id)
                    .chain(self.stage_views.keys().map(|(_, id)| *id))
                    .max()
                    .unwrap_or(0)
                    + 1,
            };
            let tree = self.dock.main_surface_mut();
            if tree.is_empty() {
                tree.push_to_first_leaf(tab);
            } else {
                match panel {
                    Panel::Timeline => {
                        tree.split_below(NodeIndex::root(), 0.76, vec![tab]);
                    }
                    Panel::Tools => {
                        tree.split_left(NodeIndex::root(), 0.06, vec![tab]);
                    }
                    _ => {
                        tree.split_right(NodeIndex::root(), 0.75, vec![tab]);
                    }
                }
            }
        }
        self.changed_at = Some(Instant::now());
    }

    fn capture(&self, ctx: &Context) -> SavedLayout {
        let mut floating = Vec::new();
        for (index, surface) in self.dock.iter_surfaces().enumerate() {
            if let Surface::Window(tree, _) = surface {
                if let Some(node) = LayoutNode::capture(tree, NodeIndex::root()) {
                    // egui_dock's WindowState does not update its rect in 0.12.
                    // The actual egui area stores the position/size. Floating
                    // docks have no frame margins, so this is also the content size.
                    let id = Id::new(format!("window {:?}", SurfaceIndex(index)));
                    let rect = ctx
                        .memory(|m| m.area_rect(id))
                        .unwrap_or(Rect::from_min_size(
                            egui::pos2(80.0, 100.0),
                            egui::vec2(360.0, 400.0),
                        ));
                    let size = rect.size().max(egui::vec2(44.0, 100.0));
                    let round = |v: f32| (v * 2.0).round() / 2.0;
                    let pending = self
                        .restore_windows
                        .then(|| {
                            self.last_observed.as_ref().and_then(|saved| {
                                saved.floating.iter().find(|window| window.tree == node)
                            })
                        })
                        .flatten();
                    floating.push(FloatingPanel {
                        tree: node.clone(),
                        position: pending
                            .map_or([round(rect.min.x), round(rect.min.y)], |w| w.position),
                        size: pending.map_or([round(size.x), round(size.y)], |w| w.size),
                    });
                }
            }
        }
        SavedLayout {
            version: 1,
            main: LayoutNode::capture(self.dock.main_surface(), NodeIndex::root()),
            floating,
        }
    }

    fn apply(&mut self, saved: &SavedLayout) {
        self.dock = saved.dock();
        self.last_observed = None;
        self.restore_windows = true;
        self.changed_at = Some(Instant::now());
        self.hidden.clear();
        self.stage_views.clear();
        self.panel_views.clear();
        self.active_stage = None;
    }

    fn restore_window_bounds(&mut self, bounds: Rect) {
        if !self.restore_windows {
            return;
        }
        self.restore_windows = false;
        // Use saved topology for geometry; runtime nodes have not been drawn yet.
        if let Some(saved) = &self.last_observed {
            let mut windows = self.dock.iter_surfaces_mut().filter_map(|s| match s {
                Surface::Window(_, state) => Some(state),
                _ => None,
            });
            for (state, window) in windows.by_ref().zip(&saved.floating) {
                let size = egui::vec2(window.size[0], window.size[1])
                    .min(bounds.size().max(egui::Vec2::splat(100.0)));
                let max = (bounds.max - size).max(bounds.min);
                let pos = egui::pos2(window.position[0], window.position[1]).clamp(bounds.min, max);
                state.set_position(pos).set_size(size);
            }
        }
    }

    fn persist(&mut self, app: &mut EditorApp, ctx: &Context, force: bool) {
        let current = self.capture(ctx);
        if self.last_observed.as_ref() != Some(&current) {
            self.last_observed = Some(current.clone());
            self.changed_at = Some(Instant::now());
        }
        let Some(changed_at) = self.changed_at else {
            return;
        };
        if !force && (ctx.input(|i| i.pointer.any_down()) || changed_at.elapsed() < SAVE_DELAY) {
            ctx.request_repaint_after(SAVE_DELAY);
            return;
        }
        self.changed_at = None;
        let Ok(value) = serde_json::to_value(current) else {
            return;
        };
        if app.settings.workspace_layout == value {
            return;
        }
        app.settings.workspace_layout = value;
        if let Err(error) = app.settings.try_save() {
            self.message = format!("could not save layout: {error}");
            app.session.status = self.message.clone();
        }
    }

    pub(crate) fn save_on_exit(&self, settings: &mut Settings) {
        if let Some(saved) = &self.last_observed {
            if let Ok(value) = serde_json::to_value(saved) {
                if settings.workspace_layout != value {
                    settings.workspace_layout = value;
                    settings.save();
                }
            }
        }
    }
}

fn topology(dock: &DockState<PanelTab>) -> Vec<Option<LayoutNode>> {
    dock.iter_surfaces()
        .filter_map(|s| s.node_tree())
        .map(|tree| LayoutNode::capture(tree, NodeIndex::root()))
        .collect()
}

fn dock_at_edge(dock: &mut DockState<PanelTab>, panel: PanelTab, split: Split) {
    if let Some(source) = dock.find_tab(&panel) {
        if source.0.is_main() && dock.main_surface().num_tabs() == 1 {
            return;
        }
        // Remove before splitting the root: splitting a parent renumbers its
        // descendants, so move_tab's old source index would no longer be valid.
        dock.remove_tab(source);
        if dock.main_surface().is_empty() {
            dock.main_surface_mut().push_to_first_leaf(panel);
        } else {
            let fraction = if matches!(split, Split::Left | Split::Above) {
                0.25
            } else {
                0.75
            };
            dock.main_surface_mut()
                .split(NodeIndex::root(), split, fraction, Node::leaf(panel));
        }
    }
}

fn edge_at(bounds: Rect, pointer: egui::Pos2) -> Option<Split> {
    if !bounds.contains(pointer) {
        return None;
    }
    let nearest = [
        (pointer.x - bounds.left(), Split::Left),
        (bounds.right() - pointer.x, Split::Right),
        (pointer.y - bounds.top(), Split::Above),
        (bounds.bottom() - pointer.y, Split::Below),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))?;
    (nearest.0 <= 22.0).then_some(nearest.1)
}

struct Viewer<'a> {
    app: &'a mut EditorApp,
    bounds: Rect,
    edge_command: Option<(PanelTab, Split)>,
    edge_preview: Option<Split>,
    duplicate: Option<PanelTab>,
    stage_views: &'a mut HashMap<(u64, u64), StagePaneState>,
    panel_views: &'a mut HashMap<(u64, u64), PanelViewState>,
    active_stage: &'a mut Option<u64>,
}

impl TabViewer for Viewer<'_> {
    type Tab = PanelTab;

    fn title(&mut self, tab: &mut PanelTab) -> WidgetText {
        tab.panel.title().into()
    }
    fn id(&mut self, tab: &mut PanelTab) -> Id {
        Id::new(("workspace_panel", tab.id))
    }
    fn scroll_bars(&self, _: &PanelTab) -> [bool; 2] {
        [false, false]
    }
    /// The canvas gets every pixel of its panel, and the tool column nearly so.
    fn tab_style_override(
        &self,
        tab: &PanelTab,
        global_style: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        let margin = match tab.panel {
            Panel::Stage => 0.0,
            Panel::Tools => 2.0,
            _ => return None,
        };
        let mut style = global_style.clone();
        style.tab_body.inner_margin = egui::Margin::same(margin);
        Some(style)
    }
    fn on_tab_button(&mut self, panel: &mut PanelTab, response: &egui::Response) {
        response.clone().on_hover_text("Drag to a dock target to split or make tabs. Drop away from a target to float. Right-click for panel options.");
        if response.dragged() || response.drag_stopped() {
            if let Some(split) = response
                .interact_pointer_pos()
                .and_then(|p| edge_at(self.bounds, p))
            {
                self.edge_preview = Some(split);
                if response.drag_stopped() {
                    self.edge_command = Some((*panel, split));
                }
            }
        }
    }
    fn context_menu(&mut self, ui: &mut Ui, tab: &mut PanelTab, _: SurfaceIndex, _: NodeIndex) {
        if ui.button("duplicate panel").clicked() {
            self.duplicate = Some(*tab);
            ui.close_menu();
        }
        ui.menu_button("Dock to workspace edge", |ui| {
            for (label, split) in [
                ("Left", Split::Left),
                ("Right", Split::Right),
                ("Top", Split::Above),
                ("Bottom", Split::Below),
            ] {
                if ui.button(label).clicked() {
                    self.edge_command = Some((*tab, split));
                    ui.close_menu();
                }
            }
        });
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut PanelTab) {
        ui.push_id(tab.id, |ui| self.panel_ui(ui, tab));
    }
}

impl Viewer<'_> {
    fn panel_ui(&mut self, ui: &mut Ui, tab: &mut PanelTab) {
        let started = Instant::now();
        let app = &mut *self.app;
        if tab.panel == Panel::Tools {
            // A floating window must not inherit yesterday's full width as its
            // content minimum; otherwise its resize handle cannot shrink it.
            let rect = ui.available_rect_before_wrap();
            let mut contents = ui.child_ui(rect, *ui.layout());
            contents.set_clip_rect(ui.clip_rect().intersect(rect));
            egui::ScrollArea::vertical()
                .id_source("tools_scroll")
                .auto_shrink([false, false])
                .show(&mut contents, |ui| crate::panels::toolbar::render(app, ui));
            ui.allocate_space(egui::vec2(32.0, rect.height().max(0.0)));
            app.perf
                .record(crate::perf::PerfSpan::Toolbar, started.elapsed());
            return;
        }
        let view_key = (app.active_project_tab_id().unwrap_or(0), tab.id);
        let previous_view = PanelViewState::capture(&app.session);
        if matches!(tab.panel, Panel::Library | Panel::Timeline) {
            self.panel_views
                .entry(view_key)
                .or_insert_with(|| previous_view.clone())
                .install(&mut app.session, tab.panel);
        }
        crate::ui::panel_contents(ui, |ui| match tab.panel {
            Panel::Stage => {
                let project = app.active_project_tab_id().unwrap_or(0);
                let key = (project, tab.id);
                let hovered = ui.rect_contains_pointer(ui.available_rect_before_wrap());
                let can_activate = egui::DragAndDrop::has_payload_of_type::<
                    crate::state::LibraryDragPayload,
                >(ui.ctx())
                    || ui.input(|i| {
                        i.pointer.any_pressed()
                            || (!i.pointer.any_down() && i.smooth_scroll_delta != egui::Vec2::ZERO)
                    });
                if *self.active_stage != Some(tab.id)
                    && (self.active_stage.is_none()
                        || (hovered && can_activate && crate::text_tool::finish_edit(app)))
                {
                    if let Some(previous) = *self.active_stage {
                        self.stage_views
                            .insert((project, previous), StagePaneState::capture(&app.session));
                    }
                    let next = self
                        .stage_views
                        .entry(key)
                        .or_insert_with(|| StagePaneState::capture(&app.session))
                        .clone();
                    next.install(&mut app.session, &app.state.project);
                    app.session.reconcile_with(&app.state.project);
                    app.session.timeline_selection = None;
                    app.session.timeline_cell_selection = None;
                    app.session.timeline_layer_selection = None;
                    *self.active_stage = Some(tab.id);
                }
                let interactive = *self.active_stage == Some(tab.id);
                let active_view = StagePaneState::capture(&app.session);
                if !interactive {
                    let passive = self
                        .stage_views
                        .entry(key)
                        .or_insert_with(|| active_view.clone())
                        .clone();
                    passive.install(&mut app.session, &app.state.project);
                }
                crate::panels::stage::render_view(app, ui, interactive);
                self.stage_views
                    .insert(key, StagePaneState::capture(&app.session));
                if !interactive {
                    active_view.install(&mut app.session, &app.state.project);
                }
            }
            Panel::Tools => {
                egui::ScrollArea::vertical()
                    .id_source("tools_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| crate::panels::toolbar::render(app, ui));
            }
            Panel::Timeline => {
                crate::panels::timeline::render(app, ui);
                app.session
                    .clear_inactive_frame_selection(&app.state.project);
            }
            Panel::Library => crate::panels::library::render(app, ui),
            Panel::Properties => crate::panels::properties::render(app, ui),
        });
        if matches!(tab.panel, Panel::Library | Panel::Timeline) {
            self.panel_views
                .insert(view_key, PanelViewState::capture(&app.session));
            previous_view.install(&mut app.session, tab.panel);
        }
        let span = match tab.panel {
            Panel::Stage => return,
            Panel::Tools => crate::perf::PerfSpan::Toolbar,
            Panel::Timeline => crate::perf::PerfSpan::Timeline,
            Panel::Library => crate::perf::PerfSpan::Library,
            Panel::Properties => crate::perf::PerfSpan::Properties,
        };
        app.perf.record(span, started.elapsed());
    }
}

pub(crate) fn render(app: &mut EditorApp, ctx: &Context) {
    let mut workspace = std::mem::take(&mut app.workspace_layout);
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let bounds = ui.available_rect_before_wrap();
            workspace.restore_window_bounds(bounds);
            if let Some(active) = workspace.active_stage {
                workspace.stage_views.insert(
                    (app.active_project_tab_id().unwrap_or(0), active),
                    StagePaneState::capture(&app.session),
                );
                let visible = workspace
                    .dock
                    .iter_surfaces()
                    .filter_map(|surface| surface.node_tree())
                    .flat_map(|tree| tree.iter())
                    .any(|node| match node {
                        Node::Leaf {
                            tabs,
                            active: index,
                            ..
                        } => tabs.get(index.0).is_some_and(|tab| tab.id == active),
                        _ => false,
                    });
                if !visible {
                    workspace.active_stage = None;
                }
            }
            let mut style = egui_dock::Style::from_egui(ui.style());
            style.dock_area_padding = Some(egui::Margin::ZERO);
            style.tab_bar.height = 26.0;
            style.tab.tab_body.inner_margin = egui::Margin::same(3.0);
            // A thin line between panels, still easy to grab.
            style.separator.width = 2.0;
            style.separator.extra_interact_width = 8.0;
            // How narrow a docked panel may get. egui_dock's own 175 kept the
            // tool column from ever shrinking to one button per row, and
            // snapped it back wider whenever it was dragged.
            style.separator.extra = 40.0;
            style.tab.minimum_width = Some(24.0);
            style.overlay.selection_color =
                app.settings.theme.accent.to_color32().linear_multiply(0.35);
            style.overlay.overlay_type = egui_dock::OverlayType::Widgets;
            let before = workspace.dock.clone();
            let mut viewer = Viewer {
                app,
                bounds,
                edge_command: None,
                edge_preview: None,
                duplicate: None,
                stage_views: &mut workspace.stage_views,
                panel_views: &mut workspace.panel_views,
                active_stage: &mut workspace.active_stage,
            };
            // egui_dock 0.12 builds floating frames from this local Ui style.
            // Tabs already provide dragging and closing; keep their content flush
            // with the window edges while retaining egui's resize hit areas.
            ui.spacing_mut().window_margin = egui::Margin::ZERO;
            ui.visuals_mut().window_stroke = egui::Stroke::NONE;
            ui.visuals_mut().window_rounding = egui::Rounding::ZERO;
            DockArea::new(&mut workspace.dock)
                .id(Id::new("editor_workspace"))
                .style(style)
                .window_bounds(bounds)
                .show_window_close_buttons(false)
                .show_window_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
            if let Some(split) = viewer.edge_preview {
                let mut preview = bounds;
                match split {
                    Split::Left => preview.max.x = bounds.left() + bounds.width() * 0.25,
                    Split::Right => preview.min.x = bounds.right() - bounds.width() * 0.25,
                    Split::Above => preview.max.y = bounds.top() + bounds.height() * 0.25,
                    Split::Below => preview.min.y = bounds.bottom() - bounds.height() * 0.25,
                }
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    Id::new("workspace_edge_preview"),
                ));
                painter.rect_filled(
                    preview,
                    3.0,
                    viewer
                        .app
                        .settings
                        .theme
                        .accent
                        .to_color32()
                        .linear_multiply(0.2),
                );
                painter.rect_stroke(
                    preview,
                    3.0,
                    egui::Stroke::new(2.0_f32, viewer.app.settings.theme.accent.to_color32()),
                );
            }
            let duplicate = viewer.duplicate;
            if let Some((panel, split)) = viewer.edge_command {
                dock_at_edge(&mut workspace.dock, panel, split);
            }
            let closed: Vec<_> = Panel::ALL
                .into_iter()
                .filter(|panel| {
                    before.iter_all_tabs().any(|(_, tab)| tab.panel == *panel)
                        && !workspace
                            .dock
                            .iter_all_tabs()
                            .any(|(_, tab)| tab.panel == *panel)
                })
                .collect();
            if closed.len() == 1 {
                workspace
                    .hidden
                    .insert(closed[0], (before, topology(&workspace.dock)));
            }
            if workspace.dock.main_surface().is_empty() {
                ui.painter().text(
                    bounds.center(),
                    egui::Align2::CENTER_CENTER,
                    "Drop a panel here to dock it",
                    egui::FontId::proportional(14.0),
                    viewer.app.settings.theme.text_dim.to_color32(),
                );
            }
            if let Some(tab) = duplicate {
                workspace.new_panel(tab.panel, Some(tab));
            }
        });
    app.workspace_layout = workspace;
}

pub(crate) fn persist(app: &mut EditorApp, ctx: &Context) {
    let mut workspace = std::mem::take(&mut app.workspace_layout);
    workspace.persist(app, ctx, ctx.input(|i| i.viewport().close_requested()));
    app.workspace_layout = workspace;
}

pub(crate) fn render_menu(app: &mut EditorApp, ui: &mut Ui) {
    let mut workspace = std::mem::take(&mut app.workspace_layout);
    ui.menu_button("Layout", |ui| {
        ui.set_min_width(220.0);
        if ui.button("Save layout…").clicked() {
            workspace.manager_open = true;
            workspace.selected_preset = None;
            workspace.preset_name.clear();
            workspace.message.clear();
            ui.close_menu();
        }
        if ui.button("Manage layouts…").clicked() {
            workspace.manager_open = true;
            ui.close_menu();
        }
        if ui.button("Restore default layout").clicked() {
            workspace.restore_default();
            ui.close_menu();
        }
        ui.separator();
        if app.settings.workspace_layout_presets.is_empty() {
            ui.weak("No saved layouts yet");
        } else {
            for preset in &app.settings.workspace_layout_presets {
                if ui.button(&preset.name).clicked() {
                    if let Some(saved) = SavedLayout::decode(&preset.layout) {
                        workspace.apply(&saved);
                        workspace.last_observed = Some(saved);
                        app.session.status = format!("layout: {}", preset.name);
                    } else {
                        app.session.status = "this saved layout is invalid".into();
                    }
                    ui.close_menu();
                }
            }
        }
        ui.separator();
        ui.menu_button("new panel", |ui| {
            for panel in Panel::ALL {
                if ui.button(panel.title()).clicked() {
                    workspace.new_panel(panel, None);
                    ui.close_menu();
                }
            }
        });
        ui.weak("Panels");
        for panel in Panel::ALL {
            let mut visible = workspace.find_panel(panel).is_some();
            if ui.checkbox(&mut visible, panel.title()).changed() {
                workspace.toggle(panel);
            }
        }
        ui.separator();
        ui.weak("Drag panel tabs to rearrange the workspace");
    });
    app.workspace_layout = workspace;
}

fn save_preferences(app: &mut EditorApp, workspace: &mut WorkspaceLayout) {
    match app.settings.try_save() {
        Ok(()) => {
            workspace.message = "layout saved".into();
        }
        Err(error) => {
            workspace.message = format!("could not save layout: {error}");
        }
    }
    app.session.status = workspace.message.clone();
}

pub(crate) fn render_manager(app: &mut EditorApp, ctx: &Context) {
    let mut workspace = std::mem::take(&mut app.workspace_layout);
    if workspace.manager_open {
        let mut open = true;
        egui::Window::new("Workspace layouts")
            .id(Id::new("layout_manager"))
            .open(&mut open)
            .default_width(440.0)
            .resizable(true)
            .fit_to_screen(ctx)
            .show(ctx, |ui| {
                ui.label("Save the current arrangement and switch between your workspaces.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut workspace.preset_name)
                            .char_limit(64)
                            .desired_width(240.0)
                            .hint_text("My workspace"),
                    );
                });
                let name = workspace.preset_name.trim().to_owned();
                let existing = app
                    .settings
                    .workspace_layout_presets
                    .iter()
                    .position(|p| p.name == name);
                let can_save = !name.is_empty()
                    && (existing.is_some()
                        || app.settings.workspace_layout_presets.len() < MAX_PRESETS);
                if ui
                    .add_enabled(
                        can_save,
                        egui::Button::new(if existing.is_some() {
                            "Update saved layout"
                        } else {
                            "Save current layout"
                        }),
                    )
                    .clicked()
                {
                    if let Ok(layout) = serde_json::to_value(workspace.capture(ctx)) {
                        let preset = LayoutPreset {
                            name,
                            layout: layout.clone(),
                        };
                        if let Some(index) = existing {
                            app.settings.workspace_layout_presets[index] = preset;
                            workspace.selected_preset = Some(index);
                        } else {
                            app.settings.workspace_layout_presets.push(preset);
                            workspace.selected_preset =
                                Some(app.settings.workspace_layout_presets.len() - 1);
                        }
                        app.settings.workspace_layout = layout;
                        save_preferences(app, &mut workspace);
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("saved_layout_list")
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, preset) in
                            app.settings.workspace_layout_presets.iter().enumerate()
                        {
                            if ui
                                .selectable_label(
                                    workspace.selected_preset == Some(index),
                                    &preset.name,
                                )
                                .clicked()
                            {
                                workspace.selected_preset = Some(index);
                                workspace.preset_name = preset.name.clone();
                                workspace.message.clear();
                            }
                        }
                        if app.settings.workspace_layout_presets.is_empty() {
                            ui.weak("No saved layouts yet");
                        }
                    });
                if let Some(index) = workspace
                    .selected_preset
                    .filter(|i| *i < app.settings.workspace_layout_presets.len())
                {
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            if let Some(saved) = SavedLayout::decode(
                                &app.settings.workspace_layout_presets[index].layout,
                            ) {
                                workspace.apply(&saved);
                                workspace.last_observed = Some(saved);
                                workspace.message = "layout applied".into();
                            }
                        }
                        let unique = !workspace.preset_name.trim().is_empty()
                            && app
                                .settings
                                .workspace_layout_presets
                                .iter()
                                .enumerate()
                                .all(|(i, p)| i == index || p.name != workspace.preset_name.trim());
                        if ui
                            .add_enabled(unique, egui::Button::new("Rename"))
                            .clicked()
                        {
                            app.settings.workspace_layout_presets[index].name =
                                workspace.preset_name.trim().to_owned();
                            save_preferences(app, &mut workspace);
                        }
                        if ui.button("Delete").clicked() {
                            app.settings.workspace_layout_presets.remove(index);
                            workspace.selected_preset = None;
                            workspace.preset_name.clear();
                            save_preferences(app, &mut workspace);
                            workspace.message = "saved layout deleted".into();
                        }
                    });
                }
                if !workspace.message.is_empty() {
                    ui.label(&workspace.message);
                }
            });
        workspace.manager_open = open;
    }
    app.workspace_layout = workspace;
}

pub(crate) fn sanitize_presets(presets: &mut Vec<LayoutPreset>) {
    let mut names = HashSet::new();
    presets.retain_mut(|preset| {
        preset.name = preset
            .name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        !preset.name.is_empty()
            && names.insert(preset.name.clone())
            && SavedLayout::decode(&preset.layout).is_some()
    });
    presets.truncate(MAX_PRESETS);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_rect(workspace: &WorkspaceLayout, panel: Panel) -> Rect {
        let (surface, node, _) = workspace.find_panel(panel).expect("panel is visible");
        match workspace.dock[surface][node] {
            Node::Leaf { rect, .. } => rect,
            _ => panic!("panel must belong to a leaf"),
        }
    }

    #[test]
    fn regression_duplicate_panels_have_unique_ids_and_survive_layout_reload() {
        let mut workspace = WorkspaceLayout::default();
        for panel in Panel::ALL {
            workspace.new_panel(panel, None);
        }
        assert_eq!(workspace.dock.iter_all_tabs().count(), 10);
        let ids: HashSet<_> = workspace
            .dock
            .iter_all_tabs()
            .map(|(_, tab)| tab.id)
            .collect();
        assert_eq!(ids.len(), 10);
        let saved = workspace.capture(&Context::default());
        let restored = SavedLayout::decode(&serde_json::to_value(saved).unwrap())
            .unwrap()
            .dock();
        for panel in Panel::ALL {
            assert_eq!(
                restored
                    .iter_all_tabs()
                    .filter(|(_, tab)| tab.panel == panel)
                    .count(),
                2
            );
        }
        let duplicate = *workspace
            .dock
            .iter_all_tabs()
            .filter(|(_, tab)| tab.panel == Panel::Stage)
            .last()
            .unwrap()
            .1;
        workspace
            .dock
            .remove_tab(workspace.dock.find_tab(&duplicate).unwrap());
        assert!(
            workspace.find_panel(Panel::Stage).is_some(),
            "closing one stage closed the other"
        );
    }

    #[test]
    fn regression_split_stages_keep_navigation_and_zoom_independent() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        let mut child = app.state.project.q0rgs[0].clone();
        child.q0rg_id = 2;
        app.state.project.q0rgs.push(child);
        let first = PanelTab::primary(Panel::Stage);
        let second = PanelTab {
            panel: Panel::Stage,
            id: 6,
        };
        app.workspace_layout.dock = DockState::new(vec![first]);
        app.workspace_layout.dock.main_surface_mut().split_right(
            NodeIndex::root(),
            0.5,
            vec![second],
        );
        app.workspace_layout.active_stage = Some(first.id);
        let project = app.active_project_tab_id().unwrap_or(0);
        let mut other = StagePaneState::capture(&app.session);
        other.viewport.zoom = 2.5;
        other.viewport.pan = q0s_format::v2::Vec2::new(17.0, 29.0);
        other.q0rg = 2;
        app.workspace_layout
            .stage_views
            .insert((project, second.id), other);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| render(&mut app, ctx),
            );
        }
        assert_eq!(
            app.session.current_q0rg_id, 1,
            "a passive stage changed the editing context"
        );
        assert_eq!(app.session.viewport.zoom, 1.0);
        assert_eq!(
            app.workspace_layout.stage_views[&(project, second.id)]
                .viewport
                .zoom,
            2.5
        );
        let (_, node, _) = app.workspace_layout.dock.find_tab(&second).unwrap();
        let center = match app.workspace_layout.dock.main_surface()[node] {
            Node::Leaf { rect, .. } => rect.center(),
            _ => unreachable!(),
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(center),
                    egui::Event::Scroll(egui::vec2(0.0, 80.0)),
                ],
                ..Default::default()
            },
            |ctx| render(&mut app, ctx),
        );
        assert_eq!(app.workspace_layout.active_stage, Some(second.id));
        assert_eq!(app.session.current_q0rg_id, 2);
        assert!(app.session.viewport.zoom > 2.5);
        assert_eq!(
            app.workspace_layout.stage_views[&(project, first.id)]
                .viewport
                .zoom,
            1.0
        );
        assert_eq!(
            app.workspace_layout.stage_views[&(project, first.id)].q0rg,
            1
        );
    }

    #[test]
    fn restoring_default_layout_recovers_all_panels_and_their_rendered_sizes() {
        for screen in [
            egui::vec2(900.0, 560.0),
            egui::vec2(1280.0, 800.0),
            egui::vec2(2560.0, 1400.0),
        ] {
            let ctx = Context::default();
            crate::theme::install(&ctx);
            let mut app = EditorApp::default();
            for panel in [Panel::Stage, Panel::Library, Panel::Properties] {
                app.workspace_layout.toggle(panel);
            }
            app.workspace_layout.restore_default();
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, screen)),
                        ..Default::default()
                    },
                    |ctx| render(&mut app, ctx),
                );
            }

            let tools = panel_rect(&app.workspace_layout, Panel::Tools);
            let stage = panel_rect(&app.workspace_layout, Panel::Stage);
            let library = panel_rect(&app.workspace_layout, Panel::Library);
            let properties = panel_rect(&app.workspace_layout, Panel::Properties);
            let timeline = panel_rect(&app.workspace_layout, Panel::Timeline);
            assert!(tools.width() < screen.x * 0.1, "tools: {tools:?}");
            assert!(stage.width() > screen.x * 0.6, "stage: {stage:?}");
            assert!(tools.right() <= stage.left());
            assert!(stage.right() <= library.left());
            assert_eq!(library.left(), properties.left());
            assert!(library.bottom() <= properties.top());
            assert!(stage.bottom() <= timeline.top());
            assert!(properties.bottom() <= timeline.top());

            let saved = app.workspace_layout.capture(&ctx);
            let decoded = SavedLayout::decode(&serde_json::to_value(&saved).unwrap()).unwrap();
            assert_eq!(
                topology(&decoded.dock()),
                topology(&app.workspace_layout.dock)
            );
            assert!(saved.floating.is_empty());
        }
    }

    #[test]
    fn closing_and_reopening_stage_restores_its_dock_position() {
        let mut workspace = WorkspaceLayout::default();
        let before = topology(&workspace.dock);

        workspace.toggle(Panel::Stage);
        assert!(workspace.find_panel(Panel::Stage).is_none());
        assert_eq!(workspace.dock.iter_all_tabs().count(), Panel::ALL.len() - 1);

        workspace.toggle(Panel::Stage);
        assert_eq!(topology(&workspace.dock), before);
    }

    #[test]
    fn layouts_and_presets_without_stage_survive_reload() {
        let mut workspace = WorkspaceLayout::default();
        workspace.toggle(Panel::Stage);
        let layout = serde_json::to_value(workspace.capture(&Context::default())).unwrap();
        let mut settings = Settings {
            workspace_layout: layout.clone(),
            workspace_layout_presets: vec![LayoutPreset {
                name: "Without stage".into(),
                layout,
            }],
            ..Settings::default()
        };

        sanitize_presets(&mut settings.workspace_layout_presets);
        assert_eq!(settings.workspace_layout_presets.len(), 1);
        let mut restored = WorkspaceLayout::from_settings(&settings);
        assert_eq!(topology(&restored.dock), topology(&workspace.dock));
        assert!(restored.find_panel(Panel::Stage).is_none());

        restored.toggle(Panel::Stage);
        for panel in Panel::ALL {
            assert!(restored.find_panel(panel).is_some());
        }
    }

    #[test]
    fn empty_workspace_survives_reload_and_can_restore_stage() {
        let mut workspace = WorkspaceLayout::default();
        for panel in Panel::ALL {
            workspace.toggle(panel);
        }
        let settings = Settings {
            workspace_layout: serde_json::to_value(workspace.capture(&Context::default())).unwrap(),
            ..Settings::default()
        };

        let mut restored = WorkspaceLayout::from_settings(&settings);
        assert_eq!(restored.dock.iter_all_tabs().count(), 0);
        restored.toggle(Panel::Stage);
        assert_eq!(restored.dock.iter_all_tabs().count(), 1);
        assert!(restored.find_panel(Panel::Stage).is_some());
    }
}
