//! Snapping. While the magnet is on, a point being drawn or dragged is
//! pulled onto the nearest anchor, line end, object origin or stage corner
//! within reach, or failing that onto the nearest line, so lines meet
//! exactly and pieces of a broken sprite fit back together.
//!
//! What can be snapped to is gathered once per frame and drawing, and once
//! per drag for the thing being dragged, which is left out of it.

use std::cell::RefCell;

use egui::{Color32, Painter, Stroke};
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, Placement, ProjectV2, Target, Transform2D, Vec2};

use crate::app::EditorApp;
use crate::render::{flatten_path, StageView};
use crate::state::{PathRef, PlacementRef, Selection};

/// How close, in screen pixels, a point has to come to snap.
pub const REACH_PX: f32 = 10.0;

/// What a point snapped onto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapKind {
    /// An anchor, an object's origin or a stage corner.
    Point,
    /// Somewhere along a line or an outline.
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapped {
    pub point: Vec2,
    pub kind: SnapKind,
}

/// What a snap scene leaves out: the things being drawn or moved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Leave {
    /// Whole paths.
    pub paths: Vec<PathRef>,
    /// Only these anchors of a path; its outline is left out too, since it
    /// bends with them.
    pub anchors: Vec<(PathRef, Vec<usize>)>,
    pub placements: Vec<PlacementRef>,
}

impl Leave {
    /// Everything `selection` holds.
    pub fn selection(selection: &Selection) -> Self {
        let mut leave = Self::default();
        match selection {
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => leave.paths.push(PathRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
                path_idx: *path_idx,
            }),
            Selection::Paths(refs) => leave.paths.extend(refs.iter().copied()),
            Selection::Mixed { paths, objects } => {
                leave.paths.extend(paths.iter().copied());
                leave.placements.extend(objects.iter().copied());
            }
            Selection::RawParts {
                fills,
                strokes,
                stroke_segments,
                objects,
            } => {
                leave.paths.extend(fills.iter().copied());
                leave.paths.extend(strokes.iter().copied());
                leave
                    .paths
                    .extend(stroke_segments.iter().map(|segment| segment.path));
                leave.placements.extend(objects.iter().copied());
            }
            Selection::StrokeSegment(segment) => leave.paths.push(segment.path),
            Selection::PathPoints {
                path,
                anchor_indices,
                ..
            }
            | Selection::SubselectPoints {
                path,
                anchor_indices,
                ..
            } => leave.anchors.push((*path, anchor_indices.clone())),
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => leave.placements.push(PlacementRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
            }),
            Selection::Multi(refs) => leave.placements.extend(refs.iter().copied()),
            Selection::RawArea {
                placements,
                objects,
                ..
            } => {
                leave.placements.extend(placements.iter().copied());
                leave.placements.extend(objects.iter().copied());
            }
            Selection::None | Selection::Asset(_) | Selection::Q0rg(_) => {}
        }
        leave
    }

    fn leaves_placement(&self, layer_id: u16, placement_idx: usize) -> bool {
        self.placements
            .iter()
            .any(|object| object.layer_id == layer_id && object.placement_idx == placement_idx)
    }

    fn leaves_path(&self, layer_id: u16, placement_idx: usize, path_idx: usize) -> bool {
        let same = |path: &PathRef| {
            path.layer_id == layer_id
                && path.placement_idx == placement_idx
                && path.path_idx == path_idx
        };
        self.paths.iter().any(same) || self.anchors.iter().any(|(path, _)| same(path))
    }

    fn leaves_anchor(
        &self,
        layer_id: u16,
        placement_idx: usize,
        path_idx: usize,
        anchor: usize,
    ) -> bool {
        self.paths.iter().any(|path| {
            path.layer_id == layer_id
                && path.placement_idx == placement_idx
                && path.path_idx == path_idx
        }) || self.anchors.iter().any(|(path, anchors)| {
            path.layer_id == layer_id
                && path.placement_idx == placement_idx
                && path.path_idx == path_idx
                && anchors.contains(&anchor)
        })
    }
}

/// Everything on one frame a point can snap to.
#[derive(Debug, Clone, Default)]
pub struct SnapScene {
    points: Vec<Vec2>,
    lines: Vec<Vec<Vec2>>,
}

impl SnapScene {
    /// The anchors, outlines and object origins on `frame` of `q0rg_id`'s
    /// visible layers, less what `leave` names. `stage` adds the stage's
    /// corners, edge middles and middle.
    pub fn build(
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        leave: &Leave,
        stage: Option<[f32; 2]>,
    ) -> Self {
        let mut scene = Self::default();
        // A symbol's own origin is where it is placed from.
        scene.points.push(Vec2::new(0.0, 0.0));
        if let Some([width, height]) = stage {
            for x in [0.0, width * 0.5, width] {
                for y in [0.0, height * 0.5, height] {
                    scene.points.push(Vec2::new(x, y));
                }
            }
        }
        let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
            return scene;
        };
        for layer in &q0rg.layers {
            if !project.layer_is_visible(q0rg_id, layer.layer_id) {
                continue;
            }
            for (placement_idx, transform) in crate::render::active_placements_at(layer, frame) {
                let Some(placement) = layer.placements.get(placement_idx) else {
                    continue;
                };
                let affine = Affine::from_transform(transform);
                match placement.target {
                    Target::Asset(asset_id) => {
                        let Some(Asset::Vector(vector)) =
                            project.assets.iter().find(|asset| asset.id() == asset_id)
                        else {
                            if !leave.leaves_placement(layer.layer_id, placement_idx) {
                                scene
                                    .points
                                    .extend(object_grips(project, placement, transform));
                            }
                            continue;
                        };
                        if leave.leaves_placement(layer.layer_id, placement_idx) {
                            continue;
                        }
                        for (path_idx, path) in vector.paths.iter().enumerate() {
                            for (anchor_idx, anchor) in path.anchors.iter().enumerate() {
                                if !leave.leaves_anchor(
                                    layer.layer_id,
                                    placement_idx,
                                    path_idx,
                                    anchor_idx,
                                ) {
                                    scene.points.push(affine.apply(anchor.point));
                                }
                            }
                            if !leave.leaves_path(layer.layer_id, placement_idx, path_idx) {
                                let line: Vec<Vec2> = flatten_path(path)
                                    .into_iter()
                                    .map(|point| affine.apply(point))
                                    .collect();
                                if line.len() >= 2 {
                                    scene.lines.push(line);
                                }
                            }
                        }
                    }
                    Target::Q0rg(_) => {
                        if !leave.leaves_placement(layer.layer_id, placement_idx) {
                            scene
                                .points
                                .extend(object_grips(project, placement, transform));
                        }
                    }
                }
            }
        }
        scene
    }

    /// The nearest point within `reach` of `point`, or failing that the
    /// nearest place on a line.
    pub fn nearest(&self, point: Vec2, reach: f32) -> Option<Snapped> {
        let reach_sq = reach * reach;
        let mut best: Option<(f32, Vec2)> = None;
        for candidate in &self.points {
            let distance = distance_sq(*candidate, point);
            if distance <= reach_sq && best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, *candidate));
            }
        }
        if let Some((_, point)) = best {
            return Some(Snapped {
                point,
                kind: SnapKind::Point,
            });
        }
        let mut best: Option<(f32, Vec2)> = None;
        for line in &self.lines {
            for pair in line.windows(2) {
                let on = nearest_on_segment(pair[0], pair[1], point);
                let distance = distance_sq(on, point);
                if distance <= reach_sq && best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, on));
                }
            }
        }
        best.map(|(_, point)| Snapped {
            point,
            kind: SnapKind::Line,
        })
    }
}

fn distance_sq(a: Vec2, b: Vec2) -> f32 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

fn nearest_on_segment(a: Vec2, b: Vec2, point: Vec2) -> Vec2 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= 1.0e-12 {
        return a;
    }
    let t = (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0);
    Vec2::new(a.x + dx * t, a.y + dy * t)
}

fn object_grips(project: &ProjectV2, placement: &Placement, transform: Transform2D) -> Vec<Vec2> {
    let affine = Affine::from_transform(transform);
    let mut grips = vec![affine.apply(placement.pivot.unwrap_or(Vec2::new(0.0, 0.0)))];
    let placed = Placement {
        transform,
        ..placement.clone()
    };
    if let Some((x0, y0, x1, y1)) = crate::render::placement_bbox(project, &placed) {
        for x in [x0, (x0 + x1) * 0.5, x1] {
            for y in [y0, (y0 + y1) * 0.5, y1] {
                grips.push(Vec2::new(x, y));
            }
        }
    }
    grips
}

/// What one drag snaps: the point of the dragged thing nearest where it
/// was grabbed, and what it may land on.
struct Moving {
    start_cursor: Vec2,
    /// From the cursor to the snapping point of what is dragged.
    offset: Vec2,
    scene: SnapScene,
}

#[derive(PartialEq)]
struct SceneKey {
    document: u64,
    revision: u64,
    q0rg: u16,
    frame: u16,
    leave: Leave,
}

#[derive(Default)]
struct SnapState {
    /// The scene drawing tools snap to, and what it was built for.
    scene: Option<(SceneKey, SnapScene)>,
    moving: Option<Moving>,
    /// The last snap and the paint pass it happened in, to show it.
    hint: Option<(Snapped, u64)>,
}

thread_local! {
    static STATE: RefCell<SnapState> = RefCell::new(SnapState::default());
}

fn stage_size(app: &EditorApp) -> Option<[f32; 2]> {
    // Stage corners only mean something on a scene's own timeline.
    app.session.edit_in_place.is_empty().then(|| {
        [
            f32::from(app.state.project.meta.stage_width),
            f32::from(app.state.project.meta.stage_height),
        ]
    })
}

fn reach(view: &StageView) -> f32 {
    REACH_PX / view.effective_scale().abs().max(1.0e-3)
}

fn note(snapped: Snapped, ctx_frame: u64) {
    STATE.with(|state| state.borrow_mut().hint = Some((snapped, ctx_frame)));
}

/// `point` snapped onto what is on the current frame, less `leave`, while
/// the magnet is on.
pub fn point(
    app: &EditorApp,
    point: Vec2,
    view: &StageView,
    leave: &Leave,
    ctx_frame: u64,
) -> Vec2 {
    if !app.settings.aids.snap {
        return point;
    }
    let key = SceneKey {
        document: app.active_project_tab_id().unwrap_or(0),
        revision: app.state.render_revision(),
        q0rg: app.session.current_q0rg_id,
        frame: app.session.current_frame,
        leave: leave.clone(),
    };
    let snapped = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.scene.as_ref().is_none_or(|(built, _)| *built != key) {
            let scene = SnapScene::build(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                leave,
                stage_size(app),
            );
            state.scene = Some((key, scene));
        }
        state
            .scene
            .as_ref()
            .and_then(|(_, scene)| scene.nearest(point, reach(view)))
    });
    match snapped {
        Some(snapped) => {
            note(snapped, ctx_frame);
            snapped.point
        }
        None => point,
    }
}

/// Where the cursor of a drag that began at `start_cursor` should be taken
/// to be, so the dragged selection's snapping point lands on something.
/// The first call of a drag works out what is dragged from the selection.
pub fn move_cursor(
    app: &EditorApp,
    start_cursor: Vec2,
    cursor: Vec2,
    view: &StageView,
    ctx_frame: u64,
) -> Vec2 {
    if !app.settings.aids.snap {
        return cursor;
    }
    let snapped = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state
            .moving
            .as_ref()
            .is_none_or(|moving| moving.start_cursor != start_cursor)
        {
            let grips = selection_grips(&app.state.project, &app.session.selection);
            let grip = grips
                .iter()
                .copied()
                .min_by(|a, b| {
                    distance_sq(*a, start_cursor).total_cmp(&distance_sq(*b, start_cursor))
                })
                .unwrap_or(start_cursor);
            let scene = SnapScene::build(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                &Leave::selection(&app.session.selection),
                stage_size(app),
            );
            state.moving = Some(Moving {
                start_cursor,
                offset: Vec2::new(grip.x - start_cursor.x, grip.y - start_cursor.y),
                scene,
            });
        }
        let moving = state.moving.as_ref()?;
        let grip = Vec2::new(cursor.x + moving.offset.x, cursor.y + moving.offset.y);
        let snapped = moving.scene.nearest(grip, reach(view))?;
        Some((snapped, moving.offset))
    });
    match snapped {
        Some((snapped, offset)) => {
            note(snapped, ctx_frame);
            Vec2::new(snapped.point.x - offset.x, snapped.point.y - offset.y)
        }
        None => cursor,
    }
}

/// Forgets the drag in progress.
pub fn end_move() {
    STATE.with(|state| state.borrow_mut().moving = None);
}

/// The points of `selection` that snap when it is dragged: its anchors and
/// the corners and middle of objects.
fn selection_grips(project: &ProjectV2, selection: &Selection) -> Vec<Vec2> {
    let leave = Leave::selection(selection);
    let mut grips = Vec::new();
    let placement = |q0rg_id: u16, layer_id: u16, placement_idx: usize| {
        project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?
            .placements
            .get(placement_idx)
    };
    let vector_of = |target: Target| match target {
        Target::Asset(asset_id) => match project.assets.iter().find(|asset| asset.id() == asset_id)
        {
            Some(Asset::Vector(vector)) => Some(vector),
            _ => None,
        },
        Target::Q0rg(_) => None,
    };
    for path in &leave.paths {
        let Some(placement) = placement(path.q0rg_id, path.layer_id, path.placement_idx) else {
            continue;
        };
        let affine = Affine::from_transform(placement.transform);
        if let Some(raw) =
            vector_of(placement.target).and_then(|vector| vector.paths.get(path.path_idx))
        {
            grips.extend(raw.anchors.iter().map(|anchor| affine.apply(anchor.point)));
        }
    }
    for (path, anchors) in &leave.anchors {
        let Some(placement) = placement(path.q0rg_id, path.layer_id, path.placement_idx) else {
            continue;
        };
        let affine = Affine::from_transform(placement.transform);
        if let Some(raw) =
            vector_of(placement.target).and_then(|vector| vector.paths.get(path.path_idx))
        {
            grips.extend(
                anchors
                    .iter()
                    .filter_map(|index| raw.anchors.get(*index))
                    .map(|anchor| affine.apply(anchor.point)),
            );
        }
    }
    for object in &leave.placements {
        let Some(placement) = placement(object.q0rg_id, object.layer_id, object.placement_idx)
        else {
            continue;
        };
        let affine = Affine::from_transform(placement.transform);
        match vector_of(placement.target) {
            Some(vector) => grips.extend(
                vector
                    .paths
                    .iter()
                    .flat_map(|path| path.anchors.iter())
                    .map(|anchor| affine.apply(anchor.point)),
            ),
            None => grips.extend(object_grips(project, placement, placement.transform)),
        }
    }
    grips
}

/// A ring where the last snap landed, while it is fresh.
pub fn paint_hint(app: &EditorApp, painter: &Painter, view: &StageView, ctx_frame: u64) {
    if !app.settings.aids.snap {
        return;
    }
    let Some((snapped, frame)) = STATE.with(|state| state.borrow().hint) else {
        return;
    };
    if frame + 1 < ctx_frame {
        return;
    }
    let at = view.stage_to_screen(snapped.point);
    let accent = app.settings.theme.accent.to_color32();
    let outline = Stroke::new(3.0_f32, Color32::from_black_alpha(200));
    let ring = Stroke::new(1.5_f32, accent);
    match snapped.kind {
        SnapKind::Point => {
            painter.circle_stroke(at, 7.0, outline);
            painter.circle_stroke(at, 7.0, ring);
            painter.circle_filled(at, 2.0, accent);
        }
        SnapKind::Line => {
            let rect = egui::Rect::from_center_size(at, egui::vec2(10.0, 10.0));
            painter.rect_stroke(rect, 0.0, outline);
            painter.rect_stroke(rect, 0.0, ring);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_snap_uses_visible_symbol_bounds_and_respects_the_toggle() {
        let mut app = EditorApp::default();
        app.state
            .project
            .assets
            .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                asset_id: 60000,
                width: 20,
                height: 20,
                rgba: vec![255; 20 * 20 * 4].into(),
            }));
        let bitmap = Placement {
            instance_id: 1,
            pivot: None,
            frame: 0,
            target: Target::Asset(60000),
            transform: Transform2D {
                tx: 30.0,
                ty: 20.0,
                ..Transform2D::IDENTITY
            },
            tween: q0s_format::v2::Tween::None,
            fx: Default::default(),
        };
        let mut child = app.state.project.q0rgs[0].clone();
        child.q0rg_id = 2;
        child.layers[0].placements = vec![bitmap.clone()];
        app.state.project.q0rgs.push(child);
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 100.0,
                    ty: 100.0,
                    ..Transform2D::IDENTITY
                },
                ..bitmap.clone()
            },
            Placement {
                transform: Transform2D {
                    tx: 200.0,
                    ty: 120.0,
                    ..Transform2D::IDENTITY
                },
                ..bitmap
            },
        ];
        let visible = SnapScene::build(&app.state.project, 1, 0, &Leave::default(), None);
        assert_eq!(
            visible.nearest(Vec2::new(149.5, 130.0), 1.0).unwrap().point,
            Vec2::new(150.0, 130.0)
        );
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let view = StageView {
            origin: egui::Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::EVERYTHING,
            edit_transform: Affine::IDENTITY,
        };
        let start = Vec2::new(140.0, 130.0);
        let cursor = Vec2::new(198.0, 130.0);
        end_move();
        app.settings.aids.snap = true;
        assert_eq!(
            move_cursor(&app, start, cursor, &view, 0),
            Vec2::new(200.0, 130.0)
        );
        app.settings.aids.snap = false;
        assert_eq!(move_cursor(&app, start, cursor, &view, 0), cursor);
        end_move();
    }

    #[test]
    fn points_win_over_lines_and_nothing_far_away_snaps() {
        let scene = SnapScene {
            points: vec![Vec2::new(10.0, 10.0)],
            lines: vec![vec![Vec2::new(0.0, 20.0), Vec2::new(100.0, 20.0)]],
        };
        let near_both = scene.nearest(Vec2::new(12.0, 16.0), 8.0).unwrap();
        assert_eq!(near_both.kind, SnapKind::Point);
        assert_eq!(near_both.point, Vec2::new(10.0, 10.0));
        let on_line = scene.nearest(Vec2::new(60.0, 23.0), 8.0).unwrap();
        assert_eq!(on_line.kind, SnapKind::Line);
        assert!((on_line.point.x - 60.0).abs() < 1.0e-3 && on_line.point.y == 20.0);
        assert!(scene.nearest(Vec2::new(60.0, 60.0), 8.0).is_none());
    }
}
