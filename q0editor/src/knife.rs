//! The knife: drag a line across drawn shapes to cut them apart. Fills
//! split into pieces that can be picked, moved and deleted on their own,
//! and outlines open along the cut, so a hat, a pan or a shield is broken
//! in a few strokes instead of being redrawn as a new symbol.
//!
//! The cut can leave a gap as wide as the knife setting, and can run along
//! a jagged line instead of the one drawn, for cracks.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Color32, Context, Painter, PointerButton, Pos2, Response, Shape, Stroke};
use geo::{Buffer, Coord, Intersects, LineString, MultiPolygon};
use q0s_format::v2::{Asset, Path as VPath, Placement, Target, Transform2D, Vec2, VectorAsset};

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{Selection, ToolState};

/// Steps each curve is followed in to find where the cut crosses it.
const STEPS: usize = 32;

/// Takes the pointer while the knife is the tool: a drag draws the cut,
/// Shift keeps it straight, and letting go cuts.
pub fn handle(
    app: &mut EditorApp,
    response: &Response,
    cursor: Option<Vec2>,
    view: &StageView,
    ctx: &Context,
) {
    let shift = ctx.input(|input| input.modifiers.shift);
    if response.drag_started_by(PointerButton::Primary) {
        let press = ctx
            .input(|input| input.pointer.press_origin())
            .map(|screen| view.screen_to_stage(screen))
            .or(cursor);
        if let Some(press) = press {
            app.session.tool_state = ToolState::KnifeDrawing {
                points: vec![press],
            };
            app.session.status = "Knife: drag across what to cut".to_string();
        }
    }
    if response.dragged_by(PointerButton::Primary) {
        if let (Some(point), ToolState::KnifeDrawing { points }) =
            (cursor, &mut app.session.tool_state)
        {
            if shift {
                points.truncate(1);
                points.push(point);
            } else {
                let far_enough = points.last().is_none_or(|last| {
                    (last.x - point.x).hypot(last.y - point.y) * view.effective_scale().abs() >= 2.0
                });
                if far_enough {
                    points.push(point);
                }
            }
        }
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::KnifeDrawing { points } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            app.session.status = if cut(app, &points) {
                "Cut".to_string()
            } else {
                "Nothing to cut there".to_string()
            };
        }
    }
}

/// The cut being drawn: a red line, jagged when the cut will be.
pub fn paint_preview(app: &EditorApp, painter: &Painter, view: &StageView) {
    let ToolState::KnifeDrawing { points } = &app.session.tool_state else {
        return;
    };
    let line = if app.settings.aids.knife_jagged {
        jagged(points, crack_step(app))
    } else {
        points.clone()
    };
    let screen: Vec<Pos2> = line
        .iter()
        .map(|point| view.stage_to_screen(*point))
        .collect();
    if screen.len() < 2 {
        return;
    }
    painter.add(Shape::line(
        screen.clone(),
        Stroke::new(3.0_f32, Color32::from_black_alpha(200)),
    ));
    painter.add(Shape::line(
        screen,
        Stroke::new(1.5_f32, Color32::from_rgb(255, 70, 60)),
    ));
}

fn crack_step(app: &EditorApp) -> f32 {
    (f32::from(app.settings.aids.knife_gap) * 3.0).max(7.0)
}

/// `line` made into a zigzag crack: points every `step` pixels, pushed off
/// to alternate sides by a varying amount. The same line always cracks the
/// same way.
pub(crate) fn jagged(line: &[Vec2], step: f32) -> Vec<Vec2> {
    let mut even = Vec::new();
    let mut carried = 0.0;
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = (b.x - a.x).hypot(b.y - a.y);
        let mut at = step - carried;
        while at < length {
            let t = at / length;
            even.push((
                Vec2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
                Vec2::new(-(b.y - a.y) / length, (b.x - a.x) / length),
            ));
            at += step;
        }
        carried = (carried + length) % step;
    }
    let (Some(first), Some(last)) = (line.first(), line.last()) else {
        return Vec::new();
    };
    let mut cracked = vec![*first];
    for (index, (point, normal)) in even.into_iter().enumerate() {
        // A small hash of the point picks how far this tooth reaches.
        let seed = (point.x * 12.9898 + point.y * 78.233).sin() * 43_758.547;
        let reach = step * (0.35 + 0.45 * (seed - seed.floor()));
        let side = if index % 2 == 0 { 1.0 } else { -1.0 };
        cracked.push(Vec2::new(
            point.x + normal.x * reach * side,
            point.y + normal.y * reach * side,
        ));
    }
    cracked.push(*last);
    cracked
}

fn distance_to_line(line: &[Vec2], point: Vec2) -> f32 {
    line.windows(2)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let length_sq = dx * dx + dy * dy;
            let t = if length_sq <= 1.0e-12 {
                0.0
            } else {
                (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0)
            };
            (a.x + dx * t - point.x).hypot(a.y + dy * t - point.y)
        })
        .fold(f32::INFINITY, f32::min)
}

fn segment_count(path: &VPath) -> usize {
    match path.anchors.len() {
        0 | 1 => 0,
        count if path.closed => count,
        count => count - 1,
    }
}

fn point_at(path: &VPath, param: f32) -> Vec2 {
    let segment = (param.floor() as usize).min(segment_count(path).saturating_sub(1));
    let t = param - segment as f32;
    let a = &path.anchors[segment];
    let b = &path.anchors[(segment + 1) % path.anchors.len()];
    let p1 = a.out_handle.unwrap_or(a.point);
    let p2 = b.in_handle.unwrap_or(b.point);
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    Vec2::new(
        w[0] * a.point.x + w[1] * p1.x + w[2] * p2.x + w[3] * b.point.x,
        w[0] * a.point.y + w[1] * p1.y + w[2] * p2.y + w[3] * b.point.y,
    )
}

/// What is left of `path` once everything within `reach` of `line` is cut
/// away: `None` when it does not come that near, otherwise the pieces.
pub(crate) fn trim_path(path: &VPath, line: &[Vec2], reach: f32) -> Option<Vec<VPath>> {
    let segments = segment_count(path);
    if segments == 0 {
        return None;
    }
    let inside = |param: f32| distance_to_line(line, point_at(path, param)) <= reach;
    let total = segments * STEPS;
    let params: Vec<f32> = (0..=total).map(|step| step as f32 / STEPS as f32).collect();
    let states: Vec<bool> = params.iter().map(|param| inside(*param)).collect();
    if !states.iter().any(|state| *state) {
        return None;
    }
    if states.iter().all(|state| *state) {
        return Some(Vec::new());
    }
    let mut cuts = Vec::new();
    for index in 0..total {
        if states[index] == states[index + 1] {
            continue;
        }
        let (mut low, mut high) = (params[index], params[index + 1]);
        for _ in 0..16 {
            let middle = (low + high) * 0.5;
            if inside(middle) == states[index] {
                low = middle;
            } else {
                high = middle;
            }
        }
        cuts.push((low + high) * 0.5);
    }
    let end = segments as f32;
    cuts.retain(|cut| *cut > 1.0e-4 && *cut < end - 1.0e-4);
    let pieces = if cuts.is_empty() {
        vec![path.clone()]
    } else {
        crate::crossings::split_path(path, &cuts)
    };
    Some(
        pieces
            .into_iter()
            .filter(|piece| {
                let middle = segment_count(piece) as f32 * 0.5;
                segment_count(piece) > 0 && distance_to_line(line, point_at(piece, middle)) > reach
            })
            .collect(),
    )
}

fn vector_mut(assets: &mut [Asset], asset_id: u16) -> Option<&mut VectorAsset> {
    match assets.iter_mut().find(|asset| asset.id() == asset_id)? {
        Asset::Vector(vector) => Some(vector),
        _ => None,
    }
}

/// Cuts what is drawn on the current layer and frame along `line`.
/// Returns whether anything was cut.
pub(crate) fn cut(app: &mut EditorApp, line: &[Vec2]) -> bool {
    let gap = f32::from(app.settings.aids.knife_gap);
    let line = if app.settings.aids.knife_jagged {
        jagged(line, crack_step(app))
    } else {
        line.to_vec()
    };
    if line.len() < 2 {
        return false;
    }
    let half = (gap * 0.5).max(0.3);
    let zone: MultiPolygon<f64> = LineString::new(
        line.iter()
            .map(|point| Coord {
                x: f64::from(point.x),
                y: f64::from(point.y),
            })
            .collect(),
    )
    .buffer(f64::from(half));
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;

    let before = app.state.project.clone();
    if crate::tools::materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
    )
    .is_none()
    {
        return false;
    }
    let reach_of =
        |stroke: Option<q0s_format::v2::Stroke>| half + stroke.map_or(0.0, |s| s.width * 0.5);

    // Plain drawings on this keyframe, with what the cut does to their lines.
    let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };
    let mut drawings: Vec<(usize, u16)> = Vec::new();
    for (placement_idx, _) in crate::render::active_placements_at(layer, frame) {
        let Some(placement) = layer.placements.get(placement_idx) else {
            continue;
        };
        if placement.frame != frame
            || placement.transform != Transform2D::IDENTITY
            || placement.tween.is_motion()
        {
            continue;
        }
        if let Target::Asset(asset_id) = placement.target {
            if matches!(
                app.state.project.assets.iter().find(|asset| asset.id() == asset_id),
                Some(Asset::Vector(vector)) if vector.stroke.is_some()
            ) {
                drawings.push((placement_idx, asset_id));
            }
        }
    }
    let mut trims: BTreeMap<u16, Vec<(usize, Vec<VPath>)>> = BTreeMap::new();
    let mut opened: BTreeSet<u16> = BTreeSet::new();
    for &(_, asset_id) in &drawings {
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let reach = reach_of(vector.stroke);
        for (path_idx, path) in vector.paths.iter().enumerate() {
            if let Some(pieces) = trim_path(path, &line, reach) {
                trims.entry(asset_id).or_default().push((path_idx, pieces));
            }
        }
        // A filled shape the cut reaches into has its fill and outline
        // parted, even where the cut stops short of the outline.
        if vector.fill.is_some()
            && (trims.contains_key(&asset_id)
                || crate::brush::vector_fill_geometry(vector).intersects(&zone))
        {
            opened.insert(asset_id);
            trims.entry(asset_id).or_default();
        }
    }

    let mut changed = false;
    if !trims.is_empty() {
        let writable = crate::brush::prepare_writable_raw_assets(
            &mut app.state.project,
            q0rg_id,
            layer_id,
            frame,
            &trims.keys().copied().collect(),
        );
        let mut new_lines: Vec<(u16, VectorAsset)> = Vec::new();
        let mut emptied = BTreeSet::new();
        for (asset_id, paths) in trims {
            let target = *writable.get(&asset_id).unwrap_or(&asset_id);
            let Some(vector) = vector_mut(&mut app.state.project.assets, target) else {
                continue;
            };
            if opened.contains(&asset_id) {
                // A filled shape's outline opens along the cut, so outline
                // and fill part ways: the fill stays here to be cut as a
                // fill, the outline pieces become their own drawing on top.
                let mut lines: Vec<VPath> = Vec::new();
                let trimmed: BTreeMap<usize, Vec<VPath>> = paths.into_iter().collect();
                for (path_idx, path) in vector.paths.iter().enumerate() {
                    match trimmed.get(&path_idx) {
                        Some(pieces) => lines.extend(pieces.iter().cloned()),
                        None => lines.push(path.clone()),
                    }
                }
                let stroke = vector.stroke.take();
                vector.paths.retain(|path| path.closed);
                if vector.paths.is_empty() {
                    emptied.insert(target);
                }
                if !lines.is_empty() {
                    new_lines.push((
                        target,
                        VectorAsset {
                            asset_id: 0,
                            paths: lines,
                            fill: None,
                            stroke,
                        },
                    ));
                }
            } else {
                // Later paths first, so earlier indices stay put.
                for (path_idx, pieces) in paths.into_iter().rev() {
                    if path_idx < vector.paths.len() {
                        vector.paths.splice(path_idx..=path_idx, pieces);
                    }
                }
                if vector.paths.is_empty() {
                    emptied.insert(target);
                }
            }
            changed = true;
        }
        for (owner, mut lines) in new_lines {
            let asset_id = crate::tools::next_asset_id_pub(&app.state.project);
            lines.asset_id = asset_id;
            app.state.project.assets.push(Asset::Vector(lines));
            if let Some(layer) = app
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
                let at = layer
                    .placements
                    .iter()
                    .position(|placement| {
                        placement.frame == frame && placement.target == Target::Asset(owner)
                    })
                    .map_or(layer.placements.len(), |index| index + 1);
                layer.placements.insert(
                    at,
                    Placement {
                        pivot: None,
                        instance_id: 0,
                        frame,
                        target: Target::Asset(asset_id),
                        transform: Transform2D::IDENTITY,
                        tween: q0s_format::v2::Tween::None,
                        fx: Default::default(),
                    },
                );
            }
        }
        crate::brush::remove_current_layer_asset_placements(
            &mut app.state.project,
            q0rg_id,
            layer_id,
            frame,
            &emptied,
        );
        crate::brush::remove_unreferenced_assets(&mut app.state.project, &emptied);
    }
    // Fills: the cut is erased out of them, leaving separate pieces.
    if crate::brush::erase_brush_region_without_snapshot(app, zone) {
        changed = true;
    }
    if changed {
        app.history.snapshot_with_session(&before, &app.session);
        app.textures.invalidate();
        app.session.selection = Selection::None;
        app.state.mark_dirty();
    } else {
        app.state.project = before;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::Anchor;

    fn line(points: &[(f32, f32)], closed: bool) -> VPath {
        VPath {
            anchors: points
                .iter()
                .map(|&(x, y)| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed,
        }
    }

    #[test]
    fn a_cut_opens_a_line_with_a_gap() {
        let path = line(&[(0.0, 0.0), (100.0, 0.0)], false);
        let knife = [Vec2::new(50.0, -20.0), Vec2::new(50.0, 20.0)];
        let pieces = trim_path(&path, &knife, 3.0).unwrap();
        assert_eq!(pieces.len(), 2);
        let left_end = pieces[0].anchors.last().unwrap().point;
        let right_start = pieces[1].anchors[0].point;
        assert!((left_end.x - 47.0).abs() < 0.05, "{left_end:?}");
        assert!((right_start.x - 53.0).abs() < 0.05, "{right_start:?}");
        assert!(trim_path(&path, &[Vec2::new(0.0, 50.0), Vec2::new(100.0, 50.0)], 3.0).is_none());
    }

    #[test]
    fn a_ring_cut_across_falls_in_two() {
        let ring = line(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let knife = [Vec2::new(50.0, -20.0), Vec2::new(50.0, 120.0)];
        let pieces = trim_path(&ring, &knife, 2.0).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|piece| !piece.closed));
    }

    #[test]
    fn cracks_zigzag_but_keep_their_ends() {
        let straight = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)];
        let crack = jagged(&straight, 10.0);
        assert_eq!(crack.first(), straight.first());
        assert_eq!(crack.last(), straight.last());
        assert!(crack.len() > 8);
        assert!(
            crack.iter().any(|point| point.y > 2.0) && crack.iter().any(|point| point.y < -2.0)
        );
        assert_eq!(crack, jagged(&straight, 10.0), "the same every time");
    }
}
