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
        if layout.version != 1 || layout.floating.len() > Panel::ALL.len() {
            return None;
        }
        let mut seen = HashSet::new();
        if let Some(node) = &layout.main {
            if !node.valid(0, &mut seen) {
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
                || !window.tree.valid(0, &mut seen)
            {
                return None;
            }
        }
        seen.contains(&Panel::Stage).then_some(layout)
    }

    fn dock(&self) -> DockState<Panel> {
        let mut dock = DockState::new(Vec::new());
        if let Some(node) = &self.main {
            *dock.main_surface_mut() = node.tree();
        }
        for window in &self.floating {
            let index = dock.add_window(vec![Panel::Stage]);
            dock[index] = window.tree.tree();
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
    fn valid(&self, depth: usize, seen: &mut HashSet<Panel>) -> bool {
        if depth > 5 {
            return false;
        }
        match self {
            Self::Tabs { panels, active } => {
                !panels.is_empty()
                    && panels.len() <= Panel::ALL.len()
                    && *active < panels.len()
                    && panels.iter().all(|panel| seen.insert(*panel))
            }
            Self::Split {
                fraction,
                first,
                second,
                ..
            } => {
                fraction.is_finite()
                    && (0.01..=0.99).contains(fraction)
                    && first.valid(depth + 1, seen)
                    && second.valid(depth + 1, seen)
            }
        }
    }

    fn capture(tree: &Tree<Panel>, index: NodeIndex) -> Option<Self> {
        match tree.iter().nth(index.0)? {
            Node::Leaf { tabs, active, .. } => Some(Self::Tabs {
                panels: tabs.clone(),
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

    fn tree(&self) -> Tree<Panel> {
        let mut tree = Tree::new(vec![Panel::Stage]);
        self.fill(&mut tree, NodeIndex::root());
        tree
    }

    fn fill(&self, tree: &mut Tree<Panel>, index: NodeIndex) {
        match self {
            Self::Tabs { panels, active } => {
                tree[index] = Node::leaf_with(panels.clone());
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
                    Node::leaf(Panel::Stage),
                );
                first.fill(tree, a);
                second.fill(tree, b);
            }
        }
    }
}

fn decorate(dock: &mut DockState<Panel>) {
    dock.translations.tab_context_menu.close_button = "Hide panel".into();
    dock.translations.tab_context_menu.eject_button = "Float panel".into();
    dock.translations.window.close_button_tooltip =
        "The stage must stay available. Drag its tab to dock it again.".into();
}

fn default_dock() -> DockState<Panel> {
    let mut dock = DockState::new(vec![Panel::Stage]);
    let tree = dock.main_surface_mut();
    let [top, _] = tree.split_below(NodeIndex::root(), 0.76, vec![Panel::Timeline]);
    let [content, _] = tree.split_left(top, 0.045, vec![Panel::Tools]);
    let [_, right] = tree.split_right(content, 0.75, vec![Panel::Library]);
    tree.split_below(right, 0.48, vec![Panel::Properties]);
    decorate(&mut dock);
    dock
}

pub(crate) struct WorkspaceLayout {
    dock: DockState<Panel>,
    last_observed: Option<SavedLayout>,
    changed_at: Option<Instant>,
    restore_windows: bool,
    manager_open: bool,
    preset_name: String,
    selected_preset: Option<usize>,
    message: String,
    hidden: HashMap<Panel, (DockState<Panel>, Vec<Option<LayoutNode>>)>,
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
        }
    }
}

impl WorkspaceLayout {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        let saved = SavedLayout::decode(&settings.workspace_layout);
        Self {
            dock: saved.as_ref().map_or_else(default_dock, SavedLayout::dock),
            last_observed: saved,
            restore_windows: true,
            ..Self::default()
        }
    }

    pub(crate) fn toggle(&mut self, panel: Panel) {
        if let Some(location) = self.dock.find_tab(&panel) {
            if panel != Panel::Stage {
                let before = self.dock.clone();
                self.dock.remove_tab(location);
                self.hidden.insert(panel, (before, topology(&self.dock)));
            }
        } else {
            if let Some((before, after)) = self.hidden.remove(&panel) {
                if topology(&self.dock) == after {
                    self.dock = before;
                    self.changed_at = Some(Instant::now());
                    return;
                }
            }
            let tree = self.dock.main_surface_mut();
            if tree.is_empty() {
                tree.push_to_first_leaf(panel);
            } else {
                match panel {
                    Panel::Timeline => {
                        tree.split_below(NodeIndex::root(), 0.76, vec![panel]);
                    }
                    Panel::Tools => {
                        tree.split_left(NodeIndex::root(), 0.06, vec![panel]);
                    }
                    _ => {
                        tree.split_right(NodeIndex::root(), 0.75, vec![panel]);
                    }
                }
            }
        }
        self.changed_at = Some(Instant::now());
    }

    fn capture(&self, ctx: &Context) -> SavedLayout {
        let mut floating = Vec::new();
        let margin = egui::Frame::window(&ctx.style()).total_margin().sum();
        for (index, surface) in self.dock.iter_surfaces().enumerate() {
            if let Surface::Window(tree, _) = surface {
                if let Some(node) = LayoutNode::capture(tree, NodeIndex::root()) {
                    // egui_dock's WindowState does not update its rect in 0.12.
                    // The actual egui area stores the outer position/size; convert
                    // size to window content size so loading never grows a window.
                    let id = Id::new(format!("window {:?}", SurfaceIndex(index)));
                    let rect = ctx
                        .memory(|m| m.area_rect(id))
                        .unwrap_or(Rect::from_min_size(
                            egui::pos2(80.0, 100.0),
                            egui::vec2(360.0, 400.0),
                        ));
                    let size = (rect.size() - margin).max(egui::vec2(44.0, 100.0));
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
    }

    fn restore_window_bounds(&mut self, ctx: &Context, bounds: Rect) {
        if !self.restore_windows {
            return;
        }
        self.restore_windows = false;
        let margin = egui::Frame::window(&ctx.style()).total_margin().sum();
        // Use saved topology for geometry; runtime nodes have not been drawn yet.
        if let Some(saved) = &self.last_observed {
            let mut windows = self.dock.iter_surfaces_mut().filter_map(|s| match s {
                Surface::Window(_, state) => Some(state),
                _ => None,
            });
            for (state, window) in windows.by_ref().zip(&saved.floating) {
                let size = egui::vec2(window.size[0], window.size[1])
                    .min((bounds.size() - margin).max(egui::Vec2::splat(100.0)));
                let max = (bounds.max - size - margin).max(bounds.min);
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

fn topology(dock: &DockState<Panel>) -> Vec<Option<LayoutNode>> {
    dock.iter_surfaces()
        .filter_map(|s| s.node_tree())
        .map(|tree| LayoutNode::capture(tree, NodeIndex::root()))
        .collect()
}

fn dock_at_edge(dock: &mut DockState<Panel>, panel: Panel, split: Split) {
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
    edge_command: Option<(Panel, Split)>,
    edge_preview: Option<Split>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Panel;

    fn title(&mut self, tab: &mut Panel) -> WidgetText {
        tab.title().into()
    }
    fn id(&mut self, tab: &mut Panel) -> Id {
        Id::new(("workspace_panel", *tab))
    }
    fn closeable(&mut self, tab: &mut Panel) -> bool {
        *tab != Panel::Stage
    }
    fn scroll_bars(&self, _: &Panel) -> [bool; 2] {
        [false, false]
    }
    fn on_tab_button(&mut self, panel: &mut Panel, response: &egui::Response) {
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
    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Panel, _: SurfaceIndex, _: NodeIndex) {
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

    fn ui(&mut self, ui: &mut Ui, tab: &mut Panel) {
        let started = Instant::now();
        let app = &mut *self.app;
        if *tab == Panel::Tools {
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
        crate::ui::panel_contents(ui, |ui| match tab {
            Panel::Stage => crate::panels::stage::render(app, ui),
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
        let span = match tab {
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
            workspace.restore_window_bounds(ctx, bounds);
            let mut style = egui_dock::Style::from_egui(ui.style());
            style.dock_area_padding = Some(egui::Margin::ZERO);
            style.tab_bar.height = 26.0;
            style.tab.tab_body.inner_margin = egui::Margin::same(6.0);
            style.separator.width = 4.0;
            style.separator.extra_interact_width = 4.0;
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
            };
            DockArea::new(&mut workspace.dock)
                .id(Id::new("editor_workspace"))
                .style(style)
                .window_bounds(bounds)
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
            if let Some((panel, split)) = viewer.edge_command {
                dock_at_edge(&mut workspace.dock, panel, split);
            }
            let closed: Vec<_> = Panel::ALL
                .into_iter()
                .filter(|panel| {
                    before.find_tab(panel).is_some() && workspace.dock.find_tab(panel).is_none()
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
            workspace.dock = default_dock();
            workspace.last_observed = None;
            workspace.changed_at = Some(Instant::now());
            workspace.hidden.clear();
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
        ui.weak("Panels");
        for panel in Panel::ALL {
            let mut visible = workspace.dock.find_tab(&panel).is_some();
            if ui
                .add_enabled(
                    panel != Panel::Stage,
                    egui::Checkbox::new(&mut visible, panel.title()),
                )
                .changed()
            {
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
