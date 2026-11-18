//! The paint the tools draw with, and the Color panel that edits it.
//!
//! Fill and stroke each hold a colour, and either may be none, though not
//! both: a shape keeps one of them. The fill can be a linear or radial
//! gradient as well. The toolbar's two swatches and the Color panel edit
//! them. While something is selected they show its paint, and a change
//! repaints it too, as one undo step however many frames a drag lasts.

use std::collections::BTreeMap;

use egui::{pos2, vec2, Align2, Color32, Context, FontId, Id, Painter, Rect, Sense, Stroke, Ui};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, GradientKind, Placement, ProjectV2, Rgba, Stroke as VStroke, Target, VectorAppearance,
    VectorMaterial,
};

use crate::app::EditorApp;
use crate::color_picker_ui as face;
use crate::paint::PaintGradient;
use crate::state::{History, PlacementRef, Selection, Session};

// ---------------------------------------------------------------- slots

/// Fill or stroke: which paint the Color panel and the toolbar's "none"
/// button edit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    #[default]
    Fill,
    Stroke,
}

impl Slot {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Slot::Fill => "Fill",
            Slot::Stroke => "Stroke",
        }
    }
}

fn slot_id() -> Id {
    Id::new("color_panel_slot")
}

/// The slot the Color panel edits: the swatch clicked last.
pub(crate) fn active_slot(ctx: &Context) -> Slot {
    ctx.data(|data| data.get_temp::<Slot>(slot_id()))
        .unwrap_or_default()
}

pub(crate) fn set_active_slot(ctx: &Context, slot: Slot) {
    ctx.data_mut(|data| data.insert_temp(slot_id(), slot));
}

// ---------------------------------------------------------------- paint

/// A new paint for a slot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    /// No fill, or no stroke.
    None,
    Color(Rgba),
    /// Fills only; a stroke takes its first colour. `refit` places the
    /// gradient on each drawing afresh; otherwise only its colours change
    /// and it stays where it sits.
    Gradient {
        gradient: PaintGradient,
        refit: bool,
    },
}

/// How a slot looks: its colour (kept while it is none), its gradient and
/// whether it is none.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub(crate) color: Rgba,
    pub(crate) gradient: Option<PaintGradient>,
    pub(crate) none: bool,
}

/// The kinds the Color panel offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Solid,
    Linear,
    Radial,
    Nothing,
}

impl Look {
    fn kind(&self) -> Kind {
        if self.none {
            return Kind::Nothing;
        }
        match self.gradient.as_ref().map(|gradient| gradient.kind) {
            None => Kind::Solid,
            Some(GradientKind::Linear) => Kind::Linear,
            Some(GradientKind::Radial) => Kind::Radial,
        }
    }

    /// The paint this look stands for.
    pub(crate) fn paint(&self, refit: bool) -> Paint {
        if self.none {
            Paint::None
        } else if let Some(gradient) = &self.gradient {
            Paint::Gradient {
                gradient: gradient.clone(),
                refit,
            }
        } else {
            Paint::Color(self.color)
        }
    }
}

/// The tools' own paint in `slot`.
pub(crate) fn tool_look(session: &Session, slot: Slot) -> Look {
    match slot {
        Slot::Fill => Look {
            color: session.fill_color.unwrap_or(session.fill_kept),
            gradient: session.fill_gradient.clone(),
            none: session.fill_color.is_none(),
        },
        Slot::Stroke => Look {
            color: session.stroke_color,
            gradient: None,
            none: session.stroke_none,
        },
    }
}

/// What the swatches and the Color panel show for `slot`: the selection's
/// paint when it has one, else the tools'. The UI asks [`look_now`].
#[cfg(test)]
pub(crate) fn look(app: &EditorApp, slot: Slot) -> Look {
    selection_look(&app.state.project, &app.session.selection, slot)
        .unwrap_or_else(|| tool_look(&app.session, slot))
}

/// The selection's paint, worked out at most once a frame per slot while
/// the project, the undo history and the selection stay as they are.
#[derive(Clone)]
struct LookMemo {
    key: (u64, u64, u64),
    selection: Selection,
    looks: [Option<Option<Look>>; 2],
}

/// [`look`] for the toolbar and the panel, which ask several times a frame.
pub(crate) fn look_now(app: &EditorApp, ctx: &Context, slot: Slot) -> Look {
    let id = Id::new("color_panel_look_memo");
    let key = (
        ctx.frame_nr(),
        app.state.revision(),
        app.history.snapshot_revision(),
    );
    let index = slot as usize;
    let mut memo = ctx
        .data(|data| data.get_temp::<LookMemo>(id))
        .filter(|memo| memo.key == key && memo.selection == app.session.selection)
        .unwrap_or_else(|| LookMemo {
            key,
            selection: app.session.selection.clone(),
            looks: [None, None],
        });
    let selected = match &memo.looks[index] {
        Some(known) => known.clone(),
        None => {
            let found = selection_look(&app.state.project, &app.session.selection, slot);
            memo.looks[index] = Some(found.clone());
            ctx.data_mut(|data| data.insert_temp(id, memo));
            found
        }
    };
    selected.unwrap_or_else(|| tool_look(&app.session, slot))
}

fn selection_look(project: &ProjectV2, selection: &Selection, slot: Slot) -> Option<Look> {
    if matches!(selection, Selection::None) {
        return None;
    }
    let raw = crate::tools::selected_paint(project, selection);
    match slot {
        Slot::Fill => {
            if let Some((color, _)) = raw.fill {
                return Some(Look {
                    color,
                    gradient: raw.fill_gradient,
                    none: false,
                });
            }
        }
        Slot::Stroke => {
            if let Some((stroke, _)) = raw.stroke {
                return Some(Look {
                    color: stroke.color,
                    gradient: None,
                    none: false,
                });
            }
        }
    }
    let (_, asset_id) = paintable_objects(project, selection).into_iter().next()?;
    let Some(Asset::Vector(vector)) = asset(project, asset_id) else {
        return None;
    };
    Some(match slot {
        Slot::Fill => Look {
            color: vector.fill.unwrap_or(crate::state::DEFAULT_FILL),
            gradient: vector.fill.and_then(|_| {
                project
                    .asset_appearances
                    .get(&asset_id)?
                    .fill_gradient
                    .as_ref()
                    .map(|fill| PaintGradient::from_fill(fill, crate::paint::vector_bounds(vector)))
            }),
            none: vector.fill.is_none(),
        },
        Slot::Stroke => Look {
            color: vector
                .stroke
                .map_or(crate::state::DEFAULT_STROKE, |stroke| stroke.color),
            gradient: None,
            none: vector.stroke.is_none(),
        },
    })
}

/// Why none was refused: the other paint is none already.
pub(crate) const BOTH_NONE: &str =
    "The fill and the stroke cannot both be none: a shape keeps one of them";

/// Whether `slot` may be made none now: only while the other one is not.
pub(crate) fn none_allowed(session: &Session, slot: Slot) -> bool {
    match slot {
        Slot::Fill => !session.stroke_none,
        Slot::Stroke => session.fill_color.is_some(),
    }
}

/// Puts `paint` in the tools' `slot`. None is refused while the other slot
/// is none already, so a shape always keeps its fill or its stroke.
/// Returns whether the paint was taken.
pub(crate) fn set_tool_paint(session: &mut Session, slot: Slot, paint: &Paint) -> bool {
    if *paint == Paint::None && !none_allowed(session, slot) {
        return false;
    }
    match (slot, paint) {
        (Slot::Fill, Paint::None) => {
            if let Some(color) = session.fill_color.take() {
                session.fill_kept = color;
            }
        }
        (Slot::Fill, Paint::Color(color)) => {
            session.fill_color = Some(*color);
            session.fill_kept = *color;
            session.fill_gradient = None;
        }
        (Slot::Fill, Paint::Gradient { gradient, .. }) => {
            // the tools' own colour follows the first stop, so a gradient
            // turned off later leaves a colour from it
            let first = gradient
                .stops
                .first()
                .map_or(session.fill_kept, |stop| stop.color);
            session.fill_color = Some(first);
            session.fill_kept = first;
            session.fill_gradient = Some(gradient.clone());
        }
        (Slot::Stroke, Paint::None) => session.stroke_none = true,
        (Slot::Stroke, Paint::Color(color)) => {
            session.stroke_color = *color;
            session.stroke_none = false;
        }
        (Slot::Stroke, Paint::Gradient { gradient, .. }) => {
            if let Some(stop) = gradient.stops.first() {
                session.stroke_color = stop.color;
            }
            session.stroke_none = false;
        }
    }
    true
}

// ---------------------------------------------------------------- undo steps

/// Makes one undo step of a change however many frames it lasts: the
/// snapshot is taken before the first change after [`ColorGesture::arm`],
/// and again only if something else took one in between.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ColorGesture {
    armed: bool,
    revision: Option<u64>,
}

impl ColorGesture {
    /// The next change starts a new undo step: a click, the start of a
    /// drag, a popup opening.
    pub(crate) fn arm(&mut self) {
        self.armed = true;
    }

    /// Call before changing the project.
    pub(crate) fn before_change(
        &mut self,
        history: &mut History,
        project: &ProjectV2,
        session: &Session,
    ) {
        if self.armed || self.revision != Some(history.snapshot_revision()) {
            history.snapshot_with_session(project, session);
            self.revision = Some(history.snapshot_revision());
            self.armed = false;
        }
    }
}

fn gesture_id() -> Id {
    Id::new("color_panel_gesture")
}

/// Starts a new undo step for the next colour change.
pub(crate) fn arm(ctx: &Context) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<ColorGesture>(gesture_id())
            .arm()
    });
}

/// Puts `paint` in the tools' `slot` and paints what is selected with it.
/// `start` begins a new undo step; a drag passes it on its first change
/// only. Returns whether the selection changed.
pub(crate) fn apply(
    app: &mut EditorApp,
    ctx: &Context,
    slot: Slot,
    paint: &Paint,
    start: bool,
) -> bool {
    let mut gesture = ctx
        .data(|data| data.get_temp::<ColorGesture>(gesture_id()))
        .unwrap_or_default();
    if start {
        gesture.arm();
    }
    let changed = apply_with(app, slot, paint, &mut gesture);
    ctx.data_mut(|data| data.insert_temp(gesture_id(), gesture));
    changed
}

/// [`apply`] with the undo step kept by the caller.
pub(crate) fn apply_with(
    app: &mut EditorApp,
    slot: Slot,
    paint: &Paint,
    gesture: &mut ColorGesture,
) -> bool {
    if !set_tool_paint(&mut app.session, slot, paint) {
        app.session.status = BOTH_NONE.to_string();
        return false;
    }
    if !selection_takes(app, slot, paint) {
        if *paint == Paint::None
            && selection_look(&app.state.project, &app.session.selection, slot).is_some()
        {
            app.session.status = match slot {
                Slot::Fill => "The selected fills stay: a shape needs its fill or its stroke (Delete removes them)",
                Slot::Stroke => "The selected lines stay: a shape needs its fill or its stroke (Delete removes them)",
            }
            .to_string();
        }
        return false;
    }
    gesture.before_change(&mut app.history, &app.state.project, &app.session);
    paint_selection(app, slot, paint)
}

/// Swaps the fill and the stroke, of the tools and of what is selected.
pub(crate) fn swap(app: &mut EditorApp, ctx: &Context) {
    let fill = tool_look(&app.session, Slot::Fill);
    let stroke = tool_look(&app.session, Slot::Stroke);
    let new_fill = if stroke.none {
        Paint::None
    } else {
        Paint::Color(stroke.color)
    };
    let new_stroke = if fill.none {
        Paint::None
    } else {
        Paint::Color(
            fill.gradient
                .as_ref()
                .and_then(|g| g.stops.first())
                .map_or(fill.color, |stop| stop.color),
        )
    };
    set_both(app, ctx, new_fill, new_stroke);
}

/// Black stroke and white fill, for the tools and what is selected.
pub(crate) fn default_colors(app: &mut EditorApp, ctx: &Context) {
    set_both(
        app,
        ctx,
        Paint::Color(crate::state::DEFAULT_FILL),
        Paint::Color(crate::state::DEFAULT_STROKE),
    );
}

/// Sets both slots as one undo step, the one that is not none first so
/// the pair never passes through "both none".
fn set_both(app: &mut EditorApp, ctx: &Context, fill: Paint, stroke: Paint) {
    let mut gesture = ColorGesture::default();
    gesture.arm();
    if fill == Paint::None {
        apply_with(app, Slot::Stroke, &stroke, &mut gesture);
        apply_with(app, Slot::Fill, &fill, &mut gesture);
    } else {
        apply_with(app, Slot::Fill, &fill, &mut gesture);
        apply_with(app, Slot::Stroke, &stroke, &mut gesture);
    }
    ctx.data_mut(|data| data.insert_temp(gesture_id(), gesture));
}

// ---------------------------------------------------------------- the selection

fn asset(project: &ProjectV2, asset_id: u16) -> Option<&Asset> {
    project.assets.iter().find(|asset| asset.id() == asset_id)
}

fn placement(project: &ProjectV2, reference: PlacementRef) -> Option<&Placement> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)
}

fn placement_mut(project: &mut ProjectV2, reference: PlacementRef) -> Option<&mut Placement> {
    project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get_mut(reference.placement_idx)
}

/// The selected objects a colour can repaint, with their drawings: plain
/// drawings only. Symbols, bitmaps, text and drawings named in the library
/// keep their paint.
fn paintable_objects(project: &ProjectV2, selection: &Selection) -> Vec<(PlacementRef, u16)> {
    crate::selection_edit::selected_object_refs(project, selection)
        .into_iter()
        .filter_map(|reference| {
            let Target::Asset(asset_id) = placement(project, reference)?.target else {
                return None;
            };
            if project.asset_names.contains_key(&asset_id) {
                return None;
            }
            matches!(asset(project, asset_id), Some(Asset::Vector(_)))
                .then_some((reference, asset_id))
        })
        .collect()
}

fn plain_appearance() -> VectorAppearance {
    VectorAppearance {
        material: VectorMaterial::Solid,
        erase_mask: Vec::new(),
        material_source: Vec::new(),
        clip_mask: Vec::new(),
        field_transform: Affine::IDENTITY,
        fill_gradient: None,
    }
}

/// The stroke a drawing gets when a stroke colour is picked for it and it
/// had none: the tools' width, ends and corners.
fn new_stroke(session: &Session, color: Rgba) -> VStroke {
    VStroke {
        color,
        width: session.stroke_width.max(0.1),
        cap: session.brush_cap,
        join: session.brush_join,
    }
}

/// The one colour a stroke takes from `paint`.
fn stroke_color_of(paint: &Paint) -> Option<Rgba> {
    match paint {
        Paint::None => None,
        Paint::Color(color) => Some(*color),
        Paint::Gradient { gradient, .. } => gradient.stops.first().map(|stop| stop.color),
    }
}

/// Paints a drawing's `slot`; says whether anything changed. A fill only
/// goes onto drawings with a closed outline, and none only takes a fill or
/// stroke away while the other stays, so a drawing never vanishes.
fn repaint(
    vector: &mut q0s_format::v2::VectorAsset,
    gradient: &mut Option<q0s_format::v2::GradientFill>,
    slot: Slot,
    paint: &Paint,
    stroke: VStroke,
) -> bool {
    let before = (vector.fill, vector.stroke, gradient.clone());
    let fillable = vector.fill.is_some() || vector.paths.iter().any(|path| path.closed);
    match (slot, paint) {
        (Slot::Fill, Paint::None) => {
            if vector.stroke.is_some() {
                vector.fill = None;
                *gradient = None;
            }
        }
        (Slot::Fill, Paint::Color(color)) => {
            if fillable {
                vector.fill = Some(*color);
                *gradient = None;
            }
        }
        (
            Slot::Fill,
            Paint::Gradient {
                gradient: pick,
                refit,
            },
        ) => {
            if fillable {
                // the drawing's own colour follows the first stop
                vector.fill = pick
                    .stops
                    .first()
                    .map(|stop| stop.color)
                    .or(vector.fill)
                    .or(Some(crate::state::DEFAULT_FILL));
                let bounds = crate::paint::vector_bounds(vector).unwrap_or((
                    q0s_format::v2::Vec2::new(-50.0, -50.0),
                    q0s_format::v2::Vec2::new(50.0, 50.0),
                ));
                *gradient = Some(match gradient.as_ref() {
                    Some(old) if !*refit && old.kind == pick.kind => q0s_format::v2::GradientFill {
                        kind: pick.kind,
                        stops: pick.stops.clone(),
                        transform: old.transform,
                    },
                    _ => pick.fit(bounds),
                });
            }
        }
        (Slot::Stroke, Paint::None) => {
            if vector.fill.is_some() {
                vector.stroke = None;
            }
        }
        (Slot::Stroke, paint) => {
            if let Some(color) = stroke_color_of(paint) {
                match vector.stroke.as_mut() {
                    Some(old) => old.color = color,
                    None => vector.stroke = Some(VStroke { color, ..stroke }),
                }
            }
        }
    }
    before != (vector.fill, vector.stroke, gradient.clone())
}

/// Whether painting drawing `asset_id` would change it.
fn would_repaint(
    project: &ProjectV2,
    asset_id: u16,
    slot: Slot,
    paint: &Paint,
    stroke: VStroke,
) -> bool {
    let Some(Asset::Vector(vector)) = asset(project, asset_id) else {
        return false;
    };
    let mut vector = vector.clone();
    let mut gradient = project
        .asset_appearances
        .get(&asset_id)
        .and_then(|appearance| appearance.fill_gradient.clone());
    repaint(&mut vector, &mut gradient, slot, paint, stroke)
}

/// Paints drawing `asset_id` in place; says whether it changed.
fn repaint_drawing(
    project: &mut ProjectV2,
    asset_id: u16,
    slot: Slot,
    paint: &Paint,
    stroke: VStroke,
) -> bool {
    let old_gradient = project
        .asset_appearances
        .get(&asset_id)
        .and_then(|appearance| appearance.fill_gradient.clone());
    let mut gradient = old_gradient.clone();
    let Some(Asset::Vector(vector)) = project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
    else {
        return false;
    };
    let changed = repaint(vector, &mut gradient, slot, paint, stroke);
    if gradient != old_gradient {
        match gradient {
            Some(gradient) => {
                project
                    .asset_appearances
                    .entry(asset_id)
                    .or_insert_with(plain_appearance)
                    .fill_gradient = Some(gradient);
            }
            None => {
                let plain = plain_appearance();
                if let Some(appearance) = project.asset_appearances.get_mut(&asset_id) {
                    appearance.fill_gradient = None;
                    if *appearance == plain {
                        project.asset_appearances.remove(&asset_id);
                    }
                }
            }
        }
    }
    changed
}

/// Gives the selected objects drawings of their own where others use the
/// same one (another keyframe, another object), so a colour changes only
/// what is selected. Returns the drawings to paint.
fn own_drawings(project: &mut ProjectV2, objects: &[(PlacementRef, u16)]) -> Vec<u16> {
    let mut chosen: BTreeMap<u16, Vec<PlacementRef>> = BTreeMap::new();
    for (reference, asset_id) in objects {
        chosen.entry(*asset_id).or_default().push(*reference);
    }
    let mut owned = Vec::with_capacity(chosen.len());
    for (asset_id, references) in chosen {
        let uses = project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .filter(|placement| placement.target == Target::Asset(asset_id))
            .count();
        if uses <= references.len() {
            owned.push(asset_id);
            continue;
        }
        let Some(Asset::Vector(mut copy)) = asset(project, asset_id).cloned() else {
            continue;
        };
        let copy_id = crate::tools::next_asset_id_pub(project);
        copy.asset_id = copy_id;
        project.assets.push(Asset::Vector(copy));
        crate::appearance::clone_asset_appearance(project, asset_id, copy_id);
        for reference in references {
            if let Some(placement) = placement_mut(project, reference) {
                placement.target = Target::Asset(copy_id);
            }
        }
        owned.push(copy_id);
    }
    owned
}

/// Whether painting the selection's `slot` with `paint` would change it.
fn selection_takes(app: &EditorApp, slot: Slot, paint: &Paint) -> bool {
    let project = &app.state.project;
    let selection = &app.session.selection;
    if matches!(selection, Selection::None) {
        return false;
    }
    let raw = crate::tools::selected_paint(project, selection);
    let raw_changes = match (slot, paint) {
        (_, Paint::None) => false,
        (Slot::Fill, Paint::Color(color)) => raw
            .fill
            .is_some_and(|(fill, mixed)| mixed || fill != *color || raw.fill_gradient.is_some()),
        (Slot::Fill, Paint::Gradient { gradient, refit }) => {
            raw.fill.is_some()
                && (*refit
                    || raw.fill_gradient.as_ref().map(|old| (old.kind, &old.stops))
                        != Some((gradient.kind, &gradient.stops)))
        }
        (Slot::Stroke, paint) => match (raw.stroke, stroke_color_of(paint)) {
            (Some((stroke, mixed)), Some(color)) => mixed || stroke.color != color,
            _ => false,
        },
    };
    if raw_changes {
        return true;
    }
    let stroke = new_stroke(&app.session, app.session.stroke_color);
    paintable_objects(project, selection)
        .iter()
        .any(|(_, asset_id)| would_repaint(project, *asset_id, slot, paint, stroke))
}

/// Paints the selection's `slot`: the selected raw fills or lines, and
/// the selected objects' drawings. Says whether anything changed.
fn paint_selection(app: &mut EditorApp, slot: Slot, paint: &Paint) -> bool {
    let objects = paintable_objects(&app.state.project, &app.session.selection);
    let stroke = new_stroke(&app.session, app.session.stroke_color);
    let worth: Vec<(PlacementRef, u16)> = objects
        .into_iter()
        .filter(|(_, asset_id)| would_repaint(&app.state.project, *asset_id, slot, paint, stroke))
        .collect();
    let mut changed_assets = Vec::new();
    for asset_id in own_drawings(&mut app.state.project, &worth) {
        if repaint_drawing(&mut app.state.project, asset_id, slot, paint, stroke) {
            changed_assets.push(asset_id);
        }
    }
    let objects_changed = !changed_assets.is_empty();
    if objects_changed {
        app.textures.invalidate_assets(changed_assets);
        app.state.mark_dirty();
    }

    // Raw parts. Their helpers may lift parts into drawings of their own
    // and select those, which would drop the selected objects: keep them.
    let kept_objects: Vec<(PlacementRef, Target, u16)> =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
            .into_iter()
            .filter_map(|reference| {
                let placement = placement(&app.state.project, reference)?;
                Some((reference, placement.target, placement.frame))
            })
            .collect();
    let raw_changed = match (slot, paint) {
        (_, Paint::None) => false,
        (Slot::Fill, Paint::Color(color)) => crate::tools::set_selected_fill(app, *color),
        (Slot::Fill, Paint::Gradient { gradient, refit }) => crate::tools::paint_selected_fills(
            app,
            crate::tools::FillPaint::Gradient {
                pick: Some(gradient),
                refit: *refit,
            },
        ),
        (Slot::Stroke, paint) => match stroke_color_of(paint) {
            Some(color) => {
                crate::tools::restyle_selected_strokes(app, move |stroke| stroke.color = color)
            }
            None => false,
        },
    };
    if raw_changed {
        keep_objects(app, &kept_objects);
    }
    objects_changed || raw_changed
}

/// Puts objects that were selected back into a selection a raw-part edit
/// replaced, as long as they are still where they were.
fn keep_objects(app: &mut EditorApp, objects: &[(PlacementRef, Target, u16)]) {
    if objects.is_empty()
        || !crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
            .is_empty()
    {
        return;
    }
    let kept: Vec<PlacementRef> = objects
        .iter()
        .filter(|(reference, target, frame)| {
            placement(&app.state.project, *reference)
                .is_some_and(|placement| placement.target == *target && placement.frame == *frame)
        })
        .map(|(reference, ..)| *reference)
        .collect();
    if kept.is_empty() {
        return;
    }
    app.session.selection = match std::mem::replace(&mut app.session.selection, Selection::None) {
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => Selection::Mixed {
            paths: vec![crate::state::PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            }],
            objects: kept,
        },
        Selection::Paths(paths) => Selection::Mixed {
            paths,
            objects: kept,
        },
        Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            ..
        } => Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            objects: kept,
        },
        other => other,
    };
}

// ---------------------------------------------------------------- colour maths

/// Hue in degrees, saturation and brightness in percent, rounded as the
/// panel's fields show them.
pub(crate) fn hsb_of_rgb(rgb: [u8; 3]) -> [u16; 3] {
    let [h, s, v] = face::rgb_to_hsv(rgb.map(|channel| f32::from(channel) / 255.0));
    [
        ((h * 360.0).round() as u16) % 360,
        (s * 100.0).round() as u16,
        (v * 100.0).round() as u16,
    ]
}

/// `RRGGBB` in capitals, without `#`.
pub(crate) fn hex_of_rgb(rgb: [u8; 3]) -> String {
    face::hex_of_values(rgb, 255, false)
}

/// A typed hex code: 3 or 6 digits, or 8 with the opacity last; with or
/// without `#`. Gives the colour and, from 8 digits, the opacity.
pub(crate) fn parse_hex(text: &str) -> Option<([u8; 3], Option<u8>)> {
    let digits: String = text
        .trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pair = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    match digits.len() {
        3 => {
            let one = |i: usize| u8::from_str_radix(&digits[i..=i], 16).ok().map(|v| v * 17);
            Some(([one(0)?, one(1)?, one(2)?], None))
        }
        6 => Some(([pair(0)?, pair(2)?, pair(4)?], None)),
        8 => Some(([pair(0)?, pair(2)?, pair(4)?], Some(pair(6)?))),
        _ => None,
    }
}

/// What the panel keeps for the colour it edits: hue, saturation and
/// brightness, so a grey keeps its hue and black its saturation while
/// they are dragged through, and the hex code being typed.
#[derive(Clone, Debug, Default)]
struct Fields {
    hsv: [f32; 3],
    alpha: f32,
    shown: Option<Rgba>,
    hex: String,
    hex_editing: bool,
}

impl Fields {
    /// Takes on a colour from outside.
    fn sync(&mut self, color: Rgba) {
        let [h, s, v] = face::rgb_to_hsv([color.r, color.g, color.b].map(|c| f32::from(c) / 255.0));
        if v > 1.0e-4 {
            if s > 1.0e-4 {
                self.hsv[0] = h;
            }
            self.hsv[1] = s;
        }
        self.hsv[2] = v;
        self.alpha = f32::from(color.a) / 255.0;
        self.shown = Some(color);
        if !self.hex_editing {
            self.hex = hex_of_rgb([color.r, color.g, color.b]);
        }
    }

    fn color(&self) -> Rgba {
        let [r, g, b] = face::hsv_to_rgb(self.hsv).map(face::byte);
        Rgba {
            r,
            g,
            b,
            a: face::byte(self.alpha),
        }
    }

    fn commit(&mut self) -> Rgba {
        let color = self.color();
        self.shown = Some(color);
        if !self.hex_editing {
            self.hex = hex_of_rgb([color.r, color.g, color.b]);
        }
        color
    }

    fn set_hsv(&mut self, hsv: [f32; 3]) -> Rgba {
        self.hsv = [
            hsv[0].rem_euclid(1.0),
            hsv[1].clamp(0.0, 1.0),
            hsv[2].clamp(0.0, 1.0),
        ];
        self.commit()
    }

    fn set_rgb(&mut self, rgb: [u8; 3]) -> Rgba {
        let alpha = face::byte(self.alpha);
        self.sync(Rgba {
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            a: alpha,
        });
        self.color_exact(rgb, alpha)
    }

    fn set_alpha(&mut self, alpha: f32) -> Rgba {
        self.alpha = alpha.clamp(0.0, 1.0);
        let color = self.shown.unwrap_or_else(|| self.color());
        let color = Rgba {
            a: face::byte(self.alpha),
            ..color
        };
        self.shown = Some(color);
        color
    }

    /// A typed colour keeps its own bytes, not a round trip through HSB.
    fn color_exact(&mut self, rgb: [u8; 3], alpha: u8) -> Rgba {
        let color = Rgba {
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            a: alpha,
        };
        self.shown = Some(color);
        color
    }
}

// ---------------------------------------------------------------- swatch faces

fn to_color32(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn edge(painter: &Painter, rect: Rect) {
    painter.rect_stroke(
        rect,
        1.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(150)),
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        0.5,
        Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
    );
}

/// A fill swatch: the colour, the gradient, or none.
pub(crate) fn paint_fill_swatch(painter: &Painter, rect: Rect, look: &Look) {
    crate::color_picker::paint_box_face(
        painter,
        rect,
        to_color32(look.color),
        look.gradient.as_ref(),
        look.none,
    );
    edge(painter, rect);
}

/// A stroke swatch: a hollow frame `band` thick in the stroke's colour, or
/// a white one crossed in red when it is none. The frame is one closed
/// line, so no seams show where its sides meet.
pub(crate) fn paint_stroke_swatch(painter: &Painter, rect: Rect, look: &Look, band: f32) {
    let middle = rect.shrink(band * 0.5);
    let ring = |color: Color32| {
        painter.add(egui::Shape::closed_line(
            vec![
                middle.left_top(),
                middle.right_top(),
                middle.right_bottom(),
                middle.left_bottom(),
            ],
            Stroke::new(band, color),
        ));
    };
    if look.none {
        ring(Color32::WHITE);
    } else {
        let color = to_color32(look.color);
        if color.a() < 255 {
            // a light ground, so a see-through stroke reads as one
            ring(Color32::from_gray(200));
        }
        ring(color);
    }
    if look.none {
        painter.line_segment(
            [rect.left_bottom(), rect.right_top()],
            Stroke::new(1.75_f32, Color32::from_rgb(0xE0, 0x23, 0x2E)),
        );
    }
    painter.rect_stroke(
        rect.shrink(0.5),
        0.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(130)),
    );
}

/// The small buttons beside the swatches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mini {
    /// Black stroke, white fill.
    Default,
    /// Fill and stroke change places.
    Swap,
    /// The active one becomes none.
    None,
}

impl Mini {
    pub(crate) const ALL: [Mini; 3] = [Mini::Default, Mini::Swap, Mini::None];

    fn hint(self) -> &'static str {
        match self {
            Mini::Default => "Default colours: black stroke, white fill",
            Mini::Swap => "Swap the fill and stroke colours",
            Mini::None => "None: no fill, or no stroke (whichever is active)",
        }
    }
}

/// A small square button with `mini`'s picture, in `rect`.
pub(crate) fn mini_button(ui: &mut Ui, rect: Rect, mini: Mini, enabled: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.id().with(("color_mini", mini as u8)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(ui, &response, rect, 3.0, false);
        let painter = ui.painter_at(rect.expand(1.0));
        let ink = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color().gamma_multiply(0.6)
        };
        let r = rect.shrink((rect.width() * 0.16).round());
        match mini {
            Mini::Default => {
                let s = r.width() * 0.62;
                let white = Rect::from_min_size(r.min, vec2(s, s));
                let black = Rect::from_min_size(r.max - vec2(s, s), vec2(s, s));
                painter.rect_filled(black, 0.0, Color32::from_gray(20));
                painter.rect_stroke(black, 0.0, Stroke::new(1.0_f32, ink));
                painter.rect_filled(white, 0.0, Color32::WHITE);
                painter.rect_stroke(white, 0.0, Stroke::new(1.0_f32, Color32::from_gray(90)));
            }
            Mini::Swap => {
                // A quarter turn from the top right to the bottom left,
                // with a head at each end.
                let c = r.left_top() + vec2(r.width() * 0.1, r.height() * 0.1);
                let radius = r.width() * 0.78;
                let points: Vec<egui::Pos2> = (0..=10)
                    .map(|i| {
                        let a = std::f32::consts::FRAC_PI_2 * i as f32 / 10.0;
                        c + vec2(a.cos(), a.sin()) * radius
                    })
                    .collect();
                let stroke = Stroke::new(1.3_f32, ink);
                painter.add(egui::Shape::line(points.clone(), stroke));
                let head = r.width() * 0.26;
                let (a, b) = (points[0], points[10]);
                painter.line_segment([a, a + vec2(-head, -head * 0.7)], stroke);
                painter.line_segment([a, a + vec2(head * 0.2, -head)], stroke);
                painter.line_segment([b, b + vec2(-head * 0.7, -head)], stroke);
                painter.line_segment([b, b + vec2(-head, head * 0.2)], stroke);
            }
            Mini::None => {
                face::paint_none(&painter, r, 0.0);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::from_gray(90)));
            }
        }
    }
    response.on_hover_text(mini.hint())
}

/// Does what a small button says.
pub(crate) fn press_mini(app: &mut EditorApp, ctx: &Context, mini: Mini) {
    match mini {
        Mini::Default => default_colors(app, ctx),
        Mini::Swap => swap(app, ctx),
        Mini::None => {
            let slot = active_slot(ctx);
            apply(app, ctx, slot, &Paint::None, true);
        }
    }
}

// ---------------------------------------------------------------- the panel

fn fields_id(slot: Slot, stop: Option<usize>) -> Id {
    Id::new(("color_panel_fields", slot, stop))
}

/// Whose gradient stops these are: this panel's, for this slot.
fn gradient_owner(ui: &Ui, slot: Slot) -> Id {
    ui.id().with(("color_panel_gradient", slot))
}

/// The Color panel: fill and stroke, the kind of paint, a saturation and
/// brightness square with a hue strip, HSB, RGB, hex and opacity, a
/// preview, and the saved swatches.
pub(crate) fn render(app: &mut EditorApp, ui: &mut Ui) {
    egui::ScrollArea::vertical()
        .id_source("color_panel_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            crate::ui::inspector_style(ui);
            // Room at the sides for outlines, thumbs and the scroll bar,
            // so nothing is cut at the panel's edges.
            egui::Frame::none()
                .inner_margin(egui::Margin {
                    left: 7.0,
                    right: 8.0,
                    top: 4.0,
                    bottom: 6.0,
                })
                .show(ui, |ui| panel(app, ui));
        });
}

fn panel(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    // A press anywhere in the panel begins a new undo step; a drag that
    // follows it stays one.
    if ui.input(|input| input.pointer.any_pressed()) && ui.rect_contains_pointer(ui.clip_rect()) {
        arm(&ctx);
    }
    let slot = active_slot(&ctx);

    // ---- fill and stroke, the small buttons, square or wheel
    let mut field_buttons_below = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for which in [Slot::Fill, Slot::Stroke] {
            let shown = look_now(app, &ctx, which);
            let (rect, response) = ui.allocate_exact_size(vec2(34.0, 28.0), Sense::click());
            if ui.is_rect_visible(rect) {
                crate::ui::paint_control(ui, &response, rect, 5.0, which == slot);
                let swatch = Rect::from_center_size(rect.center(), vec2(18.0, 18.0));
                match which {
                    Slot::Fill => paint_fill_swatch(ui.painter(), swatch, &shown),
                    Slot::Stroke => paint_stroke_swatch(ui.painter(), swatch, &shown, 4.0),
                }
            }
            if response
                .on_hover_text(match which {
                    Slot::Fill => "Fill: edit the colour inside shapes",
                    Slot::Stroke => "Stroke: edit the colour of outlines and lines",
                })
                .clicked()
            {
                set_active_slot(&ctx, which);
            }
        }
        ui.add_space(6.0);
        let side = 18.0;
        for mini in Mini::ALL {
            let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let enabled = mini != Mini::None || none_allowed(&app.session, slot);
            if mini_button(ui, rect, mini, enabled).clicked() {
                press_mini(app, &ctx, mini);
            }
        }
        // Square or wheel, at the right while there is room.
        if ui.available_width() >= field_buttons_width() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                face::field_buttons(ui);
            });
        } else {
            field_buttons_below = true;
        }
    });
    if field_buttons_below {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            face::field_buttons(ui);
        });
    }
    let slot = active_slot(&ctx);
    let mut shown = look_now(app, &ctx, slot);

    // ---- the kind of paint
    let before = shown.kind();
    let mut kind = before;
    let none_ok = none_allowed(&app.session, slot);
    let width = ui.available_width();
    face::segmented_enabled(
        ui,
        width,
        &mut kind,
        &[
            (Kind::Solid, "Solid"),
            (Kind::Linear, "Linear"),
            (Kind::Radial, "Radial"),
            (Kind::Nothing, "None"),
        ],
        |option| match (slot, option) {
            (Slot::Stroke, Kind::Linear | Kind::Radial) => {
                Some("A stroke takes one colour; gradients are for fills")
            }
            (_, Kind::Nothing) if !none_ok && before != Kind::Nothing => Some(BOTH_NONE),
            _ => None,
        },
    );
    if kind != before {
        let paint = match kind {
            Kind::Solid => Paint::Color(shown.color),
            Kind::Nothing => Paint::None,
            Kind::Linear | Kind::Radial => {
                let gradient_kind = if kind == Kind::Linear {
                    GradientKind::Linear
                } else {
                    GradientKind::Radial
                };
                let mut gradient = shown
                    .gradient
                    .clone()
                    .unwrap_or_else(|| PaintGradient::starting_from(shown.color, gradient_kind));
                gradient.kind = gradient_kind;
                Paint::Gradient {
                    gradient,
                    refit: true,
                }
            }
        };
        apply(app, &ctx, slot, &paint, true);
        shown = look_now(app, &ctx, slot);
    }

    // ---- a gradient's stops: the fields below edit the selected one
    let mut stop = None;
    if let Some(mut gradient) = shown.gradient.clone().filter(|_| !shown.none) {
        ui.add_space(2.0);
        let owner = gradient_owner(ui, slot);
        // The stop pins reach past the ends of their bar: inset it.
        let inset = 6.0;
        let width = (ui.available_width() - inset * 2.0).max(60.0);
        let change = ui
            .horizontal(|ui| {
                ui.add_space(inset);
                ui.vertical(|ui| {
                    crate::color_picker::gradient_stops_editor(ui, owner, &mut gradient, width)
                })
                .inner
            })
            .inner;
        if change != crate::color_picker::PaintChange::None {
            apply(
                app,
                &ctx,
                slot,
                &Paint::Gradient {
                    gradient: gradient.clone(),
                    refit: change == crate::color_picker::PaintChange::Shape,
                },
                false,
            );
            shown = look_now(app, &ctx, slot);
        }
        stop = Some(crate::color_picker::selected_stop(
            &ctx,
            owner,
            gradient.stops.len(),
        ));
    }
    let target = match (&shown.gradient, stop) {
        (Some(gradient), Some(index)) if !shown.none => gradient
            .stops
            .get(index)
            .map_or(shown.color, |stop| stop.color),
        _ => shown.color,
    };
    let key = fields_id(slot, stop);
    let mut fields = ctx
        .data(|data| data.get_temp::<Fields>(key))
        .unwrap_or_default();
    if fields.shown != Some(target) {
        fields.sync(target);
    }
    let mut edited: Option<Rgba> = None;

    // ---- the square and hue strip, or the wheel, and the numbers
    ui.add_space(2.0);
    let width = ui.available_width();
    let strip = 14.0;
    let gap = 8.0;
    let numbers = NUMBERS_WIDTH;
    let beside = width >= 230.0;
    let wheel = face::prefers_wheel();
    let room = if beside { width - numbers - gap } else { width };
    let square_width = if beside {
        width - strip - numbers - gap * 2.0
    } else {
        width - strip - gap
    }
    .max(60.0);
    let square_height = square_width.min(if beside { 172.0 } else { 150.0 });
    let wheel_side = room.clamp(60.0, 210.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        if wheel {
            // centred in its room when the numbers go below
            let (area, _) = ui.allocate_exact_size(
                vec2(if beside { wheel_side } else { room.max(60.0) }, wheel_side),
                Sense::hover(),
            );
            let before = fields.shown;
            let key = ui.id().with(("color_panel_wheel", slot));
            if let Some(hsv) = face::hue_wheel(
                ui,
                key,
                Rect::from_center_size(area.center(), vec2(wheel_side, wheel_side)),
                fields.hsv,
                6.5,
            ) {
                let color = fields.set_hsv(hsv);
                if before != Some(color) {
                    edited = Some(color);
                }
            }
        } else {
            if let Some(color) = sb_square(ui, vec2(square_width, square_height), &mut fields) {
                edited = Some(color);
            }
            if let Some(color) = hue_strip(ui, vec2(strip, square_height), &mut fields) {
                edited = Some(color);
            }
        }
        if beside {
            ui.vertical(|ui| {
                if let Some(color) = number_fields(ui, &mut fields, slot) {
                    edited = Some(color);
                }
            });
        }
    });
    if !beside {
        if let Some(color) = number_fields(ui, &mut fields, slot) {
            edited = Some(color);
        }
    }

    // ---- hex and the preview
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new("#")
                .monospace()
                .color(ui.visuals().weak_text_color()),
        );
        let response = ui
            .add(
                egui::TextEdit::singleline(&mut fields.hex)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(66.0)
                    .char_limit(9)
                    .hint_text("RRGGBB"),
            )
            .on_hover_text("Hex code: 3 or 6 digits, or 8 with the opacity last");
        fields.hex_editing = response.has_focus();
        if response.changed() {
            if let Some((rgb, alpha)) = parse_hex(&fields.hex) {
                let alpha = alpha.unwrap_or(face::byte(fields.alpha));
                if Some(rgb) != fields.shown.map(|c| [c.r, c.g, c.b])
                    || Some(alpha) != fields.shown.map(|c| c.a)
                {
                    fields.sync(Rgba {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                        a: alpha,
                    });
                    edited = Some(fields.color_exact(rgb, alpha));
                }
            }
        }
        if response.lost_focus() {
            fields.hex_editing = false;
            if let Some(color) = fields.shown {
                fields.hex = hex_of_rgb([color.r, color.g, color.b]);
            }
        }
        let preview_width = ui.available_width().max(24.0);
        let (rect, response) = ui.allocate_exact_size(vec2(preview_width, 22.0), Sense::hover());
        let now = edited.unwrap_or(target);
        let [h, s, b] = hsb_of_rgb([now.r, now.g, now.b]);
        response.on_hover_text(if shown.none {
            "None".to_string()
        } else {
            format!(
                "H {h}\u{b0}  S {s}%  B {b}%\nR {}  G {}  B {}  \u{b7}  {}% opaque",
                now.r,
                now.g,
                now.b,
                (f32::from(now.a) / 2.55).round()
            )
        });
        let preview = Look {
            color: edited.unwrap_or(target),
            ..shown.clone()
        };
        let preview = match (&preview.gradient, stop, edited) {
            (Some(gradient), Some(index), Some(color)) => {
                let mut gradient = gradient.clone();
                if let Some(stop) = gradient.stops.get_mut(index) {
                    stop.color = color;
                }
                Look {
                    gradient: Some(gradient),
                    ..preview
                }
            }
            _ => preview,
        };
        paint_fill_swatch(ui.painter(), rect, &preview);
    });

    if let Some(color) = edited {
        let paint = match (&shown.gradient, stop) {
            (Some(gradient), Some(index)) if !shown.none => {
                let mut gradient = gradient.clone();
                if let Some(stop) = gradient.stops.get_mut(index) {
                    stop.color = color;
                }
                Paint::Gradient {
                    gradient,
                    refit: false,
                }
            }
            _ => Paint::Color(color),
        };
        apply(app, &ctx, slot, &paint, false);
    }
    ctx.data_mut(|data| data.insert_temp(key, fields.clone()));

    // ---- swatches
    crate::ui::section_header(ui, "Swatches");
    let saved = face::saved_swatches();
    let current = fields.shown.unwrap_or(target);
    let current_rgba = [current.r, current.g, current.b, current.a];
    let mut picked = None;
    let mut forget = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
        for (index, rgba) in saved.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
            let color = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
            if ui.is_rect_visible(rect) {
                face::tile(ui, rect, 2.0, color);
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
                );
                if *rgba == current_rgba && !shown.none {
                    ui.painter().rect_stroke(
                        rect.shrink(2.5),
                        1.0,
                        Stroke::new(1.25_f32, contrast(color)),
                    );
                }
                if response.hovered() {
                    face::hover_ring(ui, rect, 2.0);
                }
            }
            let response = response.on_hover_text(format!(
                "#{} \u{b7} right-click to remove",
                face::hex_of_values([rgba[0], rgba[1], rgba[2]], rgba[3], true)
            ));
            if response.clicked() {
                picked = Some(*rgba);
            }
            if response.secondary_clicked() {
                forget = Some(index);
            }
        }
    });
    if let Some(index) = forget {
        face::forget_swatch(index);
    }
    if let Some([r, g, b, a]) = picked {
        let color = Rgba { r, g, b, a };
        let paint = match (&shown.gradient, stop) {
            (Some(gradient), Some(index)) if !shown.none => {
                let mut gradient = gradient.clone();
                if let Some(stop) = gradient.stops.get_mut(index) {
                    stop.color = color;
                }
                Paint::Gradient {
                    gradient,
                    refit: false,
                }
            }
            _ => Paint::Color(color),
        };
        apply(app, &ctx, slot, &paint, true);
    }
    ui.add_space(2.0);
    let already = saved.contains(&current_rgba);
    if ui
        .add_enabled(
            !already && !shown.none,
            egui::Button::new("Add to swatches"),
        )
        .on_hover_text("Keep this colour in the swatches, here and in every colour picker")
        .on_disabled_hover_text(if shown.none {
            "None is not a colour to keep"
        } else {
            "This colour is in the swatches already"
        })
        .clicked()
    {
        face::save_swatch(current_rgba);
    }
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!(
            "{}: {}",
            slot.label(),
            if selection_look(&app.state.project, &app.session.selection, slot).is_some() {
                "the selection's paint. Changes repaint it and the tools"
            } else {
                "the paint new shapes and lines are drawn with"
            }
        ))
        .small()
        .color(ui.visuals().weak_text_color()),
    );
}

/// Black or white, whichever shows on `color`.
fn contrast(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    if u32::from(r) * 3 + u32::from(g) * 6 + u32::from(b) > 1300 {
        Color32::from_black_alpha(200)
    } else {
        Color32::from_white_alpha(230)
    }
}

/// The saturation (across) and brightness (up) square for the current hue.
fn sb_square(ui: &mut Ui, size: egui::Vec2, fields: &mut Fields) -> Option<Rgba> {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let hue = fields.hsv[0];
    let handle = face::texture(
        ui.ctx(),
        Id::new("color_panel_square"),
        face::hash_of(&[hue]),
        size,
        |x, y| face::color_of(face::hsv_to_rgb([hue, x / size.x, 1.0 - y / size.y]), 1.0),
    );
    face::image(ui.painter(), rect, 3.0, &handle);
    ui.painter()
        .rect_stroke(rect.shrink(0.5), 2.5, face::outline(ui));
    let mut changed = None;
    if response.is_pointer_button_down_on() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let s = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let v = 1.0 - ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let before = fields.shown;
            let color = fields.set_hsv([hue, s, v]);
            if before != Some(color) {
                changed = Some(color);
            }
        }
    }
    let center = pos2(
        rect.left() + fields.hsv[1] * rect.width(),
        rect.top() + (1.0 - fields.hsv[2]) * rect.height(),
    );
    let painter = ui.painter_at(rect.expand(8.0));
    painter.circle_stroke(
        center,
        5.5,
        Stroke::new(2.5_f32, Color32::from_black_alpha(110)),
    );
    painter.circle_stroke(center, 5.0, Stroke::new(1.5_f32, Color32::WHITE));
    changed
}

/// The hue strip beside the square, red at the top.
fn hue_strip(ui: &mut Ui, size: egui::Vec2, fields: &mut Fields) -> Option<Rgba> {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let handle = face::texture(ui.ctx(), Id::new("color_panel_hue"), 0, size, |_, y| {
        face::color_of(face::hsv_to_rgb([y / size.y, 1.0, 1.0]), 1.0)
    });
    face::image(ui.painter(), rect, 3.0, &handle);
    ui.painter()
        .rect_stroke(rect.shrink(0.5), 2.5, face::outline(ui));
    let mut changed = None;
    if response.is_pointer_button_down_on() {
        if let Some(pointer) = response.interact_pointer_pos() {
            // just short of 1 at the bottom, so the bottom stays red there
            let hue = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 0.9999);
            let [_, s, v] = fields.hsv;
            let before = fields.shown;
            let color = fields.set_hsv([hue, s, v]);
            fields.hsv[0] = hue;
            if before != Some(color) {
                changed = Some(color);
            }
        }
    }
    let y = rect.top() + fields.hsv[0] * rect.height();
    let marker = Rect::from_center_size(pos2(rect.center().x, y), vec2(rect.width() + 4.0, 5.0));
    let painter = ui.painter_at(rect.expand(4.0));
    painter.rect_stroke(
        marker,
        1.5,
        Stroke::new(2.5_f32, Color32::from_black_alpha(120)),
    );
    painter.rect_stroke(marker, 1.5, Stroke::new(1.25_f32, Color32::WHITE));
    changed
}

/// The numbers' width: a letter, a gap and a field.
const NUMBERS_WIDTH: f32 = 14.0 + 6.0 + 62.0;

/// Room the square and wheel buttons take.
fn field_buttons_width() -> f32 {
    face::icon_button_side() * 2.0 + 2.0
}

/// H, S and B, then R, G and B, then the opacity, as numbers to drag or
/// type.
fn number_fields(ui: &mut Ui, fields: &mut Fields, slot: Slot) -> Option<Rgba> {
    let mut edited = None;
    let shown = fields.shown.unwrap_or_else(|| fields.color());
    egui::Grid::new(("color_panel_numbers", slot))
        .num_columns(2)
        // egui widens a column to the interact width by default; the
        // letters need only their own
        .min_col_width(0.0)
        .spacing(vec2(6.0, 3.0))
        .show(ui, |ui| {
            let label = |ui: &mut Ui, text: &str| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    text,
                    FontId::monospace(12.0),
                    ui.visuals().weak_text_color(),
                );
            };
            let [h, s, v] = fields.hsv;
            let mut hsb = [h * 360.0, s * 100.0, v * 100.0];
            for (index, (name, maximum, unit)) in
                [("H", 360.0, "\u{b0}"), ("S", 100.0, "%"), ("B", 100.0, "%")]
                    .into_iter()
                    .enumerate()
            {
                label(ui, name);
                if ui
                    .add_sized(
                        vec2(62.0, 18.0),
                        egui::DragValue::new(&mut hsb[index])
                            .clamp_range(0.0..=maximum)
                            .speed(if index == 0 { 1.0 } else { 0.5 })
                            .max_decimals(0)
                            .suffix(unit),
                    )
                    .changed()
                {
                    let hue = (hsb[0] / 360.0).min(0.9999);
                    edited = Some(fields.set_hsv([hue, hsb[1] / 100.0, hsb[2] / 100.0]));
                    fields.hsv[0] = hue;
                }
                ui.end_row();
            }
            let mut rgb = [shown.r, shown.g, shown.b];
            for (index, name) in ["R", "G", "B"].into_iter().enumerate() {
                label(ui, name);
                if ui
                    .add_sized(
                        vec2(62.0, 18.0),
                        egui::DragValue::new(&mut rgb[index])
                            .clamp_range(0..=255)
                            .speed(1.0),
                    )
                    .changed()
                {
                    edited = Some(fields.set_rgb(rgb));
                }
                ui.end_row();
            }
            label(ui, "A");
            let mut percent = fields.alpha * 100.0;
            if ui
                .add_sized(
                    vec2(62.0, 18.0),
                    egui::DragValue::new(&mut percent)
                        .clamp_range(0.0..=100.0)
                        .speed(0.5)
                        .max_decimals(0)
                        .suffix("%"),
                )
                .on_hover_text("Opacity")
                .changed()
            {
                edited = Some(fields.set_alpha(percent / 100.0));
            }
            ui.end_row();
        });
    edited
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Anchor, Path as VPath, Transform2D, Tween, Vec2, VectorAsset};

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    #[test]
    fn hsb_fields_read_like_the_numbers_people_know() {
        // Animate shows #E59E13 as H 40°, S 92%, B 90%.
        assert_eq!(hsb_of_rgb([229, 158, 19]), [40, 92, 90]);
        assert_eq!(hex_of_rgb([229, 158, 19]), "E59E13");
        assert_eq!(hsb_of_rgb([255, 0, 0]), [0, 100, 100]);
        assert_eq!(hsb_of_rgb([0, 0, 0]), [0, 0, 0]);
        assert_eq!(hsb_of_rgb([255, 255, 255]), [0, 0, 100]);
        assert_eq!(hsb_of_rgb([0, 0, 255]), [240, 100, 100]);
        // and back, as the fields set it
        let mut fields = Fields::default();
        let mut hsb = |h: f32, s: f32, b: f32| {
            let c = fields.set_hsv([h / 360.0, s / 100.0, b / 100.0]);
            [c.r, c.g, c.b]
        };
        assert_eq!(hsb(0.0, 100.0, 100.0), [255, 0, 0]);
        assert_eq!(hsb(120.0, 100.0, 50.0), [0, 128, 0]);
        assert_eq!(hsb(240.0, 0.0, 100.0), [255, 255, 255]);
        assert_eq!(hsb(360.0, 100.0, 100.0), [255, 0, 0], "360° is red again");
        for rgb in [[229, 158, 19], [12, 200, 180], [128, 128, 128], [1, 2, 3]] {
            let [h, s, v] = face::rgb_to_hsv(rgb.map(|c| f32::from(c) / 255.0));
            assert_eq!(hsb(h * 360.0, s * 100.0, v * 100.0), rgb, "{rgb:?}");
        }
    }

    #[test]
    fn hex_codes_are_read_the_ways_people_type_them() {
        assert_eq!(parse_hex("#E59E13"), Some(([229, 158, 19], None)));
        assert_eq!(parse_hex(" e59e13 "), Some(([229, 158, 19], None)));
        assert_eq!(parse_hex("f80"), Some(([255, 136, 0], None)));
        assert_eq!(parse_hex("E59E1380"), Some(([229, 158, 19], Some(128))));
        assert_eq!(parse_hex("E59E1"), None);
        assert_eq!(parse_hex("GG0000"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn fields_keep_a_greys_hue_and_typed_bytes() {
        let mut fields = Fields::default();
        fields.sync(rgba(200, 40, 40));
        let red = fields.hsv[0];
        fields.sync(rgba(128, 128, 128));
        assert_eq!(fields.hsv[0], red, "a grey keeps the hue it came from");
        assert_eq!(fields.hex, "808080");
        // Saturation up again brings the red back, not some other hue.
        let back = fields.set_hsv([fields.hsv[0], 1.0, 1.0]);
        assert_eq!(back, rgba(255, 0, 0));
        // Typed RGB is kept exactly.
        assert_eq!(fields.set_rgb([1, 2, 3]), rgba(1, 2, 3));
        // Opacity changes only the opacity.
        let half = fields.set_alpha(0.5);
        assert_eq!((half.r, half.g, half.b, half.a), (1, 2, 3, 128));
    }

    #[test]
    fn none_is_refused_for_both_fill_and_stroke() {
        let mut session = Session::for_project(&crate::state::default_project());
        assert!(session.fill_color.is_some(), "shapes start with a fill");
        assert!(!session.stroke_none, "and a stroke");
        assert!(set_tool_paint(&mut session, Slot::Fill, &Paint::None));
        assert_eq!(session.fill_color, None);
        assert_eq!(session.fill_kept, crate::state::DEFAULT_FILL);
        assert!(!none_allowed(&session, Slot::Stroke));
        assert!(!set_tool_paint(&mut session, Slot::Stroke, &Paint::None));
        assert!(!session.stroke_none, "the stroke stays");
        // A colour brings the fill back; then the stroke may go.
        assert!(set_tool_paint(
            &mut session,
            Slot::Fill,
            &Paint::Color(rgba(1, 2, 3))
        ));
        assert_eq!(session.fill_color, Some(rgba(1, 2, 3)));
        assert!(set_tool_paint(&mut session, Slot::Stroke, &Paint::None));
        assert!(session.stroke_none);
        assert!(!set_tool_paint(&mut session, Slot::Fill, &Paint::None));
        // A gradient fill keeps its first colour as the fill's own.
        let gradient = PaintGradient::starting_from(rgba(9, 8, 7), GradientKind::Radial);
        assert!(set_tool_paint(
            &mut session,
            Slot::Fill,
            &Paint::Gradient {
                gradient: gradient.clone(),
                refit: true,
            },
        ));
        assert_eq!(session.fill_color, Some(rgba(9, 8, 7)));
        assert_eq!(session.fill_gradient, Some(gradient));
    }

    fn square(size: f32) -> VPath {
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        VPath {
            anchors: vec![
                corner(0.0, 0.0),
                corner(size, 0.0),
                corner(size, size),
                corner(0.0, size),
            ],
            closed: true,
        }
    }

    /// An editor with one drawing of a filled, outlined square placed as
    /// an object on frame 0 of the first layer and, with `shared`, on frame
    /// 5 as well; the frame-0 one selected.
    fn app_with_object(shared: bool) -> (EditorApp, u16) {
        let mut app = EditorApp::default();
        let asset_id = crate::tools::next_asset_id_pub(&app.state.project);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![square(40.0)],
            fill: Some(rgba(10, 20, 30)),
            stroke: Some(VStroke {
                color: rgba(0, 0, 0),
                width: 2.0,
                cap: q0s_format::geom::CapShape::Round,
                join: q0s_format::geom::JoinShape::Round,
            }),
        }));
        let q0rg_id = app.session.current_q0rg_id;
        let layer_id = app.session.current_layer_id;
        let q0rg = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .unwrap();
        q0rg.frame_count = q0rg.frame_count.max(10);
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        let mut transform = Transform2D::IDENTITY;
        transform.tx = 25.0;
        let object = |frame| Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform,
            tween: Tween::None,
            fx: Default::default(),
        };
        layer.placements.clear();
        layer.placements.push(object(0));
        if shared {
            layer.placements.push(object(5));
        }
        app.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx: 0,
        };
        (app, asset_id)
    }

    fn drawing(app: &EditorApp, placement_idx: usize) -> (u16, q0s_format::v2::VectorAsset) {
        let q0rg = &app.state.project.q0rgs[0];
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == app.session.current_layer_id)
            .unwrap();
        let Target::Asset(asset_id) = layer.placements[placement_idx].target else {
            panic!("not a drawing");
        };
        let Some(Asset::Vector(vector)) = asset(&app.state.project, asset_id) else {
            panic!("not a vector");
        };
        (asset_id, vector.clone())
    }

    #[test]
    fn a_colour_repaints_the_selected_object_and_the_tools() {
        let (mut app, asset_id) = app_with_object(false);
        assert_eq!(
            look(&app, Slot::Fill).color,
            rgba(10, 20, 30),
            "shows the selection"
        );
        let mut gesture = ColorGesture::default();
        gesture.arm();
        assert!(apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(200, 0, 0)),
            &mut gesture
        ));
        let (id, vector) = drawing(&app, 0);
        assert_eq!(
            id, asset_id,
            "nothing else uses it, so it is painted in place"
        );
        assert_eq!(vector.fill, Some(rgba(200, 0, 0)));
        assert_eq!(
            app.session.fill_color,
            Some(rgba(200, 0, 0)),
            "the tools follow"
        );
        assert!(apply_with(
            &mut app,
            Slot::Stroke,
            &Paint::Color(rgba(0, 0, 200)),
            &mut gesture
        ));
        assert_eq!(drawing(&app, 0).1.stroke.unwrap().color, rgba(0, 0, 200));
        assert_eq!(
            drawing(&app, 0).1.stroke.unwrap().width,
            2.0,
            "only the colour changes"
        );
        // None takes the stroke off while the fill stays...
        assert!(apply_with(
            &mut app,
            Slot::Stroke,
            &Paint::None,
            &mut gesture
        ));
        assert_eq!(drawing(&app, 0).1.stroke, None);
        // ...and then the fill may not go too.
        assert!(!apply_with(
            &mut app,
            Slot::Fill,
            &Paint::None,
            &mut gesture
        ));
        assert!(drawing(&app, 0).1.fill.is_some());
        // A stroke colour gives the drawing a stroke again, at the tools' width.
        app.session.stroke_width = 3.0;
        assert!(apply_with(
            &mut app,
            Slot::Stroke,
            &Paint::Color(rgba(5, 5, 5)),
            &mut gesture
        ));
        let stroke = drawing(&app, 0).1.stroke.unwrap();
        assert_eq!((stroke.color, stroke.width), (rgba(5, 5, 5), 3.0));
    }

    #[test]
    fn a_shared_drawing_is_copied_so_only_the_selected_one_changes() {
        let (mut app, asset_id) = app_with_object(true);
        let mut gesture = ColorGesture::default();
        gesture.arm();
        assert!(apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(0, 200, 0)),
            &mut gesture
        ));
        let (selected_id, selected) = drawing(&app, 0);
        let (other_id, other) = drawing(&app, 1);
        assert_ne!(selected_id, asset_id);
        assert_eq!(other_id, asset_id, "the other keyframe keeps the original");
        assert_eq!(selected.fill, Some(rgba(0, 200, 0)));
        assert_eq!(other.fill, Some(rgba(10, 20, 30)));
        assert!(matches!(
            app.session.selection,
            Selection::Placement {
                placement_idx: 0,
                ..
            }
        ));
    }

    #[test]
    fn a_drag_of_many_colours_is_one_undo_step() {
        let (mut app, _) = app_with_object(false);
        let mut gesture = ColorGesture::default();
        let before = app.state.project.clone();
        gesture.arm();
        for step in 0..20u8 {
            apply_with(
                &mut app,
                Slot::Fill,
                &Paint::Color(rgba(step * 10, 0, 0)),
                &mut gesture,
            );
        }
        assert_eq!(drawing(&app, 0).1.fill, Some(rgba(190, 0, 0)));
        let restored = app
            .history
            .pop_undo(app.state.project.clone(), |_, _| {})
            .expect("one step to undo");
        assert_eq!(restored, before, "one undo goes back to before the drag");
        assert!(!app.history.can_undo(), "and there was only that one");

        // A new press starts a new step; a colour that changes nothing takes none.
        let (mut app, _) = app_with_object(false);
        let mut gesture = ColorGesture::default();
        gesture.arm();
        apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(1, 1, 1)),
            &mut gesture,
        );
        gesture.arm();
        apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(2, 2, 2)),
            &mut gesture,
        );
        gesture.arm();
        assert!(!apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(2, 2, 2)),
            &mut gesture
        ));
        let mut steps = 0;
        let mut project = app.state.project.clone();
        while let Ok(previous) = app.history.pop_undo(project.clone(), |_, _| {}) {
            project = previous;
            steps += 1;
        }
        assert_eq!(steps, 2);
    }

    #[test]
    fn nothing_selected_changes_only_the_tools() {
        let mut app = EditorApp::default();
        let mut gesture = ColorGesture::default();
        gesture.arm();
        assert!(!apply_with(
            &mut app,
            Slot::Stroke,
            &Paint::Color(rgba(1, 2, 3)),
            &mut gesture
        ));
        assert_eq!(app.session.stroke_color, rgba(1, 2, 3));
        assert!(!app.history.can_undo(), "a tool colour is not an undo step");
    }

    #[test]
    fn a_gradient_fill_fits_the_selected_drawing() {
        let (mut app, _) = app_with_object(false);
        let gradient = PaintGradient::starting_from(rgba(50, 60, 70), GradientKind::Linear);
        let mut gesture = ColorGesture::default();
        gesture.arm();
        assert!(apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Gradient {
                gradient: gradient.clone(),
                refit: true,
            },
            &mut gesture,
        ));
        let (asset_id, vector) = drawing(&app, 0);
        let stored = app.state.project.asset_appearances[&asset_id]
            .fill_gradient
            .clone()
            .expect("a gradient");
        assert_eq!(stored.stops, gradient.stops);
        assert_eq!(
            vector.fill,
            Some(rgba(50, 60, 70)),
            "the fill follows the first stop"
        );
        assert_eq!(
            look(&app, Slot::Fill).gradient.map(|g| g.kind),
            Some(GradientKind::Linear)
        );
        // Solid again takes the gradient off, and the appearance with it.
        assert!(apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Color(rgba(1, 1, 1)),
            &mut gesture
        ));
        assert!(!app.state.project.asset_appearances.contains_key(&asset_id));
    }

    fn painted_text(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(label) => {
                text.push_str(&label.galley.job.text);
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    painted_text(shape, text);
                }
            }
            _ => {}
        }
    }

    /// A few frames of the Color panel and the tool column, as the editor
    /// shows them; the text they paint.
    fn show(app: &mut EditorApp, context: &egui::Context, width: f32) -> String {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(width + 60.0, 900.0),
            )),
            ..Default::default()
        };
        let mut text = String::new();
        for _ in 0..3 {
            let output = context.run(input.clone(), |context| {
                egui::SidePanel::left("tools")
                    .exact_width(52.0)
                    .show(context, |ui| crate::panels::toolbar::render(app, ui));
                egui::CentralPanel::default().show(context, |ui| render(app, ui));
            });
            text.clear();
            for clipped in &output.shapes {
                painted_text(&clipped.shape, &mut text);
            }
        }
        text
    }

    /// Everything the panel paints, `width` points wide, lies inside it.
    fn assert_fits(app: &mut EditorApp, context: &egui::Context, width: f32) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 1400.0))),
            ..Default::default()
        };
        for _ in 0..3 {
            let output = context.run(input.clone(), |context| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(context, |ui| render(app, ui));
            });
            for clipped in &output.shapes {
                let bounds = clipped.shape.visual_bounding_rect();
                if !bounds.is_finite() || bounds.width() < 0.0 {
                    continue;
                }
                assert!(
                    bounds.min.x >= -0.5 && bounds.max.x <= width + 0.5,
                    "at {width} wide something reaches {:?}: {:?}",
                    bounds.x_range(),
                    clipped.shape
                );
            }
        }
    }

    #[test]
    fn the_panel_fits_the_width_it_is_given() {
        let context = egui::Context::default();
        let (mut app, _) = app_with_object(false);
        for width in [170.0, 200.0, 229.0, 230.0, 260.0, 320.0, 480.0] {
            assert_fits(&mut app, &context, width);
        }
        // with the hue wheel instead of the square
        face::set_prefers_wheel(true);
        for width in [170.0, 229.0, 230.0, 320.0, 480.0] {
            assert_fits(&mut app, &context, width);
        }
        face::set_prefers_wheel(false);
        // with a gradient's stop bar too
        let mut gesture = ColorGesture::default();
        apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Gradient {
                gradient: PaintGradient::starting_from(rgba(200, 30, 30), GradientKind::Linear),
                refit: true,
            },
            &mut gesture,
        );
        for width in [170.0, 230.0, 249.0, 250.0, 320.0] {
            assert_fits(&mut app, &context, width);
        }
    }

    #[test]
    fn the_panel_and_the_swatches_draw_at_any_width_and_with_any_paint() {
        let context = egui::Context::default();
        let (mut app, _) = app_with_object(false);
        for width in [180.0, 320.0] {
            let text = show(&mut app, &context, width);
            for label in [
                "Solid",
                "Linear",
                "Radial",
                "None",
                "Swatches",
                "Add to swatches",
            ] {
                assert!(text.contains(label), "{label} at {width}: {text}");
            }
            assert!(text.contains("Fill: the selection's paint"));
        }
        // A gradient brings its stop bar; none and the stroke slot draw too.
        let gradient = PaintGradient::starting_from(rgba(50, 60, 70), GradientKind::Radial);
        let mut gesture = ColorGesture::default();
        apply_with(
            &mut app,
            Slot::Fill,
            &Paint::Gradient {
                gradient,
                refit: true,
            },
            &mut gesture,
        );
        assert!(show(&mut app, &context, 320.0).contains("Stop"));
        app.session.selection = Selection::None;
        set_active_slot(&context, Slot::Stroke);
        apply_with(&mut app, Slot::Stroke, &Paint::None, &mut gesture);
        let text = show(&mut app, &context, 320.0);
        assert!(text.contains("Stroke: the paint new shapes"));
    }
}
