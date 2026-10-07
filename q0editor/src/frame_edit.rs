//! Editor-only range selection. Every reference retains its own frame, so
//! editing a range never materializes all its objects onto the playhead.
use std::collections::{BTreeMap, BTreeSet};

use egui::{pos2, vec2, Context, Painter, Rect, Response, Stroke};
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, Placement, ProjectV2, Target, Transform2D, Vec2, VectorAsset};

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{
    GroupTransformOperation, Handle, PlacementRef, Selection, Session, TimelineSelection, Tool,
    ToolState, TransformPivot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameRef {
    pub reference: PlacementRef,
    pub frame: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct Pose {
    reference: FrameRef,
    transform: Transform2D,
    raw_asset: Option<u16>,
}

#[derive(Debug, Clone)]
pub(crate) struct RawSnapshot {
    asset: VectorAsset,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance: Option<q0s_format::v2::VectorAppearance>,
}

#[derive(Debug, Clone)]
pub(crate) enum Gesture {
    Pivot,
    Marquee {
        start: Vec2,
        cursor: Vec2,
        base: Vec<FrameRef>,
    },
    Transform {
        operation: GroupTransformOperation,
        pivot: Vec2,
        poses: Vec<Pose>,
        raw: Vec<RawSnapshot>,
    },
}

pub(crate) fn enable(app: &mut EditorApp) {
    if !crate::text_tool::finish_edit(app) {
        return;
    }
    app.session.onion.enabled = false;
    app.session.onion.edit_frames = true;
    app.session.playing = false;
    app.session.current_tool = Tool::Select;
    app.session.tool_state = ToolState::Idle;
    app.session.frame_edit_gesture = None;
    let _ = app.refresh_audio_playback();
    if let Some(selection) = app.session.timeline_selection {
        let first = selection.anchor_frame.min(selection.focus_frame);
        let last = selection.anchor_frame.max(selection.focus_frame);
        let frame = app.session.timeline_frame();
        app.session.onion.before = frame.saturating_sub(first);
        app.session.onion.after = last.saturating_sub(frame);
    }
    if let Some((q, cells)) = &app.session.timeline_cell_selection {
        if *q == app.session.current_q0rg_id {
            if let (Some(first), Some(last)) = (
                cells.iter().map(|(_, f)| *f).min(),
                cells.iter().map(|(_, f)| *f).max(),
            ) {
                app.session.onion.before = app.session.current_frame.saturating_sub(first);
                app.session.onion.after = last.saturating_sub(app.session.current_frame);
                if let Some(q0rg) = app
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == *q)
                {
                    let layers: Vec<_> = q0rg
                        .layers
                        .iter()
                        .filter(|l| cells.iter().any(|(id, _)| *id == l.layer_id))
                        .collect();
                    if let (Some(a), Some(b)) = (layers.first(), layers.last()) {
                        app.session.timeline_selection = Some(TimelineSelection {
                            anchor_layer_id: a.layer_id,
                            anchor_frame: first,
                            focus_layer_id: b.layer_id,
                            focus_frame: last,
                        });
                    }
                }
            }
        }
    }
    let selected =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection);
    app.session.frame_edit_selection = candidates(app)
        .into_iter()
        .filter(|r| selected.contains(&r.reference))
        .collect();
    sync_selection(app);
}

pub(crate) fn disable(app: &mut EditorApp) {
    app.session.onion.edit_frames = false;
    app.session.frame_edit_gesture = None;
    app.session.frame_edit_selection.clear();
    app.session
        .clear_inactive_frame_selection(&app.state.project);
}

pub(crate) fn select_all(app: &mut EditorApp) {
    app.session.frame_edit_selection = candidates(app);
    sync_selection(app);
}

fn samples(project: &ProjectV2, session: &Session) -> BTreeMap<u16, Vec<u16>> {
    let mut result = BTreeMap::new();
    let Some(q) = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == session.current_q0rg_id)
    else {
        return result;
    };
    let frame = session.current_frame;
    let first = frame.saturating_sub(session.onion.before);
    let last = frame
        .saturating_add(session.onion.after)
        .min(q.frame_count.saturating_sub(1));
    let sparse = session
        .timeline_cell_selection
        .as_ref()
        .filter(|(id, cells)| *id == q.q0rg_id && !cells.is_empty());
    let selected_rows = session
        .timeline_selection
        .filter(|_| sparse.is_none() && !session.onion.all_unlocked_layers)
        .and_then(|s| {
            let a = q
                .layers
                .iter()
                .position(|l| l.layer_id == s.anchor_layer_id)?;
            let b = q
                .layers
                .iter()
                .position(|l| l.layer_id == s.focus_layer_id)?;
            Some(a.min(b)..=a.max(b))
        });
    for (index, layer) in q.layers.iter().enumerate() {
        if project.layer_is_folder(q.q0rg_id, layer.layer_id)
            || project.layer_is_locked(q.q0rg_id, layer.layer_id)
            || !project.layer_is_visible(q.q0rg_id, layer.layer_id)
            || selected_rows
                .as_ref()
                .is_some_and(|rows| !rows.contains(&index))
            || (sparse.is_none()
                && selected_rows.is_none()
                && !session.onion.all_unlocked_layers
                && layer.layer_id != session.current_layer_id)
        {
            continue;
        }
        if let Some((selected_q, cells)) = &session.timeline_cell_selection {
            if *selected_q == q.q0rg_id && !cells.is_empty() {
                let keys: BTreeSet<_> = cells
                    .iter()
                    .filter(|(l, _)| session.onion.all_unlocked_layers || *l == layer.layer_id)
                    .map(|(_, f)| *f)
                    .filter(|f| *f >= first && *f <= last)
                    .collect();
                if !keys.is_empty() {
                    result.insert(layer.layer_id, keys.into_iter().collect());
                }
                continue;
            }
        }
        let mut keys: Vec<_> = layer
            .keyframe_frames()
            .into_iter()
            .filter(|f| *f >= first && *f <= last)
            .collect();
        // A held cell at the left boundary is shown once. On first edit it is
        // baked there, preserving the key before the chosen range.
        if !keys.contains(&first) {
            keys.insert(0, first);
        }
        result.insert(layer.layer_id, keys);
    }
    result
}

pub(crate) fn preview_layers(app: &EditorApp) -> BTreeMap<u16, BTreeSet<u16>> {
    let mut result: BTreeMap<u16, BTreeSet<u16>> = BTreeMap::new();
    let Some(q) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == app.session.current_q0rg_id)
    else {
        return result;
    };
    for (layer_id, frames) in samples(&app.state.project, &app.session) {
        let Some(layer) = q.layers.iter().find(|l| l.layer_id == layer_id) else {
            continue;
        };
        let current =
            q0s_format::raster::active_placement_states_at(layer, app.session.current_frame);
        for sample in frames {
            let active = q0s_format::raster::active_placement_states_at(layer, sample);
            if !active.is_empty()
                && !active.iter().all(|a| {
                    current
                        .iter()
                        .any(|b| a.index == b.index && a.transform == b.transform)
                })
            {
                result.entry(sample).or_default().insert(layer_id);
            }
        }
    }
    result
}

fn candidates_for(project: &ProjectV2, session: &Session) -> Vec<FrameRef> {
    let mut result = Vec::new();
    let Some(q) = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == session.current_q0rg_id)
    else {
        return result;
    };
    for (layer_id, frames) in samples(project, session) {
        let Some(layer) = q.layers.iter().find(|l| l.layer_id == layer_id) else {
            continue;
        };
        for frame in frames {
            for active in q0s_format::raster::active_placement_states_at(layer, frame) {
                let reference = PlacementRef {
                    q0rg_id: q.q0rg_id,
                    layer_id,
                    placement_idx: active.index,
                };
                if !result.iter().any(|r: &FrameRef| r.reference == reference) {
                    result.push(FrameRef { reference, frame });
                }
            }
        }
    }
    result
}

fn candidates(app: &EditorApp) -> Vec<FrameRef> {
    candidates_for(&app.state.project, &app.session)
}

pub(crate) fn refresh_session(session: &mut Session, project: &ProjectV2) {
    if session.frame_edit_gesture.is_some() {
        return;
    }
    let refs = crate::selection_edit::selected_object_refs(project, &session.selection);
    session.frame_edit_selection = candidates_for(project, session)
        .into_iter()
        .filter(|r| refs.contains(&r.reference))
        .collect();
    if !refs.is_empty() {
        let selected: Vec<_> = session
            .frame_edit_selection
            .iter()
            .map(|r| r.reference)
            .collect();
        session.selection = match selected.as_slice() {
            [] => Selection::None,
            [r] => Selection::Placement {
                q0rg_id: r.q0rg_id,
                layer_id: r.layer_id,
                placement_idx: r.placement_idx,
            },
            _ => Selection::Multi(selected),
        };
    }
}

fn placement(app: &EditorApp, reference: FrameRef) -> Option<Placement> {
    let r = reference.reference;
    let layer = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == r.q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == r.layer_id)?;
    let mut p = layer.placements.get(r.placement_idx)?.clone();
    if let Some((transform, _)) = crate::tools::placement_transform_for_edit(
        &app.state.project,
        r.q0rg_id,
        r.layer_id,
        r.placement_idx,
        reference.frame,
    ) {
        p.transform = transform;
    }
    if let Some(active) = q0s_format::raster::active_placement_states_at(layer, reference.frame)
        .into_iter()
        .find(|a| a.index == r.placement_idx)
    {
        p.fx = active.fx;
    }
    Some(p)
}

fn bounds(app: &EditorApp, r: FrameRef) -> Option<(f32, f32, f32, f32)> {
    crate::render::placement_bbox(&app.state.project, &placement(app, r)?)
}

fn selected_bounds(app: &EditorApp) -> Option<(f32, f32, f32, f32)> {
    app.session
        .frame_edit_selection
        .iter()
        .filter_map(|r| bounds(app, *r))
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

fn sync_selection(app: &mut EditorApp) {
    let refs: Vec<_> = app
        .session
        .frame_edit_selection
        .iter()
        .map(|r| r.reference)
        .collect();
    app.session.selection = match refs.as_slice() {
        [] => Selection::None,
        [r] => Selection::Placement {
            q0rg_id: r.q0rg_id,
            layer_id: r.layer_id,
            placement_idx: r.placement_idx,
        },
        _ => Selection::Multi(refs),
    };
}

pub(crate) fn frame_for(app: &EditorApp, reference: PlacementRef) -> u16 {
    if app.session.onion.edit_frames {
        app.session
            .frame_edit_selection
            .iter()
            .find(|r| r.reference == reference)
            .map(|r| r.frame)
            .unwrap_or_else(|| {
                app.state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == reference.q0rg_id)
                    .and_then(|q| q.layers.iter().find(|l| l.layer_id == reference.layer_id))
                    .and_then(|l| l.placements.get(reference.placement_idx))
                    .map(|p| {
                        p.frame.max(
                            app.session
                                .current_frame
                                .saturating_sub(app.session.onion.before),
                        )
                    })
                    .unwrap_or(app.session.current_frame)
            })
    } else {
        app.session.current_frame
    }
}

pub(crate) fn set_transform_at_frame(
    app: &mut EditorApp,
    reference: PlacementRef,
    frame: u16,
    transform: Transform2D,
) -> bool {
    let saved = app.session.current_frame;
    app.session.current_frame = frame;
    let changed = crate::tools::set_placement_transform_for_edit(
        app,
        reference.q0rg_id,
        reference.layer_id,
        reference.placement_idx,
        transform,
    );
    app.session.current_frame = saved;
    changed
}

pub(crate) fn remap(app: &mut EditorApp, original: &[PlacementRef], mapped: &[PlacementRef]) {
    for r in &mut app.session.frame_edit_selection {
        if let Some(index) = original.iter().position(|old| *old == r.reference) {
            r.reference = mapped[index];
        }
    }
    sync_selection(app);
}

pub(crate) fn materialize_refs(
    project: &mut ProjectV2,
    refs: &[PlacementRef],
    frames: &[u16],
) -> Option<Vec<PlacementRef>> {
    let mut mappings = BTreeMap::new();
    for (r, frame) in refs.iter().zip(frames) {
        let key = (r.q0rg_id, r.layer_id, *frame);
        if let std::collections::btree_map::Entry::Vacant(e) = mappings.entry(key) {
            e.insert(crate::tools::materialize_layer_keyframe_for_edit(
                project, key.0, key.1, key.2,
            )?);
        }
    }
    refs.iter()
        .zip(frames)
        .map(|(r, frame)| {
            Some(PlacementRef {
                placement_idx: *mappings
                    .get(&(r.q0rg_id, r.layer_id, *frame))?
                    .get(&r.placement_idx)?,
                ..*r
            })
        })
        .collect()
}

fn pick(app: &EditorApp, p: Vec2, scale: f32) -> Option<FrameRef> {
    let mut refs = candidates(app);
    refs.sort_by_key(|r| {
        (
            r.frame == app.session.current_frame,
            r.frame,
            app.state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == r.reference.q0rg_id)
                .and_then(|q| {
                    q.layers
                        .iter()
                        .position(|l| l.layer_id == r.reference.layer_id)
                })
                .unwrap_or(0),
            r.reference.placement_idx,
        )
    });
    refs.into_iter().rev().find(|r| {
        placement(app, *r).is_some_and(|placement| {
            let playback_start_frame = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == r.reference.q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.reference.layer_id))
                .and_then(|layer| {
                    q0s_format::v2::placement_playback_start_frames(layer)
                        .get(r.reference.placement_idx)
                        .copied()
                })
                .unwrap_or(placement.frame);
            crate::tools::frame_range_placement_hit(
                &app.state.project,
                &placement,
                playback_start_frame,
                r.frame,
                p,
                scale,
            )
        })
    })
}

fn handles(b: (f32, f32, f32, f32)) -> [(Handle, Vec2); 8] {
    let (x, y) = ((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5);
    [
        (Handle::TopLeft, Vec2::new(b.0, b.1)),
        (Handle::TopRight, Vec2::new(b.2, b.1)),
        (Handle::BottomRight, Vec2::new(b.2, b.3)),
        (Handle::BottomLeft, Vec2::new(b.0, b.3)),
        (Handle::MidTop, Vec2::new(x, b.1)),
        (Handle::MidRight, Vec2::new(b.2, y)),
        (Handle::MidBottom, Vec2::new(x, b.3)),
        (Handle::MidLeft, Vec2::new(b.0, y)),
    ]
}

fn prepare(app: &mut EditorApp, operation: GroupTransformOperation, pivot: Vec2) -> Gesture {
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let mut mappings = BTreeMap::new();
    for r in &app.session.frame_edit_selection {
        let key = (r.reference.q0rg_id, r.reference.layer_id, r.frame);
        mappings.entry(key).or_insert_with(|| {
            crate::tools::materialize_layer_keyframe_for_edit(
                &mut app.state.project,
                key.0,
                key.1,
                key.2,
            )
        });
    }
    for r in &mut app.session.frame_edit_selection {
        if let Some(Some(map)) = mappings.get(&(r.reference.q0rg_id, r.reference.layer_id, r.frame))
        {
            if let Some(index) = map.get(&r.reference.placement_idx) {
                r.reference.placement_idx = *index;
            }
        }
    }
    sync_selection(app);
    let mut raw_groups: BTreeMap<(u16, u16, u16), BTreeSet<u16>> = BTreeMap::new();
    for r in &app.session.frame_edit_selection {
        if let Some(p) = placement(app, *r) {
            if crate::tools::is_raw_graphics_placement(&app.state.project, &p) {
                if let Target::Asset(id) = p.target {
                    if app
                        .state
                        .project
                        .assets
                        .iter()
                        .any(|a| a.id() == id && matches!(a, Asset::Vector(_)))
                    {
                        raw_groups
                            .entry((r.reference.q0rg_id, r.reference.layer_id, r.frame))
                            .or_default()
                            .insert(id);
                    }
                }
            }
        }
    }
    let mut raw = Vec::new();
    for ((q, l, f), ids) in raw_groups {
        for id in crate::brush::prepare_writable_raw_assets(&mut app.state.project, q, l, f, &ids)
            .into_values()
        {
            if raw.iter().any(|s: &RawSnapshot| s.asset.asset_id == id) {
                continue;
            }
            if let Some(Asset::Vector(asset)) =
                app.state.project.assets.iter().find(|a| a.id() == id)
            {
                raw.push(RawSnapshot {
                    asset: asset.clone(),
                    #[cfg(feature = "appearance-mask-eraser")]
                    appearance: app.state.project.asset_appearances.get(&id).cloned(),
                });
            }
        }
    }
    let poses = app
        .session
        .frame_edit_selection
        .iter()
        .filter_map(|r| {
            let p = placement(app, *r)?;
            let raw_asset = match p.target {
                Target::Asset(id) if raw.iter().any(|s| s.asset.asset_id == id) => Some(id),
                _ => None,
            };
            Some(Pose {
                reference: *r,
                transform: p.transform,
                raw_asset,
            })
        })
        .collect();
    Gesture::Transform {
        operation,
        pivot,
        poses,
        raw,
    }
}

fn apply(app: &mut EditorApp, poses: &[Pose], raw: &[RawSnapshot], affine: Affine, pivot: Vec2) {
    let saved_frame = app.session.current_frame;
    for pose in poses.iter().filter(|p| p.raw_asset.is_none()) {
        let r = pose.reference.reference;
        if let Some(transform) = crate::app::affine_to_transform(Affine::compose(
            affine,
            Affine::from_transform(pose.transform),
        )) {
            app.session.current_frame = pose.reference.frame;
            crate::tools::set_placement_transform_for_edit(
                app,
                r.q0rg_id,
                r.layer_id,
                r.placement_idx,
                transform,
            );
        }
    }
    app.session.current_frame = saved_frame;
    for source in raw {
        let mut transformed = source.asset.clone();
        for path in &mut transformed.paths {
            for anchor in &mut path.anchors {
                anchor.point = affine.apply(anchor.point);
                anchor.in_handle = anchor.in_handle.map(|p| affine.apply(p));
                anchor.out_handle = anchor.out_handle.map(|p| affine.apply(p));
            }
        }
        if let Some(asset) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|a| a.id() == source.asset.asset_id)
        {
            *asset = Asset::Vector(transformed);
        }
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = &source.appearance {
            app.state.project.asset_appearances.insert(
                source.asset.asset_id,
                crate::appearance::transform_appearance(appearance, affine),
            );
        }
    }
    app.session.transform_pivot = Some(TransformPivot {
        selection: app.session.selection.clone(),
        point: affine.apply(pivot),
    });
    app.state.mark_dirty();
    app.textures.invalidate();
}

pub(crate) fn handle(
    app: &mut EditorApp,
    response: &Response,
    view: &StageView,
    ctx: &Context,
) -> bool {
    if !app.session.onion.edit_frames
        || !matches!(
            app.session.current_tool,
            Tool::Select | Tool::Subselect | Tool::Lasso
        )
    {
        return false;
    }
    refresh_session(&mut app.session, &app.state.project);
    if app.session.playing && (response.clicked() || response.drag_started()) {
        app.session.playing = false;
        app.stop_audio_playback();
    }
    let shift = ctx.input(|i| i.modifiers.shift);
    if response.secondary_clicked() {
        if let Some(hit) = response
            .interact_pointer_pos()
            .and_then(|p| pick(app, view.screen_to_stage(p), view.effective_scale()))
        {
            if !app.session.frame_edit_selection.contains(&hit) {
                app.session.frame_edit_selection = vec![hit];
                sync_selection(app);
            }
        }
    }
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let hit = pick(app, view.screen_to_stage(pointer), view.effective_scale());
            if !shift {
                app.session.frame_edit_selection.clear();
            }
            if let Some(hit) = hit {
                if shift && app.session.frame_edit_selection.contains(&hit) {
                    app.session.frame_edit_selection.retain(|r| *r != hit);
                } else {
                    app.session.frame_edit_selection.push(hit);
                }
            }
            sync_selection(app);
        }
    }
    if response.drag_started() {
        if let Some(pointer) = ctx.input(|i| i.pointer.press_origin()) {
            let start = view.screen_to_stage(pointer);
            let mut operation = None;
            let mut pivot_drag = false;
            if let Some(b) = selected_bounds(app) {
                let center = app
                    .session
                    .transform_pivot
                    .as_ref()
                    .filter(|p| p.selection == app.session.selection)
                    .map(|p| p.point)
                    .unwrap_or(Vec2::new((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5));
                pivot_drag = ctx.input(|i| i.modifiers.alt)
                    && view.stage_to_screen(center).distance(pointer) <= 7.0;
                let rotate =
                    view.stage_to_screen(Vec2::new((b.0 + b.2) * 0.5, b.1)) - vec2(0.0, 24.0);
                if rotate.distance(pointer) <= 12.0 {
                    operation = Some(GroupTransformOperation::Rotate {
                        center,
                        start_angle: (start.y - center.y).atan2(start.x - center.x),
                    });
                } else if let Some((handle, _)) = handles(b).into_iter().find(|(_, p)| {
                    let distance = view.stage_to_screen(*p).distance(pointer);
                    distance <= 12.0
                        && (distance <= 5.0
                            || !view
                                .stage_bounds_to_screen(Vec2::new(b.0, b.1), Vec2::new(b.2, b.3))
                                .contains(pointer))
                }) {
                    operation = Some(GroupTransformOperation::Scale {
                        handle,
                        start_bounds: b,
                    });
                }
            }
            if operation.is_none() && !pivot_drag {
                if let Some(hit) = pick(app, start, view.effective_scale()) {
                    if !app.session.frame_edit_selection.contains(&hit) {
                        if !shift {
                            app.session.frame_edit_selection.clear();
                        }
                        app.session.frame_edit_selection.push(hit);
                        sync_selection(app);
                    }
                    operation = Some(GroupTransformOperation::Move {
                        start_cursor: start,
                    });
                }
            }
            app.session.frame_edit_gesture = Some(if pivot_drag {
                Gesture::Pivot
            } else if let Some(operation) = operation {
                let b = selected_bounds(app).unwrap_or((start.x, start.y, start.x, start.y));
                let pivot = app
                    .session
                    .transform_pivot
                    .as_ref()
                    .filter(|p| p.selection == app.session.selection)
                    .map(|p| p.point)
                    .unwrap_or(Vec2::new((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5));
                prepare(app, operation, pivot)
            } else {
                Gesture::Marquee {
                    start,
                    cursor: start,
                    base: if shift {
                        app.session.frame_edit_selection.clone()
                    } else {
                        Vec::new()
                    },
                }
            });
        }
    }
    if response.dragged() || response.drag_stopped() {
        if let (Some(mut gesture), Some(pointer)) = (
            app.session.frame_edit_gesture.take(),
            response
                .interact_pointer_pos()
                .or_else(|| ctx.pointer_interact_pos()),
        ) {
            let cursor = view.screen_to_stage(pointer);
            match &mut gesture {
                Gesture::Pivot => {
                    app.session.transform_pivot = Some(TransformPivot {
                        selection: app.session.selection.clone(),
                        point: cursor,
                    });
                }
                Gesture::Marquee {
                    start,
                    cursor: last,
                    base,
                } => {
                    *last = cursor;
                    let rect = Rect::from_two_pos(pos2(start.x, start.y), pos2(cursor.x, cursor.y));
                    let mut selected = base.clone();
                    for r in candidates(app) {
                        if let Some(b) = bounds(app, r) {
                            if rect.intersects(Rect::from_min_max(pos2(b.0, b.1), pos2(b.2, b.3)))
                                && !selected.contains(&r)
                            {
                                selected.push(r);
                            }
                        }
                    }
                    app.session.frame_edit_selection = selected;
                    sync_selection(app);
                }
                Gesture::Transform {
                    operation,
                    pivot,
                    poses,
                    raw,
                } => {
                    if let Some(affine) = crate::tools::group_transform_affine(
                        *operation,
                        *pivot,
                        cursor,
                        shift,
                        ctx.input(|i| i.modifiers.ctrl),
                    ) {
                        apply(app, poses, raw, affine, *pivot);
                    }
                }
            }
            if !response.drag_stopped() {
                app.session.frame_edit_gesture = Some(gesture);
            }
        }
    }
    true
}

pub(crate) fn draw_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    let color = app.settings.theme.accent.to_color32();
    for r in &app.session.frame_edit_selection {
        if let Some(b) = bounds(app, *r) {
            painter.rect_stroke(
                view.stage_bounds_to_screen(Vec2::new(b.0, b.1), Vec2::new(b.2, b.3)),
                0.0,
                Stroke::new(1.0_f32, color.gamma_multiply(0.5)),
            );
        }
    }
    if let Some(b) = selected_bounds(app) {
        painter.rect_stroke(
            view.stage_bounds_to_screen(Vec2::new(b.0, b.1), Vec2::new(b.2, b.3)),
            0.0,
            Stroke::new(1.3_f32, color),
        );
        for (_, p) in handles(b) {
            painter.rect_filled(
                Rect::from_center_size(view.stage_to_screen(p), vec2(6.0, 6.0)),
                1.0,
                color,
            );
        }
        let top = view.stage_to_screen(Vec2::new((b.0 + b.2) * 0.5, b.1));
        painter.line_segment([top, top - vec2(0.0, 24.0)], Stroke::new(1.0_f32, color));
        painter.circle_stroke(top - vec2(0.0, 24.0), 4.0, Stroke::new(1.5_f32, color));
        let pivot = app
            .session
            .transform_pivot
            .as_ref()
            .filter(|p| p.selection == app.session.selection)
            .map(|p| p.point)
            .unwrap_or(Vec2::new((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5));
        let p = view.stage_to_screen(pivot);
        painter.circle_stroke(p, 3.5, Stroke::new(1.3_f32, color));
        painter.line_segment(
            [p - vec2(6.0, 0.0), p + vec2(6.0, 0.0)],
            Stroke::new(1.0_f32, color),
        );
        painter.line_segment(
            [p - vec2(0.0, 6.0), p + vec2(0.0, 6.0)],
            Stroke::new(1.0_f32, color),
        );
    }
    if let Some(Gesture::Marquee { start, cursor, .. }) = &app.session.frame_edit_gesture {
        let rect = Rect::from_two_pos(view.stage_to_screen(*start), view.stage_to_screen(*cursor));
        painter.rect_filled(rect, 0.0, color.gamma_multiply(0.08));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, color));
    }
}

pub(crate) fn nudge(app: &mut EditorApp, delta: Vec2) -> bool {
    refresh_session(&mut app.session, &app.state.project);
    let Some(b) = selected_bounds(app) else {
        return false;
    };
    let pivot = app
        .session
        .transform_pivot
        .as_ref()
        .filter(|p| p.selection == app.session.selection)
        .map(|p| p.point)
        .unwrap_or(Vec2::new((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5));
    if let Gesture::Transform { poses, raw, .. } = prepare(
        app,
        GroupTransformOperation::Move {
            start_cursor: Vec2::new(0.0, 0.0),
        },
        pivot,
    ) {
        apply(
            app,
            &poses,
            &raw,
            Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            },
            pivot,
        );
    }
    true
}

pub(crate) fn flip(app: &mut EditorApp, horizontal: bool) -> bool {
    refresh_session(&mut app.session, &app.state.project);
    let Some(b) = selected_bounds(app) else {
        return false;
    };
    let pivot = app
        .session
        .transform_pivot
        .as_ref()
        .filter(|p| p.selection == app.session.selection)
        .map(|p| p.point)
        .unwrap_or(Vec2::new((b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5));
    if let Gesture::Transform { poses, raw, .. } = prepare(
        app,
        GroupTransformOperation::Move {
            start_cursor: Vec2::new(0.0, 0.0),
        },
        pivot,
    ) {
        let affine = if horizontal {
            Affine {
                a11: -1.0,
                tx: 2.0 * pivot.x,
                ..Affine::IDENTITY
            }
        } else {
            Affine {
                a22: -1.0,
                ty: 2.0 * pivot.y,
                ..Affine::IDENTITY
            }
        };
        apply(app, &poses, &raw, affine, pivot);
    }
    true
}

pub(crate) fn delete(app: &mut EditorApp) -> bool {
    refresh_session(&mut app.session, &app.state.project);
    if app.session.frame_edit_selection.is_empty() {
        return false;
    }
    let gesture = prepare(
        app,
        GroupTransformOperation::Move {
            start_cursor: Vec2::new(0.0, 0.0),
        },
        Vec2::new(0.0, 0.0),
    );
    if let Gesture::Transform { poses, .. } = gesture {
        let mut refs: Vec<_> = poses.into_iter().map(|p| p.reference.reference).collect();
        refs.sort_by_key(|r| (r.q0rg_id, r.layer_id, r.placement_idx));
        for r in refs.into_iter().rev() {
            if let Some(layer) = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == r.q0rg_id)
                .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == r.layer_id))
            {
                if let Some(placement) = layer.placements.get(r.placement_idx) {
                    let frame = placement.frame;
                    layer.ensure_explicit_keyframe(frame);
                    layer.placements.remove(r.placement_idx);
                }
            }
        }
    }
    app.session.frame_edit_selection.clear();
    sync_selection(app);
    app.state.mark_dirty();
    app.textures.invalidate();
    true
}
