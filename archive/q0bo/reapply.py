"""Puts q0bo back into q0editor: run from the repository root after copying
`archive/q0bo/mascot` to `q0editor/src/mascot`.

    python archive/q0bo/reapply.py .

`--remove` undoes it (that is how he was taken out)."""
import sys
root = sys.argv[1]

REMOVE = "--remove" in sys.argv

def edit(path, pairs):
    if REMOVE:
        pairs = [(new, old) for old, new in pairs]
    full = root + "/" + path
    raw = open(full, "rb").read().decode("utf-8")
    crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")
    for old, new in pairs:
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected 1 match, found {count}:\n{old}")
        text = text.replace(old, new)
    if crlf:
        text = text.replace("\n", "\r\n")
    open(full, "wb").write(text.encode("utf-8"))
    print("edited", path)

edit("q0editor/src/lib.rs", [
    ("pub mod lights;\n", "pub mod lights;\nmod mascot;\n"),
])

edit("q0editor/src/settings.rs", [
    ("    #[serde(default)]\n    pub plugins: crate::plugins::PluginPrefs,\n}\n",
     "    #[serde(default)]\n    pub plugins: crate::plugins::PluginPrefs,\n"
     "    /// What the mascot, q0bo, remembers: whether he has said hello.\n"
     "    #[serde(default)]\n    pub mascot: crate::mascot::MascotSettings,\n}\n"),
    ("            plugins: Default::default(),\n        }\n",
     "            plugins: Default::default(),\n            mascot: Default::default(),\n        }\n"),
])

edit("q0editor/src/state.rs", [
    ("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Tool {",
     "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum Tool {"),
])

edit("q0editor/src/tools.rs", [
    ("fn primitive_drag(\n",
     "/// The outline the oval tool draws between two corners.\n"
     "pub(crate) fn oval_path(a: Vec2, b: Vec2) -> Option<VPath> {\n"
     "    primitive_path(PrimitiveKind::Oval, a, b)\n}\n\nfn primitive_drag(\n"),
])

edit("q0editor/src/app.rs", [
    ("    pub(crate) sweeper: crate::sweeper::Sweeper,\n",
     "    pub(crate) sweeper: crate::sweeper::Sweeper,\n"
     "    /// q0bo, the mascot: his greeting, the tutorial and the ask tab.\n"
     "    pub(crate) mascot: crate::mascot::Mascot,\n"),
    ("            sweeper: crate::sweeper::Sweeper::default(),\n",
     "            sweeper: crate::sweeper::Sweeper::default(),\n"
     "            mascot: crate::mascot::Mascot::default(),\n"),
    ("    ToggleSweeper,\n    TogglePlugins,\n",
     "    ToggleSweeper,\n    StartTutorial,\n    AskQ0bo,\n    TogglePlugins,\n"),
    ("            \"toggle-sweeper\" => Self::ToggleSweeper,\n",
     "            \"toggle-sweeper\" => Self::ToggleSweeper,\n"
     "            \"start-tutorial\" => Self::StartTutorial,\n"
     "            \"ask-q0bo\" => Self::AskQ0bo,\n"),
    ("            Self::ToggleSweeper => \"toggle-sweeper\",\n",
     "            Self::ToggleSweeper => \"toggle-sweeper\",\n"
     "            Self::StartTutorial => \"start-tutorial\",\n"
     "            Self::AskQ0bo => \"ask-q0bo\",\n"),
    ("            Action::ToggleSweeper => {\n",
     "            Action::StartTutorial => crate::mascot::start_tutorial(self),\n"
     "            Action::AskQ0bo => crate::mascot::open_ask(self),\n"
     "            Action::ToggleSweeper => {\n"),
    ("        self.record_held_objects(ctx);\n\n        let dialogs_started = Instant::now();\n",
     "        self.record_held_objects(ctx);\n        crate::mascot::update(self, ctx);\n\n        let dialogs_started = Instant::now();\n"),
    ("    fn switch_to_document_tab(&mut self, tab_id: u64) -> bool {\n",
     "    /// Opens `project`, unsaved, in a new document tab and returns the tab.\n"
     "    pub(crate) fn open_new_document(&mut self, project: ProjectV2) -> Option<u64> {\n"
     "        let workspace = self.workspace_for_project(project, None);\n"
     "        if self.activate_new_workspace(workspace) {\n"
     "            self.active_document_tab\n"
     "        } else {\n"
     "            None\n"
     "        }\n"
     "    }\n\n"
     "    pub(crate) fn switch_to_document(&mut self, tab_id: u64) -> bool {\n"
     "        self.switch_to_document_tab(tab_id)\n"
     "    }\n\n"
     "    pub(crate) fn document_tab_exists(&self, tab_id: u64) -> bool {\n"
     "        self.document_tabs.iter().any(|tab| tab.id == tab_id)\n"
     "    }\n\n"
     "    fn switch_to_document_tab(&mut self, tab_id: u64) -> bool {\n"),
])

edit("q0editor/src/workspace.rs", [
    ("    Library,\n    Properties,\n}\n\nimpl Panel {\n    const ALL: [Self; 5] = [\n        Self::Stage,\n        Self::Tools,\n        Self::Timeline,\n        Self::Library,\n        Self::Properties,\n    ];\n",
     "    Library,\n    Properties,\n    /// q0bo's \"ask q0bo\" tab.\n    Helper,\n}\n\nimpl Panel {\n"
     "    /// The panels of the default workspace.\n"
     "    #[cfg(test)]\n"
     "    const DEFAULT: [Self; 5] = [\n        Self::Stage,\n        Self::Tools,\n        Self::Timeline,\n        Self::Library,\n        Self::Properties,\n    ];\n"
     "    /// Every panel, for the menus.\n"
     "    const ALL: [Self; 6] = [\n        Self::Stage,\n        Self::Tools,\n        Self::Timeline,\n        Self::Library,\n        Self::Properties,\n        Self::Helper,\n    ];\n"),
    ("            Self::Properties => \"Properties\",\n        }\n",
     "            Self::Properties => \"Properties\",\n            Self::Helper => \"ask q0bo\",\n        }\n"),
    ("        let app = &mut *self.app;\n        if tab.panel == Panel::Tools {\n",
     "        let app = &mut *self.app;\n"
     "        crate::mascot::mark(ui.ctx(), crate::mascot::Spot::Panel(tab.panel), ui.max_rect());\n"
     "        if tab.panel == Panel::Tools {\n"),
    ("            Panel::Properties => crate::panels::properties::render(app, ui),\n        });\n",
     "            Panel::Properties => crate::panels::properties::render(app, ui),\n"
     "            Panel::Helper => crate::mascot::ask_panel(app, ui),\n        });\n"),
    ("            Panel::Stage => return,\n",
     "            Panel::Stage | Panel::Helper => return,\n"),
    ("    pub(crate) fn toggle(&mut self, panel: Panel) {\n",
     "    /// Brings a panel into sight: docks it if it is hidden, then puts its\n"
     "    /// tab in front.\n"
     "    pub(crate) fn show(&mut self, panel: Panel) {\n"
     "        if self.find_panel(panel).is_none() {\n"
     "            self.toggle(panel);\n"
     "        }\n"
     "        if let Some(location) = self.find_panel(panel) {\n"
     "            self.dock.set_active_tab(location);\n"
     "            self.changed_at = Some(Instant::now());\n"
     "        }\n"
     "    }\n\n"
     "    pub(crate) fn toggle(&mut self, panel: Panel) {\n"),
    # Tests that mean the default workspace's panels.
    ("        let mut workspace = WorkspaceLayout::default();\n        for panel in Panel::ALL {\n            workspace.new_panel(panel, None);\n        }\n",
     "        let mut workspace = WorkspaceLayout::default();\n        for panel in Panel::DEFAULT {\n            workspace.new_panel(panel, None);\n        }\n"),
    ("            .dock();\n        for panel in Panel::ALL {\n",
     "            .dock();\n        for panel in Panel::DEFAULT {\n"),
    ("Panel::ALL.len() - 1);", "Panel::DEFAULT.len() - 1);"),
    ("        restored.toggle(Panel::Stage);\n        for panel in Panel::ALL {\n            assert!(restored.find_panel(panel).is_some());\n",
     "        restored.toggle(Panel::Stage);\n        for panel in Panel::DEFAULT {\n            assert!(restored.find_panel(panel).is_some());\n"),
    ("        let mut workspace = WorkspaceLayout::default();\n        for panel in Panel::ALL {\n            workspace.toggle(panel);\n        }\n",
     "        let mut workspace = WorkspaceLayout::default();\n        for panel in Panel::DEFAULT {\n            workspace.toggle(panel);\n        }\n"),
])

edit("q0editor/src/panels/stage.rs", [
    ("    app.textures.editing_mask_q0rg_id = Some(app.session.current_q0rg_id);\n",
     "    if interactive {\n"
     "        crate::mascot::mark(ui.ctx(), crate::mascot::Spot::Stage, canvas_rect);\n"
     "        crate::mascot::mark(ui.ctx(), crate::mascot::Spot::Paper, stage_rect);\n"
     "    }\n"
     "    app.textures.editing_mask_q0rg_id = Some(app.session.current_q0rg_id);\n"),
])

edit("q0editor/src/panels/toolbar.rs", [
    ("    if response.clicked() {\n        app.queue(Action::SelectTool(tool));\n    }\n",
     "    crate::mascot::mark(ui.ctx(), crate::mascot::Spot::Tool(tool), response.rect);\n"
     "    if response.clicked() {\n        app.queue(Action::SelectTool(tool));\n    }\n"),
])

edit("q0editor/src/panels/timeline.rs", [
    ("pub fn render(app: &mut EditorApp, ui: &mut Ui) {\n    let scrub_frame_before = app.session.timeline_frame();\n",
     "pub fn render(app: &mut EditorApp, ui: &mut Ui) {\n"
     "    crate::mascot::mark(\n"
     "        ui.ctx(),\n"
     "        crate::mascot::Spot::Panel(crate::workspace::Panel::Timeline),\n"
     "        ui.max_rect(),\n"
     "    );\n"
     "    let scrub_frame_before = app.session.timeline_frame();\n"),
    ("                let row_y = rect.min.y + ROW_H * (li as f32 + camera_row_count as f32 + 1.0);\n                let row_rect =\n                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));\n                let is_current_layer",
     "                let row_y = rect.min.y + ROW_H * (li as f32 + camera_row_count as f32 + 1.0);\n                let row_rect =\n                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));\n"
     "                crate::mascot::mark(\n"
     "                    ui.ctx(),\n"
     "                    crate::mascot::Spot::TimelineRow(row_layer_id),\n"
     "                    Rect::from_min_size(pos2(frame_base_x, row_y), vec2(FRAME_W, ROW_H)),\n"
     "                );\n"
     "                let is_current_layer"),
])

edit("q0editor/src/panels/menu.rs", [
    ("    ui.menu_button(\"Help\", |ui| {\n        if ui.button(\"Credits...\").clicked() {\n",
     "    ui.menu_button(\"Help\", |ui| {\n"
     "        if ui.button(\"ask q0bo\").clicked() {\n"
     "            app.queue(Action::AskQ0bo);\n"
     "            ui.close_menu();\n"
     "        }\n"
     "        if ui.button(\"Tutorial with q0bo\").clicked() {\n"
     "            app.queue(Action::StartTutorial);\n"
     "            ui.close_menu();\n"
     "        }\n"
     "        ui.separator();\n"
     "        if ui.button(\"Credits...\").clicked() {\n"),
])
