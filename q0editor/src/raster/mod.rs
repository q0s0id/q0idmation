//! Raster layers: frame-by-frame drawing in pixels.
//!
//! A raster layer is an ordinary layer whose keyframes hold pixel drawings
//! (`Asset::Raster`) on identity placements, the way a vector layer's hold
//! raw vector drawings. So keyframes, holds, blank keyframes, onion skin,
//! symbols, tweens and export all work on it unchanged.
//!
//! Like 3D, raster is an aspect of the editor that shows up when it is used:
//! the toolbox grows a Raster section once the project has a raster layer
//! (`offered`). While a raster tool is picked (`RasterSession::tool`), the
//! stage sends its input here instead of to the 2D tools.

pub mod cursor;
pub mod engine;
pub mod fill;
pub mod presets;
pub mod select;
pub mod stage;
pub mod ui;

use std::collections::BTreeMap;

use egui::{Context, Event, Key, Pos2, Response};
use q0s_format::pixels::RasterAsset;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, Layer, LayerKey, LayerKind, LayerMetadata, Placement, ProjectV2, Rgba, Target,
    Transform2D, Tween, Vec2,
};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::render::StageView;
use engine::BrushSettings;
use presets::Family;

/// The raster tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RasterTool {
    Brush,
    Eraser,
    Smudge,
    Fill,
    Picker,
    Marquee,
    Lasso,
}

impl RasterTool {
    pub const ALL: [Self; 7] = [
        Self::Brush,
        Self::Eraser,
        Self::Smudge,
        Self::Fill,
        Self::Picker,
        Self::Marquee,
        Self::Lasso,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Brush => "Raster brush",
            Self::Eraser => "Raster eraser",
            Self::Smudge => "Smudge",
            Self::Fill => "Raster fill",
            Self::Picker => "Colour picker",
            Self::Marquee => "Rectangle select",
            Self::Lasso => "Lasso select",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Brush => "Raster brush: paints pixels with the stroke colour. Pencils, inks, markers, paints and more in the Properties panel. [ and ] size it, Shift-click draws a straight line from the last stroke, Alt-click picks a colour.",
            Self::Eraser => "Raster eraser: rubs pixels out. [ and ] size it, Shift-click erases a straight line.",
            Self::Smudge => "Smudge: drags and blends the paint under it, or softens it.",
            Self::Fill => "Raster fill: pours the fill colour into an area. Closes small gaps in the lines and fills under their edges.",
            Self::Picker => "Colour picker: click to take a colour from the stage into the active swatch.",
            Self::Marquee => "Rectangle select: drag a box. Drag inside it to move the pixels, its handles to size and turn them. Delete clears, Enter or Escape lets go. Shift adds, Alt takes away.",
            Self::Lasso => "Lasso select: draw round the pixels to take. Same moves as Rectangle select.",
        }
    }

    pub fn family(self) -> Option<Family> {
        match self {
            Self::Brush => Some(Family::Paint),
            Self::Eraser => Some(Family::Erase),
            Self::Smudge => Some(Family::Smudge),
            _ => None,
        }
    }
}

/// What the raster tools remember between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RasterPrefs {
    pub brush: String,
    pub eraser: String,
    pub smudge: String,
    /// The user's own changes to the built-in brushes, by preset key.
    pub changed: BTreeMap<String, BrushSettings>,
    pub fill: fill::FillSettings,
    /// The picker reads every visible layer, not just the current one.
    pub pick_all_layers: bool,
}

impl Default for RasterPrefs {
    fn default() -> Self {
        Self {
            brush: "pencil".to_string(),
            eraser: "hard_eraser".to_string(),
            smudge: "finger".to_string(),
            changed: BTreeMap::new(),
            fill: fill::FillSettings::default(),
            pick_all_layers: true,
        }
    }
}

impl RasterPrefs {
    pub fn chosen(&self, family: Family) -> &str {
        match family {
            Family::Paint => &self.brush,
            Family::Erase => &self.eraser,
            Family::Smudge => &self.smudge,
        }
    }

    pub fn choose(&mut self, family: Family, key: &str) {
        let slot = match family {
            Family::Paint => &mut self.brush,
            Family::Erase => &mut self.eraser,
            Family::Smudge => &mut self.smudge,
        };
        *slot = key.to_string();
    }

    /// The chosen brush of `family`, with the user's changes.
    pub fn preset(&self, family: Family) -> (presets::Preset, BrushSettings) {
        let all = presets::family(family);
        let key = self.chosen(family);
        let preset = all
            .iter()
            .find(|preset| preset.key == key)
            .copied()
            .unwrap_or(all[0]);
        let settings = self
            .changed
            .get(preset.key)
            .copied()
            .unwrap_or(preset.settings)
            .sanitized();
        (preset, settings)
    }

    /// Keeps `settings` as the user's version of the chosen brush.
    pub fn set_settings(&mut self, family: Family, settings: BrushSettings) {
        let (preset, _) = self.preset(family);
        if settings == preset.settings {
            self.changed.remove(preset.key);
        } else {
            self.changed.insert(preset.key.to_string(), settings);
        }
    }
}

/// Where a stroke paints: one raster drawing, placed on one keyframe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasRef {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub asset_id: u16,
    /// From the timeline's space (what the stage cursor is in) to the
    /// drawing's pixels.
    pub to_local: Affine,
}

impl CanvasRef {
    pub fn local(&self, stage: Vec2) -> (f32, f32) {
        let p = self.to_local.apply(stage);
        (p.x, p.y)
    }

    /// Drawing pixels per stage unit.
    pub fn density(&self) -> f32 {
        self.to_local.uniform_scale().max(1e-4)
    }
}

struct ActiveStroke {
    stroke: engine::Stroke,
    canvas: CanvasRef,
    /// The history after this stroke's snapshot: an undo mid-stroke ends it.
    history_mark: u64,
}

/// What the raster tools keep for one document.
#[derive(Default)]
pub struct RasterSession {
    pub tool: Option<RasterTool>,
    stroke: Option<ActiveStroke>,
    /// Where the last stroke ended, for Shift-click lines.
    last_end: Option<(u16, u16, u16, f32, f32)>,
    pub selection: select::SelectionState,
    /// Pixels copied from a raster selection.
    pub clipboard: Option<select::Clip>,
    /// The last Copy or Cut was of pixels, so Paste on a raster layer
    /// pastes them.
    pub clip_newest: bool,
}

impl RasterSession {
    /// Back to the 2D tools. A stroke in progress keeps what it painted.
    pub fn leave(&mut self) {
        self.tool = None;
        self.stroke = None;
    }

    pub fn active(&self) -> bool {
        self.tool.is_some()
    }

    pub fn painting(&self) -> bool {
        self.stroke.is_some()
    }
}

// ---------------------------------------------------------------------------
// Gating

/// Does this project have raster in it, so the raster tools are worth
/// showing?
pub fn offered(project: &ProjectV2) -> bool {
    q0s_format::v2::uses_raster(project)
}

/// A raster layer: marked as one, or holding raster drawings (as a layer
/// a raster drawing was moved to is, when it became a symbol).
pub fn is_raster_layer(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    if project
        .layer_metadata
        .get(&LayerKey::new(q0rg_id, layer_id))
        .is_some_and(|metadata| metadata.raster)
    {
        return true;
    }
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };
    layer
        .placements
        .iter()
        .any(|placement| match placement.target {
            Target::Asset(asset_id) => raster_ref(project, asset_id).is_some(),
            Target::Q0rg(_) => false,
        })
}

/// The raster tool a 2D tool stands for on the current layer, when it is a
/// raster layer: Brush and Pencil paint, Eraser erases, the Bucket fills,
/// the Eyedropper picks and the Lasso selects pixels.
pub fn raster_kin(app: &EditorApp, tool: crate::state::Tool) -> Option<RasterTool> {
    use crate::state::Tool;
    let kin = match tool {
        Tool::Brush | Tool::Pencil => RasterTool::Brush,
        Tool::Eraser => RasterTool::Eraser,
        Tool::Bucket => RasterTool::Fill,
        Tool::Eyedropper => RasterTool::Picker,
        Tool::Lasso => RasterTool::Lasso,
        _ => return None,
    };
    is_raster_layer(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_layer_id,
    )
    .then_some(kin)
}

/// Picks a raster tool, leaving the 2D and 3D ones.
pub fn pick_tool(app: &mut EditorApp, tool: RasterTool) {
    if !crate::text_tool::finish_edit(app) {
        return;
    }
    if app.session.raster.tool != Some(tool) {
        select::let_go(app);
    }
    app.plugins.active_tool = None;
    app.session.tools3d.leave();
    app.session.raster.tool = Some(tool);
    app.session.raster.stroke = None;
    app.session.tool_state = crate::state::ToolState::Idle;
    if !is_raster_layer(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_layer_id,
    ) {
        if let Some(layer_id) = first_raster_layer(&app.state.project, app.session.current_q0rg_id)
        {
            app.session.current_layer_id = layer_id;
        }
    }
    app.session.status = tool.hint().to_string();
}

fn first_raster_layer(project: &ProjectV2, q0rg_id: u16) -> Option<u16> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    q0rg.layers
        .iter()
        .map(|layer| layer.layer_id)
        .find(|layer_id| is_raster_layer(project, q0rg_id, *layer_id))
}

/// Adds a raster layer above the current one and picks the raster brush,
/// as "New raster layer" in the timeline and the Insert menu do.
pub fn add_raster_layer(app: &mut EditorApp, ctx: &Context) {
    if in_room(app) {
        app.session.status = NoCanvas::Online.message().to_string();
        return;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let before = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .map(|q0rg| q0rg.layers.len());
    app.handle(ctx, crate::app::Action::AddLayer);
    let after = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .map(|q0rg| q0rg.layers.len());
    if after <= before {
        return;
    }
    let layer_id = app.session.current_layer_id;
    let count = app
        .state
        .project
        .layer_metadata
        .values()
        .filter(|metadata| metadata.raster)
        .count();
    let key = LayerKey::new(q0rg_id, layer_id);
    let metadata = app.state.project.layer_metadata.entry(key).or_default();
    metadata.raster = true;
    if let Some(layer) = layer_mut(&mut app.state.project, q0rg_id, layer_id) {
        layer.name = format!("Raster {}", count + 1);
    }
    app.state.mark_dirty();
    pick_tool(app, RasterTool::Brush);
    app.session.status = "raster layer added: the Raster tools are in the toolbox".to_string();
}

/// Turns an empty layer into a raster layer, or a raster layer with no
/// drawings back. Layers with artwork keep their kind.
pub fn set_layer_raster(app: &mut EditorApp, q0rg_id: u16, layer_id: u16, raster: bool) -> bool {
    if raster && in_room(app) {
        app.session.status = NoCanvas::Online.message().to_string();
        return false;
    }
    let project = &app.state.project;
    if project.layer_metadata(q0rg_id, layer_id).kind == LayerKind::Folder {
        return false;
    }
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };
    if !layer.placements.is_empty() {
        app.session.status = "only empty layers change between vector and raster".to_string();
        return false;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let key = LayerKey::new(q0rg_id, layer_id);
    let metadata = app.state.project.layer_metadata.entry(key).or_default();
    metadata.raster = raster;
    if *metadata == LayerMetadata::default() {
        app.state.project.layer_metadata.remove(&key);
    }
    app.state.mark_dirty();
    true
}

/// Whether the open document is an online room's.
fn in_room(app: &EditorApp) -> bool {
    app.online.editing_tab(app.active_project_tab_id())
}

fn layer_mut(project: &mut ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<&mut Layer> {
    project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)
}

pub fn raster_mut(project: &mut ProjectV2, asset_id: u16) -> Option<&mut RasterAsset> {
    project.assets.iter_mut().find_map(|asset| match asset {
        Asset::Raster(raster) if raster.asset_id == asset_id => Some(raster),
        _ => None,
    })
}

pub fn raster_ref(project: &ProjectV2, asset_id: u16) -> Option<&RasterAsset> {
    project.assets.iter().find_map(|asset| match asset {
        Asset::Raster(raster) if raster.asset_id == asset_id => Some(raster),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Canvases

/// Why there is nothing to paint on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCanvas {
    NotRaster,
    Locked,
    Hidden,
    Missing,
    /// Online rooms do not carry raster drawings yet.
    Online,
}

impl NoCanvas {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotRaster => "this is a vector layer: raster tools paint on raster layers (layer menu > New raster layer)",
            Self::Locked => "this layer is locked",
            Self::Hidden => "this layer is hidden",
            Self::Missing => "no layer to paint on",
            Self::Online => "raster layers don't sync in online rooms yet: paint offline and share the file",
        }
    }
}

/// The keyframe that shows on `frame`: the last at or before it.
fn showing_keyframe(layer: &Layer, frame: u16) -> Option<u16> {
    layer
        .keyframe_frames()
        .into_iter()
        .rev()
        .find(|key| *key <= frame)
}

/// The raster drawing shown on the current layer and frame, without making
/// one: for the picker, the fill's reference and the selection.
pub fn find_canvas(app: &EditorApp) -> Result<Option<CanvasRef>, NoCanvas> {
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    if !is_raster_layer(project, q0rg_id, layer_id) {
        return Err(NoCanvas::NotRaster);
    }
    let layer = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .ok_or(NoCanvas::Missing)?;
    if layer
        .timeline_frame_count
        .is_some_and(|count| frame >= count)
    {
        return Ok(None);
    }
    let Some(key) = showing_keyframe(layer, frame) else {
        return Ok(None);
    };
    let active = q0s_format::raster::active_placements_at(layer, frame);
    for (index, transform) in active.into_iter().rev() {
        let Some(placement) = layer.placements.get(index) else {
            continue;
        };
        if placement.frame != key {
            continue;
        }
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        if raster_ref(project, asset_id).is_none() {
            continue;
        }
        let mut world = Affine::from_transform(transform);
        if let Some(parent) = q0s_format::parenting::parent_world(project, q0rg_id, layer_id, frame)
        {
            world = Affine::compose(parent, world);
        }
        let Some(to_local) = world.inverse() else {
            continue;
        };
        return Ok(Some(CanvasRef {
            q0rg_id,
            layer_id,
            asset_id,
            to_local,
        }));
    }
    Ok(None)
}

fn next_asset_id(project: &ProjectV2) -> Option<u16> {
    project
        .assets
        .iter()
        .map(Asset::id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
}

/// How many placements anywhere show `asset_id`.
fn uses_of(project: &ProjectV2, asset_id: u16) -> usize {
    project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter(|placement| placement.target == Target::Asset(asset_id))
        .count()
}

/// The drawing to paint on at the current layer and frame, made ready:
/// a keyframe and an empty drawing are made when there are none, and a
/// drawing another keyframe shares is copied first (its tiles stay shared
/// until painted). Call after taking the undo snapshot.
pub fn prepare_canvas(app: &mut EditorApp) -> Result<CanvasRef, NoCanvas> {
    if in_room(app) {
        return Err(NoCanvas::Online);
    }
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    {
        let project = &app.state.project;
        if !is_raster_layer(project, q0rg_id, layer_id) {
            return Err(NoCanvas::NotRaster);
        }
        if project.layer_is_locked(q0rg_id, layer_id) {
            return Err(NoCanvas::Locked);
        }
        if !project.layer_is_visible(q0rg_id, layer_id) {
            return Err(NoCanvas::Hidden);
        }
    }
    if let Some(canvas) = find_canvas(app)? {
        if uses_of(&app.state.project, canvas.asset_id) <= 1 {
            return Ok(canvas);
        }
        // Shared with another keyframe (a copied keyframe, say): this one
        // gets its own copy.
        let new_id = next_asset_id(&app.state.project).ok_or(NoCanvas::Missing)?;
        let mut copy = raster_ref(&app.state.project, canvas.asset_id)
            .cloned()
            .ok_or(NoCanvas::Missing)?;
        copy.asset_id = new_id;
        app.state.project.assets.push(Asset::Raster(copy));
        let layer =
            layer_mut(&mut app.state.project, q0rg_id, layer_id).ok_or(NoCanvas::Missing)?;
        let key = showing_keyframe(layer, frame).ok_or(NoCanvas::Missing)?;
        if let Some(placement) = layer.placements.iter_mut().rev().find(|placement| {
            placement.frame == key && placement.target == Target::Asset(canvas.asset_id)
        }) {
            placement.target = Target::Asset(new_id);
        }
        return Ok(CanvasRef {
            asset_id: new_id,
            ..canvas
        });
    }
    // No drawing yet: on a blank keyframe, a held empty frame, or past the
    // layer's last frame.
    let new_id = next_asset_id(&app.state.project).ok_or(NoCanvas::Missing)?;
    app.state
        .project
        .assets
        .push(Asset::Raster(RasterAsset::new(new_id)));
    let layer = layer_mut(&mut app.state.project, q0rg_id, layer_id).ok_or(NoCanvas::Missing)?;
    let past_end = layer
        .timeline_frame_count
        .is_some_and(|count| frame >= count);
    let key = match showing_keyframe(layer, frame) {
        Some(key) if !past_end => key,
        _ => {
            layer.ensure_explicit_keyframe(frame);
            frame
        }
    };
    layer.push_placement(Placement {
        instance_id: 0,
        frame: key,
        target: Target::Asset(new_id),
        transform: Transform2D::IDENTITY,
        pivot: None,
        tween: Tween::None,
        fx: Default::default(),
    });
    if let Some(q0rg) = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    {
        q0rg.frame_count = q0rg.frame_count.max(frame.saturating_add(1));
    }
    let mut world = Affine::IDENTITY;
    if let Some(parent) =
        q0s_format::parenting::parent_world(&app.state.project, q0rg_id, layer_id, frame)
    {
        world = parent;
    }
    Ok(CanvasRef {
        q0rg_id,
        layer_id,
        asset_id: new_id,
        to_local: world.inverse().unwrap_or(Affine::IDENTITY),
    })
}

// ---------------------------------------------------------------------------
// Colours

fn rgb(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b]
}

/// The colour the brush paints with: the stroke swatch.
pub fn paint_color(app: &EditorApp) -> Option<Rgba> {
    (!app.session.stroke_none).then_some(app.session.stroke_color)
}

/// The colour the fill pours: the fill swatch, or the last one when it is
/// set to none.
pub fn fill_color(app: &EditorApp) -> Rgba {
    app.session.fill_color.unwrap_or(app.session.fill_kept)
}

// ---------------------------------------------------------------------------
// Input

/// Pen pressure in this frame's input, if a pen is down.
fn pressure(ctx: &Context) -> Option<f32> {
    ctx.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            Event::Touch { force, .. } => *force,
            _ => None,
        })
    })
}

/// Stage input while a raster tool is picked.
pub fn handle(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    let Some(tool) = app.session.raster.tool else {
        return;
    };
    if !offered(&app.state.project) {
        // The last raster layer went (an undo, say): back to the 2D tools.
        app.session.raster.leave();
        return;
    }
    if !matches!(tool, RasterTool::Marquee | RasterTool::Lasso) && select::floating(app) {
        select::let_go(app);
    }
    resize_keys(app, ctx, tool);
    match tool {
        RasterTool::Brush | RasterTool::Eraser | RasterTool::Smudge => {
            take_screen_pick(app, ctx);
            if ctx.input(|input| input.modifiers.alt) && !app.session.raster.painting() {
                if response.clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        pick(app, view, pointer, ctx, false);
                    }
                }
                return;
            }
            paint(app, response, view, ctx, tool);
        }
        RasterTool::Fill => fill::handle(app, response, view, ctx),
        RasterTool::Picker => {
            take_screen_pick(app, ctx);
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    pick(app, view, pointer, ctx, true);
                }
            }
        }
        RasterTool::Marquee | RasterTool::Lasso => select::handle(app, response, view, ctx, tool),
    }
}

/// `[` and `]` size the brush, as in most painting programs.
fn resize_keys(app: &mut EditorApp, ctx: &Context, tool: RasterTool) {
    let Some(family) = tool.family() else {
        return;
    };
    if ctx.wants_keyboard_input() {
        return;
    }
    let (smaller, bigger) = ctx.input(|input| {
        (
            input.key_pressed(Key::OpenBracket),
            input.key_pressed(Key::CloseBracket),
        )
    });
    if !smaller && !bigger {
        return;
    }
    let (_, mut settings) = app.settings.raster.preset(family);
    // A fifth each press, and always at least a pixel.
    let size = settings.size;
    let next = if bigger {
        (size * 1.25).max(size + 1.0)
    } else {
        (size * 0.8).min(size - 1.0)
    };
    settings.size = next
        .round()
        .clamp(*engine::SIZE_RANGE.start(), *engine::SIZE_RANGE.end());
    app.settings.raster.set_settings(family, settings);
    app.session.status = format!("brush size {:.0} px", settings.size);
}

fn paint(
    app: &mut EditorApp,
    response: &Response,
    view: &StageView,
    ctx: &Context,
    tool: RasterTool,
) {
    let Some(family) = tool.family() else {
        return;
    };
    let (pressed, released, shift, time, moves) = ctx.input(|input| {
        let moves: Vec<Pos2> = input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::PointerMoved(pos) => Some(*pos),
                _ => None,
            })
            .collect();
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_released() || !input.pointer.primary_down(),
            input.modifiers.shift,
            input.time,
            moves,
        )
    });
    let force = pressure(ctx);
    if app.session.raster.stroke.is_none() {
        let panning = app.session.viewport.panning || app.session.viewport.hand_active;
        if !(pressed && response.hovered()) || panning {
            return;
        }
        let Some(pointer) = ctx.input(|input| input.pointer.press_origin()) else {
            return;
        };
        let color = if family == Family::Paint {
            match paint_color(app) {
                Some(color) => color,
                None => {
                    app.session.status =
                        "the stroke colour is none: pick a colour to paint with".to_string();
                    return;
                }
            }
        } else {
            app.session.stroke_color
        };
        if let Err(why) = check_layer(app) {
            app.session.status = why.message().to_string();
            return;
        }
        if family != Family::Paint && matches!(find_canvas(app), Ok(None)) {
            app.session.status = "nothing drawn on this frame".to_string();
            return;
        }
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        let canvas = match prepare_canvas(app) {
            Ok(canvas) => canvas,
            Err(why) => {
                app.session.status = why.message().to_string();
                return;
            }
        };
        let (_, mut settings) = app.settings.raster.preset(family);
        // Sizes are in stage pixels; a drawing scaled up paints finer.
        settings.size *= canvas.density();
        settings.taper *= canvas.density();
        let start = canvas.local(view.screen_to_stage(pointer));
        let line_from = app
            .session
            .raster
            .last_end
            .filter(|(q0rg, layer, asset, _, _)| {
                shift
                    && (*q0rg, *layer, *asset) == (canvas.q0rg_id, canvas.layer_id, canvas.asset_id)
            })
            .map(|(_, _, _, x, y)| (x, y));
        let pixels_per_screen = canvas.density() / view.effective_scale().max(1e-4);
        let Some(drawing) = raster_mut(&mut app.state.project, canvas.asset_id) else {
            return;
        };
        let first = line_from.unwrap_or(start);
        let mut stroke = engine::Stroke::begin(
            settings,
            rgb(color),
            pixels_per_screen,
            drawing,
            engine::Sample::new(first.0, first.1, force, time),
        );
        if line_from.is_some() {
            stroke.add(drawing, engine::Sample::new(start.0, start.1, force, time));
        }
        app.session.raster.stroke = Some(ActiveStroke {
            stroke,
            canvas,
            history_mark: app.history.snapshot_revision(),
        });
        app.state.mark_artwork_dirty();
        ctx.request_repaint();
        if !released {
            return;
        }
    }
    let Some(mut active) = app.session.raster.stroke.take() else {
        return;
    };
    if app.history.snapshot_revision() != active.history_mark {
        // Undone while painting: the stroke goes with it.
        return;
    }
    let canvas = active.canvas;
    let Some(drawing) = raster_mut(&mut app.state.project, canvas.asset_id) else {
        return;
    };
    if moves.is_empty() {
        active.stroke.hold(drawing, time);
    } else {
        for pos in moves {
            let (x, y) = canvas.local(view.screen_to_stage(pos));
            active
                .stroke
                .add(drawing, engine::Sample::new(x, y, force, time));
        }
    }
    if released {
        let end = active.stroke.nib();
        let changed = active.stroke.finish(drawing);
        if let Some((x, y)) = end {
            app.session.raster.last_end =
                Some((canvas.q0rg_id, canvas.layer_id, canvas.asset_id, x, y));
        }
        app.state.mark_dirty();
        if !changed {
            app.session.status = "nothing changed".to_string();
        }
    } else {
        app.session.raster.stroke = Some(active);
        app.state.mark_artwork_dirty();
        ctx.request_repaint();
    }
}

/// Whether the current layer can be painted at all, before anything is
/// made or snapshot.
fn check_layer(app: &EditorApp) -> Result<(), NoCanvas> {
    if in_room(app) {
        return Err(NoCanvas::Online);
    }
    let project = &app.state.project;
    let (q0rg_id, layer_id) = (app.session.current_q0rg_id, app.session.current_layer_id);
    if !is_raster_layer(project, q0rg_id, layer_id) {
        return Err(NoCanvas::NotRaster);
    }
    if project.layer_is_locked(q0rg_id, layer_id) {
        return Err(NoCanvas::Locked);
    }
    if !project.layer_is_visible(q0rg_id, layer_id) {
        return Err(NoCanvas::Hidden);
    }
    Ok(())
}

/// The id a screen pick waits under, and where the swatch it fills is kept.
fn screen_pick_owner() -> egui::Id {
    egui::Id::new("raster_screen_pick")
}

/// Takes the colour under `pointer` into a swatch: the active one for the
/// picker, the stroke for Alt-click while painting. Raster paint is read
/// straight from the drawings; anything else (vector art, the paper) is
/// read off the screen, as the vector Eyedropper does, a frame later.
fn pick(app: &mut EditorApp, view: &StageView, pointer: Pos2, ctx: &Context, active_slot: bool) {
    use crate::color_panel::Slot;
    let slot = if active_slot {
        crate::color_panel::active_slot(ctx)
    } else {
        Slot::Stroke
    };
    let stage = view.screen_to_stage(pointer);
    let picked = if app.settings.raster.pick_all_layers {
        sample_raster(app, stage)
    } else {
        find_canvas(app).ok().flatten().and_then(|canvas| {
            let drawing = raster_ref(&app.state.project, canvas.asset_id)?;
            let (x, y) = canvas.local(stage);
            let mut pixel = drawing.pixel(x.floor() as i32, y.floor() as i32);
            q0s_format::pixels::unpremultiply(&mut pixel);
            (pixel[3] > 0).then_some(pixel)
        })
    };
    match picked {
        Some([r, g, b, _]) => take_color(app, slot, Rgba { r, g, b, a: 255 }),
        None if app.settings.raster.pick_all_layers => {
            let owner = screen_pick_owner();
            ctx.data_mut(|data| data.insert_temp(owner.with("slot"), slot == Slot::Stroke));
            crate::color_picker::request_pixel(ctx, owner, pointer);
        }
        None => app.session.status = "nothing drawn there on this drawing".to_string(),
    }
}

/// A colour a screen pick asked for, once it has come.
fn take_screen_pick(app: &mut EditorApp, ctx: &Context) {
    let owner = screen_pick_owner();
    let Some(color) = crate::color_picker::take_pixel(ctx, owner) else {
        return;
    };
    let to_stroke = ctx.data(|data| data.get_temp::<bool>(owner.with("slot")).unwrap_or(false));
    let slot = if to_stroke {
        crate::color_panel::Slot::Stroke
    } else {
        crate::color_panel::Slot::Fill
    };
    let [r, g, b, _] = color.to_array();
    take_color(app, slot, Rgba { r, g, b, a: 255 });
}

fn take_color(app: &mut EditorApp, slot: crate::color_panel::Slot, color: Rgba) {
    use crate::color_panel::{Paint, Slot};
    crate::color_panel::set_tool_paint(&mut app.session, slot, &Paint::Color(color));
    app.session.status = format!(
        "picked #{:02X}{:02X}{:02X} into the {} colour",
        color.r,
        color.g,
        color.b,
        match slot {
            Slot::Fill => "fill",
            Slot::Stroke => "stroke",
        }
    );
}

/// The raster drawings shown on the current frame of the edited timeline,
/// bottom to top, each with its place (drawing pixels to the timeline's
/// space). Hidden layers, folders and masks are left out. Nothing is drawn
/// to find them, so this is cheap enough for every frame.
pub fn shown_drawings(app: &EditorApp) -> Vec<(u16, Affine)> {
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut shown = Vec::new();
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_metadata(q0rg_id, layer.layer_id).kind != LayerKind::Normal
            || layer
                .timeline_frame_count
                .is_some_and(|count| frame >= count)
        {
            continue;
        }
        let parent = q0s_format::parenting::parent_world(project, q0rg_id, layer.layer_id, frame);
        for (index, transform) in q0s_format::raster::active_placements_at(layer, frame) {
            let Some(Target::Asset(asset_id)) = layer.placements.get(index).map(|p| p.target)
            else {
                continue;
            };
            if raster_ref(project, asset_id).is_none() {
                continue;
            }
            let own = Affine::from_transform(transform);
            shown.push((
                asset_id,
                parent.map_or(own, |parent| Affine::compose(parent, own)),
            ));
        }
    }
    shown
}

/// The raster paint at `stage` on the current frame: the topmost shown
/// drawing with paint there, straight alpha.
pub fn sample_raster(app: &EditorApp, stage: Vec2) -> Option<[u8; 4]> {
    for (asset_id, world) in shown_drawings(app).into_iter().rev() {
        let Some(local) = world.inverse().map(|inverse| inverse.apply(stage)) else {
            continue;
        };
        let Some(drawing) = raster_ref(&app.state.project, asset_id) else {
            continue;
        };
        let mut pixel = drawing.pixel(local.x.floor() as i32, local.y.floor() as i32);
        if pixel[3] > 0 {
            q0s_format::pixels::unpremultiply(&mut pixel);
            return Some(pixel);
        }
    }
    None
}

#[cfg(test)]
mod tests;
