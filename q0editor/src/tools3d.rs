//! 3D tools on the stage: move, turn and size anything in 3D with the same
//! gizmos as the model editor, and a free view to look at the stage from
//! any side. They show in the tool column once the project has 3D in it.

use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, Response,
    Shape, Stroke, Ui, Vec2,
};
use q0s_format::three::draw::View3d;
use q0s_format::three::frame;
use q0s_format::three::scene::{placement_matrix, CameraPose, Space3d};
use q0s_format::three::{Mat4, Quat, V3};
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, PlacementFx, ProjectV2, Target, Transform2D, Vec2 as StageVec};

use crate::app::EditorApp;
use crate::model_studio::gizmo::{self, Gizmo, Handle, Kind, AXIS_COLOURS};
use crate::render::StageView;
use crate::state::{PlacementRef, Selection};

/// Screen length of a gizmo arrow.
const ARROW: f32 = 84.0;
/// The depth knob sits this far below the gizmo when its z arrow points
/// at the eye and cannot be grabbed.
const KNOB_OFFSET: Vec2 = vec2(0.0, 58.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool3d {
    Move,
    Rotate,
    Scale,
    View,
}

impl Tool3d {
    pub(crate) const ALL: [Self; 4] = [Self::Move, Self::Rotate, Self::Scale, Self::View];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Move => "Move in 3D",
            Self::Rotate => "Turn in 3D",
            Self::Scale => "Size in 3D",
            Self::View => "3D view",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Move => "Move in 3D: drag an arrow to move along it, a square to slide in its plane, the dot to move freely. Pushes drawings into depth too.",
            Self::Rotate => "Turn in 3D: drag a ring to turn about its axis, the outer ring to turn toward you.",
            Self::Scale => "Size in 3D: drag a handle to stretch along it, the middle to size evenly.",
            Self::View => "3D view: drag to look around the stage, Shift-drag to slide, wheel to come closer. Click to pick.",
        }
    }
}

/// A free camera circling a point of the stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub target: V3,
    /// Radians about the stage's vertical axis.
    pub yaw: f32,
    /// Radians up from level; looking down is positive.
    pub pitch: f32,
    pub distance: f32,
}

impl Orbit {
    fn from_camera(camera: &CameraPose) -> Self {
        let offset = camera.target - camera.position;
        let distance = offset.length().max(1.0);
        let forward = offset / distance;
        Self {
            target: camera.target,
            yaw: forward.x.atan2(forward.z),
            pitch: forward.y.clamp(-1.0, 1.0).asin(),
            distance,
        }
    }

    fn forward(&self) -> V3 {
        V3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }

    fn right(&self) -> V3 {
        V3::Y.cross(self.forward()).normalized()
    }

    fn down(&self) -> V3 {
        self.forward().cross(self.right()).normalized()
    }

    fn camera(&self, zoom: f32) -> CameraPose {
        CameraPose {
            position: self.target - self.forward() * self.distance,
            target: self.target,
            roll: 0.0,
            zoom,
        }
    }
}

/// What the 3D tools keep for one document.
#[derive(Default)]
pub struct Tools3dSession {
    pub tool: Option<Tool3d>,
    /// Looking through a free camera instead of the scene's.
    pub orbit: Option<Orbit>,
    /// The gizmo follows the object's own axes instead of the stage's.
    pub own_axes: bool,
    drag: Option<Drag>,
    hover: Option<Grip>,
}

impl Tools3dSession {
    /// Back to the 2D tools and the scene camera.
    pub fn leave(&mut self) {
        self.tool = None;
        self.orbit = None;
        self.drag = None;
        self.hover = None;
    }

    pub fn active(&self) -> bool {
        self.tool.is_some()
    }
}

/// A part of the gizmo that can be held.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Gizmo(Handle),
    /// Pushes the object away or pulls it near, when its z arrow points at
    /// the eye.
    Depth,
}

#[derive(Debug, Clone, Copy)]
struct Pose {
    transform: Transform2D,
    space: Space3d,
}

#[derive(Debug, Clone)]
struct Drag {
    kind: Kind,
    grip: Grip,
    reference: PlacementRef,
    start: Pose,
    pivot: StageVec,
    centre: V3,
    axes: [V3; 3],
    /// Where the pointer first met the axis or plane, in stage space.
    grab: V3,
    press: Pos2,
}

/// One placement as the 3D tools see it.
struct Object {
    reference: PlacementRef,
    transform: Transform2D,
    space: Space3d,
    /// Its own coordinates to stage space.
    matrix: Mat4,
    /// The point it turns and sizes about, in its own coordinates.
    pivot: StageVec,
    /// Where that point is in stage space.
    centre: V3,
    model: bool,
}

/// Does this project have 3D, so the tools are worth showing?
pub(crate) fn offered(project: &ProjectV2) -> bool {
    frame::project_uses_3d(project)
}

/// The stage camera's lens: how far the free camera zooms.
fn lens(project: &ProjectV2) -> f32 {
    CameraPose::stage(project.meta.stage_width, project.meta.stage_height).zoom
}

/// The free camera the stage is drawn through, when there is one.
pub(crate) fn free_view(app: &EditorApp) -> Option<q0glblend::FreeView> {
    let orbit = app.session.tools3d.orbit?;
    Some(q0glblend::FreeView {
        camera: orbit.camera(lens(&app.state.project)),
        orthographic: false,
    })
}

/// The 3D view of the stage as it is drawn now: through the scene camera,
/// or the free one.
pub(crate) fn stage_view3d(app: &EditorApp, view: &StageView) -> View3d {
    let project = &app.state.project;
    let mut look = frame::scene_look(
        project,
        app.session.current_q0rg_id,
        app.session.current_frame,
        |_| None,
    );
    if let Some(free) = free_view(app) {
        look.camera = free.camera;
        look.orthographic = free.orthographic;
    }
    look.view(project, view.screen_transform(), [1, 1])
}

fn to_screen(view3d: &View3d, at: V3) -> Option<Pos2> {
    view3d.project(at).map(|[x, y, _]| pos2(x, y))
}

fn local_pivot(
    project: &ProjectV2,
    target: Target,
    pivot: Option<StageVec>,
    model: bool,
) -> StageVec {
    if let Some(pivot) = pivot {
        return pivot;
    }
    if model {
        return StageVec::new(0.0, 0.0);
    }
    crate::render::target_local_bbox(project, target, None)
        .map_or(StageVec::new(0.0, 0.0), |(x0, y0, x1, y1)| {
            StageVec::new((x0 + x1) * 0.5, (y0 + y1) * 0.5)
        })
}

fn is_model(project: &ProjectV2, target: Target) -> bool {
    matches!(target, Target::Asset(id) if project.assets.iter().any(|asset| matches!(asset, Asset::Model(m) if m.asset_id == id)))
}

/// A placement's 3D state on the current frame, as other parts of the
/// editor need it.
pub(crate) struct ObjectState {
    pub transform: Transform2D,
    pub space: Space3d,
    /// The point it turns about, in its own coordinates.
    pub pivot: StageVec,
    /// Where that point is in stage space.
    pub centre: V3,
}

pub(crate) fn object_state(app: &EditorApp, reference: PlacementRef) -> Option<ObjectState> {
    let object = object(app, reference)?;
    Some(ObjectState {
        transform: object.transform,
        space: object.space,
        pivot: object.pivot,
        centre: object.centre,
    })
}

/// A placement's turn as one rotation.
pub(crate) fn orientation_of(transform: &Transform2D, space: &Space3d) -> Quat {
    orientation(&Pose {
        transform: *transform,
        space: *space,
    })
}

/// A placement's 3D state on the current frame.
fn object(app: &EditorApp, reference: PlacementRef) -> Option<Object> {
    let project = &app.state.project;
    let layer = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?;
    let placement = layer.placements.get(reference.placement_idx)?;
    let (transform, fx) = crate::render::active_state_for_placement(
        layer,
        reference.placement_idx,
        app.session.current_frame,
    )?;
    let model = is_model(project, placement.target);
    let space = fx.three.unwrap_or_default();
    let pivot = local_pivot(project, placement.target, placement.pivot, model);
    let matrix = matrix_of(&transform, &fx, pivot);
    let centre = matrix.transform_point(V3::new(pivot.x, pivot.y, 0.0));
    Some(Object {
        reference,
        transform,
        space,
        matrix,
        pivot,
        centre,
        model,
    })
}

fn matrix_of(transform: &Transform2D, fx: &PlacementFx, pivot: StageVec) -> Mat4 {
    placement_matrix(
        &Mat4::IDENTITY,
        Affine::from_transform(*transform),
        fx.three,
        pivot,
    )
}

/// The object's own axes in stage space, or the stage's, in the order x,
/// y (shown pointing up) and z (into the stage).
fn axes_of(object: &Object, own: bool) -> [V3; 3] {
    let stage = [V3::X, -V3::Y, V3::Z];
    if !own {
        return stage;
    }
    let m = &object.matrix;
    let mut axes = [
        m.transform_vector(V3::X).normalized(),
        -m.transform_vector(V3::Y).normalized(),
        m.transform_vector(V3::Z).normalized(),
    ];
    for (axis, fallback) in axes.iter_mut().zip(stage) {
        if axis.length_sq() < 0.5 {
            *axis = fallback;
        }
    }
    axes
}

fn kind_of(tool: Tool3d) -> Option<Kind> {
    match tool {
        Tool3d::Move => Some(Kind::Move),
        Tool3d::Rotate => Some(Kind::Rotate),
        Tool3d::Scale => Some(Kind::Scale),
        Tool3d::View => None,
    }
}

/// The gizmo of the current tool around `object`.
fn layout(view3d: &View3d, object: &Object, kind: Kind, axes: [V3; 3]) -> Option<Gizmo> {
    let length = gizmo::world_length(view3d, object.centre, ARROW);
    let mut gizmo = Gizmo::project(view3d, kind, object.centre, axes, length)?;
    if kind == Kind::Scale {
        // Stage placements store width and height; depth follows their size.
        gizmo.tips[2] = None;
        gizmo.plane_centres[..2].fill(None);
    }
    Some(gizmo)
}

/// The depth knob's place, when the z arrow is too short to hold.
fn depth_knob(gizmo: &Gizmo) -> Option<Pos2> {
    if gizmo.kind != Kind::Move {
        return None;
    }
    let short = gizmo.tips[2].is_none_or(|tip| tip.distance(gizmo.centre) < 26.0);
    short.then_some(gizmo.centre + KNOB_OFFSET)
}

fn grip_at(gizmo: &Gizmo, pos: Pos2) -> Option<Grip> {
    if depth_knob(gizmo).is_some_and(|knob| knob.distance(pos) < 13.0) {
        return Some(Grip::Depth);
    }
    gizmo::hit(gizmo, pos).map(Grip::Gizmo)
}

fn selected(app: &EditorApp) -> Option<PlacementRef> {
    match app.session.selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } if q0rg_id == app.session.current_q0rg_id => Some(PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx,
        }),
        _ => None,
    }
}

/// Placements on screen with the box around each, front ones last.
fn objects_on_screen(app: &EditorApp, view3d: &View3d) -> Vec<(PlacementRef, Rect, f32)> {
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
            || project.layer_is_folder(q0rg_id, layer.layer_id)
        {
            continue;
        }
        for (placement_idx, _) in crate::render::active_placements_at(layer, frame) {
            let reference = PlacementRef {
                q0rg_id,
                layer_id: layer.layer_id,
                placement_idx,
            };
            let Some(object) = object(app, reference) else {
                continue;
            };
            let Some(rect) = screen_box(
                project,
                &object,
                view3d,
                layer.placements[placement_idx].target,
            ) else {
                continue;
            };
            let depth = view3d.view.transform_point(object.centre).z;
            out.push((reference, rect, depth));
        }
    }
    out
}

fn screen_box(
    project: &ProjectV2,
    object: &Object,
    view3d: &View3d,
    target: Target,
) -> Option<Rect> {
    if object.model {
        let Target::Asset(id) = target else {
            return None;
        };
        let model = project.assets.iter().find_map(|asset| match asset {
            Asset::Model(m) if m.asset_id == id => Some(m),
            _ => None,
        })?;
        let world = model.data.world_matrices(&model.data.rest_locals());
        let [x0, y0, x1, y1] =
            frame::model_output_bounds(&model.data, &object.matrix, &world, view3d)?;
        return Some(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)));
    }
    let (x0, y0, x1, y1) = crate::render::target_local_bbox(project, target, None)?;
    let mut rect: Option<Rect> = None;
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
        let p = to_screen(view3d, object.matrix.transform_point(V3::new(x, y, 0.0)))?;
        rect = Some(rect.map_or(Rect::from_min_max(p, p), |r| {
            r.union(Rect::from_min_max(p, p))
        }));
    }
    rect
}

/// What is under the pointer: the nearest thing whose box holds it, the
/// later layer when two are as near.
fn pick(app: &EditorApp, view3d: &View3d, at: Pos2) -> Option<PlacementRef> {
    let mut best: Option<(PlacementRef, f32)> = None;
    for (reference, rect, depth) in objects_on_screen(app, view3d) {
        if !rect.expand(3.0).contains(at) {
            continue;
        }
        if best.is_none_or(|(_, nearest)| depth <= nearest + 0.5) {
            best = Some((reference, depth));
        }
    }
    best.map(|(reference, _)| reference)
}

/// Stage input while a 3D tool is on. Runs before the stage is drawn.
pub(crate) fn handle(app: &mut EditorApp, response: &Response, view: &StageView) {
    if crate::scene3d_stage::handle_stage(app, response, view) {
        return;
    }
    let Some(tool) = app.session.tools3d.tool else {
        return;
    };
    let ctx = response.ctx.clone();
    navigate(app, response, view, tool);
    let view3d = stage_view3d(app, view);
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    let snapping = ctx.input(|i| i.modifiers.ctrl);

    // Holding a gizmo.
    if app.session.tools3d.drag.is_some() {
        let dragging = response.dragged_by(PointerButton::Primary);
        if dragging || response.drag_stopped_by(PointerButton::Primary) {
            if let Some(at) = pointer {
                drag_to(app, &view3d, at, snapping);
            }
        }
        if dragging {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            return;
        }
        app.session.tools3d.drag = None;
        app.state.mark_dirty();
        app.textures.invalidate();
        return;
    }

    let gizmo = kind_of(tool).and_then(|kind| {
        let object = object(app, selected(app)?)?;
        let axes = axes_of(&object, app.session.tools3d.own_axes || kind == Kind::Scale);
        Some((layout(&view3d, &object, kind, axes)?, object, axes))
    });
    let hover = response
        .hover_pos()
        .zip(gizmo.as_ref())
        .and_then(|(at, (g, _, _))| grip_at(g, at));
    app.session.tools3d.hover = hover;

    if response.drag_started_by(PointerButton::Primary) {
        let press = ctx.input(|i| i.pointer.press_origin()).or(pointer);
        if let (Some(press), Some((g, object, axes))) = (press, gizmo) {
            if let Some(grip) = grip_at(&g, press) {
                begin(app, &view3d, object, axes, grip, press);
                if let Some(at) = pointer {
                    drag_to(app, &view3d, at, snapping);
                }
                return;
            }
        }
        // Grabbing the thing itself moves it freely.
        if tool == Tool3d::Move {
            if let Some(press) = press {
                if let Some(reference) = pick(app, &view3d, press) {
                    select(app, reference);
                    if let Some(object) = object(app, reference) {
                        let axes = axes_of(&object, app.session.tools3d.own_axes);
                        begin(app, &view3d, object, axes, Grip::Gizmo(Handle::Free), press);
                        if let Some(at) = pointer {
                            drag_to(app, &view3d, at, snapping);
                        }
                    }
                }
            }
        }
        return;
    }
    if response.clicked_by(PointerButton::Primary) {
        if let Some(at) = pointer {
            match pick(app, &view3d, at) {
                Some(reference) => select(app, reference),
                None => app.session.selection = Selection::None,
            }
        }
    }
    if hover.is_some() {
        ctx.set_cursor_icon(CursorIcon::Grab);
    } else if tool == Tool3d::View {
        ctx.set_cursor_icon(CursorIcon::Move);
    }
}

fn select(app: &mut EditorApp, reference: PlacementRef) {
    app.session.selection = Selection::Placement {
        q0rg_id: reference.q0rg_id,
        layer_id: reference.layer_id,
        placement_idx: reference.placement_idx,
    };
    app.session.current_layer_id = reference.layer_id;
}

/// Looking around: the 3D view tool's drag, the right or middle button's
/// with any 3D tool, and the wheel in the free view.
fn navigate(app: &mut EditorApp, response: &Response, view: &StageView, tool: Tool3d) {
    let ctx = &response.ctx;
    let (delta, shift, scroll) = ctx.input(|i| {
        (
            i.pointer.delta(),
            i.modifiers.shift,
            i.smooth_scroll_delta.y,
        )
    });
    let held_gizmo = app.session.tools3d.drag.is_some();
    let looking = response.dragged_by(PointerButton::Middle)
        || response.dragged_by(PointerButton::Secondary)
        || (tool == Tool3d::View && response.dragged_by(PointerButton::Primary) && !held_gizmo);
    if looking && app.session.tools3d.orbit.is_none() {
        // Leave the scene camera from exactly where it looks.
        let look = frame::scene_look(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_frame,
            |_| None,
        );
        app.session.tools3d.orbit = Some(Orbit::from_camera(&look.camera));
    }
    let zoom = lens(&app.state.project);
    let Some(orbit) = app.session.tools3d.orbit.as_mut() else {
        return;
    };
    if looking && delta != Vec2::ZERO {
        if shift {
            let per_pixel = orbit.distance / zoom / view.effective_scale().max(1.0e-6);
            orbit.target = orbit.target
                - orbit.right() * (delta.x * per_pixel)
                - orbit.down() * (delta.y * per_pixel);
        } else {
            orbit.yaw += delta.x * 0.005;
            orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(-1.5, 1.5);
        }
        ctx.request_repaint();
    }
    if response.hovered() && scroll != 0.0 {
        orbit.distance = (orbit.distance * (-scroll * 0.0018).exp()).clamp(20.0, 200_000.0);
        ctx.request_repaint();
    }
}

fn begin(
    app: &mut EditorApp,
    view3d: &View3d,
    object: Object,
    axes: [V3; 3],
    grip: Grip,
    press: Pos2,
) {
    let kind = app
        .session
        .tools3d
        .tool
        .and_then(kind_of)
        .unwrap_or(Kind::Move);
    let grab = match (kind, grip) {
        (Kind::Move, Grip::Gizmo(Handle::Axis(k))) => {
            axis_point(view3d, object.centre, axes[k], press)
        }
        (Kind::Move, Grip::Gizmo(Handle::Plane(k))) => {
            gizmo::plane_point(view3d, object.centre, gizmo::plane_normal(axes, k), press)
        }
        (Kind::Move, Grip::Gizmo(Handle::Free)) => gizmo::plane_point(
            view3d,
            object.centre,
            view3d.toward_eye(object.centre),
            press,
        ),
        _ => Some(object.centre),
    };
    let Some(grab) = grab else { return };
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let reference = object.reference;
    // Turning and sizing a drawing happen about its middle: make that its
    // pivot, so the stage and every renderer turn it there too.
    if !object.model {
        if let Some(placement) = placement_mut(app, reference) {
            if placement.pivot.is_none() {
                placement.pivot = Some(object.pivot);
            }
        }
    }
    app.session.tools3d.drag = Some(Drag {
        kind,
        grip,
        reference,
        start: Pose {
            transform: object.transform,
            space: object.space,
        },
        pivot: object.pivot,
        centre: object.centre,
        axes,
        grab,
        press,
    });
}

/// The point on the line through `centre` along `axis` nearest the ray
/// under `at`.
fn axis_point(view3d: &View3d, centre: V3, axis: V3, at: Pos2) -> Option<V3> {
    let (origin, direction) = view3d.ray(at.x, at.y)?;
    let distance = gizmo::axis_parameter(origin, direction, centre, axis)?;
    Some(centre + axis * distance)
}

fn snap(value: f32, step: f32, on: bool) -> f32 {
    if on && step > 0.0 {
        (value / step).round() * step
    } else {
        value
    }
}

/// Follows the pointer with whatever the drag holds.
fn drag_to(app: &mut EditorApp, view3d: &View3d, at: Pos2, snapping: bool) {
    let Some(drag) = app.session.tools3d.drag.clone() else {
        return;
    };
    let mut pose = drag.start;
    let readout = match drag.grip {
        Grip::Gizmo(Handle::Axis(k)) => {
            let Some(point) = axis_point(view3d, drag.centre, drag.axes[k], at) else {
                return;
            };
            let amount = snap((point - drag.grab).dot(drag.axes[k]), 10.0, snapping);
            move_by(&mut pose, drag.axes[k] * amount);
            format!("{} {amount:+.0}", ["x", "y", "z"][k])
        }
        Grip::Gizmo(Handle::Plane(k)) if drag.kind == Kind::Move => {
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let (u, v) = (drag.axes[a], drag.axes[b]);
            let normal = u.cross(v);
            let Some(point) = gizmo::plane_point(view3d, drag.centre, normal.normalized(), at)
            else {
                return;
            };
            let delta = point - drag.grab;
            let da = snap(
                delta.cross(v).dot(normal) / normal.length_sq(),
                10.0,
                snapping,
            );
            let db = snap(
                u.cross(delta).dot(normal) / normal.length_sq(),
                10.0,
                snapping,
            );
            move_by(&mut pose, u * da + v * db);
            format!(
                "{} {da:+.0}  {} {db:+.0}",
                ["x", "y", "z"][a],
                ["x", "y", "z"][b]
            )
        }
        Grip::Gizmo(Handle::Free) => {
            let Some(point) =
                gizmo::plane_point(view3d, drag.centre, view3d.toward_eye(drag.centre), at)
            else {
                return;
            };
            let delta = point - drag.grab;
            move_by(&mut pose, delta);
            format!("x {:+.0}  y {:+.0}  z {:+.0}", delta.x, -delta.y, delta.z)
        }
        Grip::Depth => {
            let amount = snap(
                gizmo::world_length(view3d, drag.centre, (drag.press.y - at.y) * 2.0),
                10.0,
                snapping,
            );
            move_by(&mut pose, V3::Z * amount);
            format!("depth {amount:+.0}")
        }
        Grip::Gizmo(Handle::Ring(k)) => {
            let Some(degrees) = gizmo::ring_turn(view3d, drag.centre, drag.axes[k], drag.press, at)
            else {
                return;
            };
            let degrees = snap(degrees, 15.0, snapping);
            turn(&mut pose, drag.pivot, drag.axes[k], degrees);
            format!("turn {degrees:+.0}°")
        }
        Grip::Gizmo(Handle::ViewRing) => {
            let Some(centre) = to_screen(view3d, drag.centre) else {
                return;
            };
            let forward = -view3d.toward_eye(drag.centre);
            let degrees = snap(gizmo::screen_turn(centre, drag.press, at), 15.0, snapping);
            turn(&mut pose, drag.pivot, forward, degrees);
            format!("turn {degrees:+.0}°")
        }
        Grip::Gizmo(Handle::ScaleAxis(k)) => {
            let Some(factor) = gizmo::axis_scale(view3d, drag.centre, drag.axes[k], drag.press, at)
            else {
                return;
            };
            let factor = snap(factor, 0.1, snapping).max(0.01);
            size_by(
                &mut pose,
                drag.pivot,
                if k == 0 { [factor, 1.0] } else { [1.0, factor] },
            );
            format!("{} {:.0}%", ["width", "height"][k.min(1)], factor * 100.0)
        }
        Grip::Gizmo(handle @ (Handle::ScaleAll | Handle::Plane(_))) => {
            let factor = if let Handle::Plane(k) = handle {
                let normal = gizmo::plane_normal(drag.axes, k);
                let Some(factor) = gizmo::plane_scale(view3d, drag.centre, normal, drag.press, at)
                else {
                    return;
                };
                factor
            } else {
                gizmo::uniform_scale(drag.press, at)
            };
            let factor = snap(factor, 0.1, snapping).max(0.01);
            size_by(&mut pose, drag.pivot, [factor, factor]);
            format!("size {:.0}%", factor * 100.0)
        }
        Grip::Gizmo(Handle::Extrude) => return,
    };
    write(app, drag.reference, pose);
    app.session.status = readout;
}

fn move_by(pose: &mut Pose, delta: V3) {
    pose.transform.tx += delta.x;
    pose.transform.ty += delta.y;
    pose.space.z += delta.z;
}

/// The placement's turn as one rotation: its 2D angle, then its lean back,
/// then its turn.
fn orientation(pose: &Pose) -> Quat {
    Quat::from_axis_angle(V3::Z, pose.transform.rotation)
        * Quat::from_axis_angle(V3::X, pose.space.tilt_x.to_radians())
        * Quat::from_axis_angle(V3::Y, pose.space.tilt_y.to_radians())
}

/// Splits a rotation back into the 2D angle (radians), lean back and turn
/// (degrees).
pub(crate) fn split_turn(q: Quat) -> (f32, f32, f32) {
    let m = Mat4::from_quat(q);
    let r = |row: usize, col: usize| m.at(row, col);
    let lean = r(2, 1).clamp(-1.0, 1.0).asin();
    if lean.cos().abs() > 1.0e-4 {
        let turn = (-r(2, 0)).atan2(r(2, 2));
        let angle = (-r(0, 1)).atan2(r(1, 1));
        (angle, lean.to_degrees(), turn.to_degrees())
    } else {
        // Straight up or down: the angle and the turn do the same thing.
        let angle = r(1, 0).atan2(r(0, 0));
        (angle, lean.to_degrees(), 0.0)
    }
}

fn turn(pose: &mut Pose, pivot: StageVec, axis: V3, degrees: f32) {
    let before = Affine::from_transform(pose.transform).apply(pivot);
    let q = Quat::from_axis_angle(axis.normalized(), degrees.to_radians()) * orientation(pose);
    let (angle, lean, turn) = split_turn(q);
    pose.transform.rotation = angle;
    pose.space.tilt_x = lean;
    pose.space.tilt_y = turn;
    keep_pivot(pose, pivot, before);
}

fn size_by(pose: &mut Pose, pivot: StageVec, factor: [f32; 2]) {
    let before = Affine::from_transform(pose.transform).apply(pivot);
    pose.transform.sx *= factor[0];
    pose.transform.sy *= factor[1];
    keep_pivot(pose, pivot, before);
}

/// Slides the placement so its pivot lands back where it was.
fn keep_pivot(pose: &mut Pose, pivot: StageVec, before: StageVec) {
    let after = Affine::from_transform(pose.transform).apply(pivot);
    pose.transform.tx += before.x - after.x;
    pose.transform.ty += before.y - after.y;
}

fn placement_mut(
    app: &mut EditorApp,
    reference: PlacementRef,
) -> Option<&mut q0s_format::v2::Placement> {
    app.state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get_mut(reference.placement_idx)
}

/// Puts a pose on the placement. Depth and turning place a drawing in 3D.
fn write(app: &mut EditorApp, reference: PlacementRef, pose: Pose) {
    let flat = pose.space.z == 0.0 && pose.space.tilt_x == 0.0 && pose.space.tilt_y == 0.0;
    crate::tools::set_placement_transform_for_edit(
        app,
        reference.q0rg_id,
        reference.layer_id,
        reference.placement_idx,
        pose.transform,
    );
    if let Some(placement) = placement_mut(app, reference) {
        if placement.fx.three.is_some() || !flat {
            placement.fx.three = Some(pose.space);
        }
    }
    app.mark_placement_transforms_changed(&[(
        reference.q0rg_id,
        reference.layer_id,
        reference.placement_idx,
    )]);
    app.state.mark_render_dirty();
}

/// The ground and the scene camera, drawn under the stage in the free view.
pub(crate) fn paint_ground(app: &EditorApp, painter: &Painter, view: &StageView) {
    let Some(orbit) = app.session.tools3d.orbit else {
        return;
    };
    let view3d = stage_view3d(app, view);
    let project = &app.state.project;
    let (w, h) = (
        f32::from(project.meta.stage_width),
        f32::from(project.meta.stage_height),
    );
    let step = 10f32.powf((orbit.distance / 3.0).max(10.0).log10().floor());
    let reach = (orbit.distance * 2.5).max(w.max(h) * 2.0);
    let middle = V3::new(
        (orbit.target.x / step).round() * step,
        h,
        (orbit.target.z / step).round() * step,
    );
    let lines = (reach / step).ceil() as i32;
    let line = |a: V3, b: V3, colour: Color32| {
        // In pieces, so a line passing behind the eye still shows in front.
        const PIECES: usize = 12;
        for k in 0..PIECES {
            let p = a.lerp(b, k as f32 / PIECES as f32);
            let q = a.lerp(b, (k + 1) as f32 / PIECES as f32);
            let fade = 1.0 - ((p + q) * 0.5 - middle).length() / reach;
            if fade <= 0.0 {
                continue;
            }
            if let (Some(p), Some(q)) = (to_screen(&view3d, p), to_screen(&view3d, q)) {
                painter.line_segment(
                    [p, q],
                    Stroke::new(1.0_f32, colour.gamma_multiply(fade.min(1.0))),
                );
            }
        }
    };
    let faint = Color32::from_white_alpha(28);
    let major = Color32::from_white_alpha(60);
    for i in -lines..=lines {
        let offset = i as f32 * step;
        let colour = if (middle.x + offset).rem_euclid(step * 10.0) < 0.5 {
            major
        } else {
            faint
        };
        line(
            V3::new(middle.x + offset, h, middle.z - reach),
            V3::new(middle.x + offset, h, middle.z + reach),
            colour,
        );
        let colour = if (middle.z + offset).rem_euclid(step * 10.0) < 0.5 {
            major
        } else {
            faint
        };
        line(
            V3::new(middle.x - reach, h, middle.z + offset),
            V3::new(middle.x + reach, h, middle.z + offset),
            colour,
        );
    }
}

/// The 3D tools' marks over the drawn stage: the scene camera and the
/// paper's edge in the free view, boxes round things, the gizmo.
pub(crate) fn draw_overlay(
    app: &EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
) {
    let view3d = stage_view3d(app, view);
    let accent = app.settings.theme.accent.to_color32();
    if app.session.tools3d.orbit.is_some() {
        paint_scene_camera(app, painter, &view3d);
    }
    crate::scene3d_stage::paint_stage_overlay(app, painter, view);
    let Some(tool) = app.session.tools3d.tool else {
        return;
    };
    // The thing under the pointer, when it is not the picked one.
    if app.session.tools3d.drag.is_none() && app.session.tools3d.hover.is_none() {
        if let Some(at) = response.hover_pos() {
            if let Some(reference) = pick(app, &view3d, at).filter(|r| Some(*r) != selected(app)) {
                if let Some(rect) = object(app, reference).and_then(|o| {
                    let target = target_of(app, reference)?;
                    screen_box(&app.state.project, &o, &view3d, target)
                }) {
                    painter.rect_stroke(
                        rect.expand(2.0),
                        3.0,
                        Stroke::new(1.0_f32, accent.gamma_multiply(0.6)),
                    );
                }
            }
        }
    }
    let Some(reference) = selected(app) else {
        if response.hovered() {
            hint(painter, response.rect, tool.hint());
        }
        return;
    };
    let Some(object) = object(app, reference) else {
        return;
    };
    if let Some(rect) = target_of(app, reference)
        .and_then(|target| screen_box(&app.state.project, &object, &view3d, target))
    {
        dashed_box(painter, rect.expand(3.0), accent);
    }
    if let Some(kind) = kind_of(tool) {
        let axes = axes_of(&object, app.session.tools3d.own_axes || kind == Kind::Scale);
        if let Some(g) = layout(&view3d, &object, kind, axes) {
            let held = app.session.tools3d.drag.as_ref().map(|drag| drag.grip);
            let as_handle = |grip: Option<Grip>| match grip {
                Some(Grip::Gizmo(handle)) => Some(handle),
                _ => None,
            };
            gizmo::paint(
                painter,
                &g,
                as_handle(app.session.tools3d.hover),
                as_handle(held),
            );
            if let Some(knob) = depth_knob(&g) {
                let hot = matches!(app.session.tools3d.hover, Some(Grip::Depth))
                    || matches!(held, Some(Grip::Depth));
                let colour = if hot {
                    Color32::from_rgb(255, 222, 80)
                } else {
                    AXIS_COLOURS[2]
                };
                painter.line_segment(
                    [g.centre, knob],
                    Stroke::new(1.5_f32, colour.gamma_multiply(0.7)),
                );
                painter.circle(
                    knob,
                    10.0,
                    colour,
                    Stroke::new(1.5_f32, Color32::from_black_alpha(160)),
                );
                for dir in [-1.0, 1.0] {
                    let tip = knob + vec2(0.0, dir * 4.5);
                    painter.add(Shape::convex_polygon(
                        vec![
                            tip + vec2(0.0, dir * 3.5),
                            tip + vec2(-3.5, 0.0),
                            tip + vec2(3.5, 0.0),
                        ],
                        Color32::WHITE,
                        Stroke::NONE,
                    ));
                }
                painter.text(
                    knob + vec2(16.0, 0.0),
                    Align2::LEFT_CENTER,
                    "depth",
                    FontId::proportional(11.0),
                    Color32::from_white_alpha(200),
                );
            }
        }
    }
    if app.session.tools3d.drag.is_some() && !app.session.status.is_empty() {
        if let Some(at) = response
            .hover_pos()
            .or_else(|| response.interact_pointer_pos())
        {
            let galley = painter.layout_no_wrap(
                app.session.status.clone(),
                FontId::monospace(12.0),
                Color32::WHITE,
            );
            let rect = Rect::from_min_size(at + vec2(18.0, 14.0), galley.size() + vec2(10.0, 6.0));
            painter.rect_filled(rect, 4.0, Color32::from_black_alpha(190));
            painter.galley(rect.min + vec2(5.0, 3.0), galley, Color32::WHITE);
        }
    }
}

fn target_of(app: &EditorApp, reference: PlacementRef) -> Option<Target> {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)
        .map(|placement| placement.target)
}

fn dashed_box(painter: &Painter, rect: Rect, colour: Color32) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.extend(Shape::dashed_line(
        &corners,
        Stroke::new(1.5_f32, colour),
        6.0,
        4.0,
    ));
}

fn hint(painter: &Painter, area: Rect, text: &str) {
    let galley = painter.layout(
        text.to_owned(),
        FontId::proportional(12.0),
        Color32::from_white_alpha(220),
        (area.width() - 40.0).max(120.0),
    );
    let rect = Rect::from_center_size(
        pos2(
            area.center().x,
            area.bottom() - 24.0 - galley.size().y * 0.5,
        ),
        galley.size() + vec2(20.0, 10.0),
    );
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha(150));
    painter.galley(rect.min + vec2(10.0, 5.0), galley, Color32::WHITE);
}

/// The scene camera as the free view sees it: where it stands and the
/// part of the stage it frames.
fn paint_scene_camera(app: &EditorApp, painter: &Painter, view3d: &View3d) {
    let project = &app.state.project;
    let look = frame::scene_look(
        project,
        app.session.current_q0rg_id,
        app.session.current_frame,
        |_| None,
    );
    let camera = look.camera;
    let (w, h) = (
        f32::from(project.meta.stage_width),
        f32::from(project.meta.stage_height),
    );
    let offset = camera.target - camera.position;
    let distance = offset.length().max(1.0);
    let forward = offset / distance;
    let mut right = V3::Y.cross(forward).normalized();
    if right.length_sq() < 0.5 {
        right = V3::X;
    }
    let down = forward.cross(right).normalized();
    let half = (
        w * 0.5 * distance / camera.zoom,
        h * 0.5 * distance / camera.zoom,
    );
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .map(|(x, y)| camera.target + right * (x * half.0) + down * (y * half.1));
    let colour = Color32::from_rgb(120, 200, 255);
    let eye = to_screen(view3d, camera.position);
    let screen: Vec<Option<Pos2>> = corners.iter().map(|c| to_screen(view3d, *c)).collect();
    for k in 0..4 {
        if let (Some(a), Some(b)) = (screen[k], screen[(k + 1) % 4]) {
            painter.line_segment([a, b], Stroke::new(1.5_f32, colour));
        }
        if let (Some(e), Some(c)) = (eye, screen[k]) {
            painter.line_segment([e, c], Stroke::new(1.0_f32, colour.gamma_multiply(0.55)));
        }
    }
    if let Some(e) = eye {
        painter.circle(
            e,
            6.0,
            colour,
            Stroke::new(1.5_f32, Color32::from_black_alpha(180)),
        );
        painter.text(
            e + vec2(10.0, -10.0),
            Align2::LEFT_BOTTOM,
            "camera",
            FontId::proportional(11.0),
            colour,
        );
    }
}

/// A set way to look: its name, what it shows, and the turn and tilt of the
/// free camera (none: the scene camera).
type QuickView = (&'static str, &'static str, Option<(f32, f32)>);

/// The 3D bar over the stage's top-left corner, whenever the project has
/// 3D: the four tools, and with one of them on, where to look from.
pub(crate) fn view_bar(app: &mut EditorApp, ui: &mut Ui, canvas: Rect) {
    if !offered(&app.state.project) {
        return;
    }
    let accent = app.settings.theme.accent.to_color32();
    let rect = Rect::from_min_size(
        canvas.min + vec2(10.0, 10.0),
        vec2((canvas.width() - 20.0).max(60.0), 30.0),
    );
    let mut child = ui.child_ui(rect, egui::Layout::left_to_right(egui::Align::Center));
    egui::Frame::none()
        .fill(Color32::from_rgba_unmultiplied(18, 18, 22, 190))
        .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(18)))
        .rounding(9.0)
        .inner_margin(egui::Margin::symmetric(6.0, 3.0))
        .show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(
                    egui::RichText::new("3D")
                        .small()
                        .strong()
                        .color(Color32::from_white_alpha(150)),
                );
                for tool in Tool3d::ALL {
                    let on = app.session.tools3d.tool == Some(tool);
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
                    if on {
                        ui.painter().rect(
                            rect,
                            6.0,
                            accent.gamma_multiply(0.35),
                            Stroke::new(1.0_f32, accent),
                        );
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, 6.0, Color32::from_white_alpha(22));
                    }
                    let colour = if on {
                        Color32::WHITE
                    } else {
                        Color32::from_white_alpha(200)
                    };
                    paint_icon(ui.painter(), rect.shrink(4.0), tool, colour, accent);
                    if response.on_hover_text(tool.hint()).clicked()
                        && crate::text_tool::finish_edit(app)
                    {
                        if on {
                            app.session.tools3d.leave();
                        } else {
                            app.plugins.active_tool = None;
                            app.session.tool_state = crate::state::ToolState::Idle;
                            app.session.tools3d.tool = Some(tool);
                        }
                    }
                }
                if app.session.tools3d.tool.is_none() {
                    return;
                }
                ui.add_space(2.0);
                ui.separator();
                let zoom = lens(&app.state.project);
                let project = &app.state.project;
                let look = frame::scene_look(
                    project,
                    app.session.current_q0rg_id,
                    app.session.current_frame,
                    |_| None,
                );
                let home = Orbit::from_camera(&look.camera);
                let views: [QuickView; 5] = [
                    ("camera", "Look through the scene camera", None),
                    ("front", "From the front", Some((0.0, 0.0))),
                    ("top", "From above", Some((0.0, 1.5))),
                    (
                        "side",
                        "From the left",
                        Some((std::f32::consts::FRAC_PI_2, 0.0)),
                    ),
                    ("3/4", "From above and to the side", Some((0.7, 0.45))),
                ];
                for (label, tip, angles) in views {
                    let current = match (angles, app.session.tools3d.orbit) {
                        (None, None) => true,
                        (Some((yaw, pitch)), Some(orbit)) => {
                            (orbit.yaw - yaw).abs() < 1.0e-3 && (orbit.pitch - pitch).abs() < 1.0e-3
                        }
                        _ => false,
                    };
                    let text = egui::RichText::new(label).small().color(if current {
                        Color32::WHITE
                    } else {
                        Color32::from_white_alpha(165)
                    });
                    if ui
                        .selectable_label(current, text)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        app.session.tools3d.orbit = angles.map(|(yaw, pitch)| Orbit {
                            target: home.target,
                            yaw,
                            pitch,
                            distance: home.distance.max(zoom * 0.5),
                        });
                    }
                }
                ui.separator();
                let mut own = app.session.tools3d.own_axes;
                if ui
                    .checkbox(
                        &mut own,
                        egui::RichText::new("own axes")
                            .small()
                            .color(Color32::from_white_alpha(190)),
                    )
                    .on_hover_text(
                        "Move and turn along the object's own axes instead of the stage's",
                    )
                    .changed()
                {
                    app.session.tools3d.own_axes = own;
                }
            });
        });
}

/// Paints a 3D tool's icon.
pub(crate) fn paint_icon(
    painter: &Painter,
    rect: Rect,
    tool: Tool3d,
    colour: Color32,
    accent: Color32,
) {
    let c = rect.center();
    let r = rect.width() * 0.5;
    let stroke = Stroke::new(1.6_f32, colour);
    match tool {
        Tool3d::Move => {
            // Three arrows from one point: the move gizmo in small.
            let ends = [vec2(0.95, 0.35), vec2(0.0, -1.0), vec2(-0.8, 0.55)];
            for (k, end) in ends.iter().enumerate() {
                let tip = c + *end * r * 0.85;
                let tint = if colour == accent {
                    accent
                } else {
                    AXIS_COLOURS[k].gamma_multiply(0.95)
                };
                painter.line_segment([c, tip], Stroke::new(1.8_f32, tint));
                let n = end.normalized();
                let side = vec2(-n.y, n.x);
                painter.add(Shape::convex_polygon(
                    vec![tip + n * 3.5, tip + side * 2.8, tip - side * 2.8],
                    tint,
                    Stroke::NONE,
                ));
            }
            painter.circle_filled(c, 2.2, colour);
        }
        Tool3d::Rotate => {
            painter.circle_stroke(c, r * 0.78, stroke);
            painter.add(Shape::ellipse_stroke(
                c,
                vec2(r * 0.78, r * 0.3),
                Stroke::new(1.4_f32, colour.gamma_multiply(0.75)),
            ));
            let tip = c + vec2(r * 0.55, -r * 0.55);
            painter.add(Shape::convex_polygon(
                vec![
                    tip + vec2(3.5, -1.0),
                    tip + vec2(-1.0, -3.5),
                    tip + vec2(0.5, 2.5),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        Tool3d::Scale => {
            // A box with its near corner pulled out.
            let s = r * 0.5;
            let back =
                Rect::from_center_size(c + vec2(-s * 0.35, -s * 0.35), vec2(s * 1.4, s * 1.4));
            let front = back.translate(vec2(s * 0.6, s * 0.6));
            painter.rect_stroke(back, 1.0, Stroke::new(1.2_f32, colour.gamma_multiply(0.6)));
            painter.rect_stroke(front, 1.0, stroke);
            for (a, b) in [
                (back.left_top(), front.left_top()),
                (back.right_top(), front.right_top()),
                (back.left_bottom(), front.left_bottom()),
                (back.right_bottom(), front.right_bottom()),
            ] {
                painter.line_segment([a, b], Stroke::new(1.0_f32, colour.gamma_multiply(0.6)));
            }
            painter.rect_filled(
                Rect::from_center_size(front.right_bottom(), vec2(4.5, 4.5)),
                1.0,
                colour,
            );
        }
        Tool3d::View => {
            // An eye with an orbit round it.
            painter.add(Shape::ellipse_stroke(
                c,
                vec2(r * 0.9, r * 0.38),
                Stroke::new(1.3_f32, colour.gamma_multiply(0.7)),
            ));
            painter.circle_stroke(c, r * 0.42, stroke);
            painter.circle_filled(c, r * 0.18, colour);
            let tip = c + vec2(r * 0.9, 0.0);
            painter.add(Shape::convex_polygon(
                vec![
                    tip + vec2(0.0, -3.5),
                    tip + vec2(3.0, 1.5),
                    tip + vec2(-3.0, 1.5),
                ],
                colour,
                Stroke::NONE,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.0e-3
    }

    #[test]
    fn a_turn_splits_back_into_angle_lean_and_turn() {
        for (angle, lean, turn) in [(0.4, 20.0, -35.0), (-1.2, -50.0, 80.0), (2.5, 10.0, 0.0)] {
            let pose = Pose {
                transform: Transform2D {
                    rotation: angle,
                    ..Transform2D::IDENTITY
                },
                space: Space3d {
                    tilt_x: lean,
                    tilt_y: turn,
                    ..Space3d::default()
                },
            };
            let (a, l, t) = split_turn(orientation(&pose));
            assert!(
                close(a, angle) && close(l, lean) && close(t, turn),
                "{a} {l} {t}"
            );
        }
    }

    #[test]
    fn turning_keeps_the_pivot_in_place() {
        let mut pose = Pose {
            transform: Transform2D {
                tx: 100.0,
                ty: 50.0,
                ..Transform2D::IDENTITY
            },
            space: Space3d::default(),
        };
        let pivot = StageVec::new(40.0, 20.0);
        let before = Affine::from_transform(pose.transform).apply(pivot);
        turn(&mut pose, pivot, V3::Z, 90.0);
        let after = Affine::from_transform(pose.transform).apply(pivot);
        assert!(close(before.x, after.x) && close(before.y, after.y));
        assert!(close(pose.transform.rotation, std::f32::consts::FRAC_PI_2));
        // Turning about the vertical axis turns the card, not its 2D angle.
        let mut pose = Pose {
            transform: Transform2D::IDENTITY,
            space: Space3d::default(),
        };
        turn(&mut pose, pivot, V3::Y, 30.0);
        assert!(close(pose.space.tilt_y, 30.0) && close(pose.transform.rotation, 0.0));
    }

    #[test]
    fn the_free_camera_starts_where_the_scene_camera_looks() {
        let camera = CameraPose::stage(640, 480);
        let orbit = Orbit::from_camera(&camera);
        let again = orbit.camera(camera.zoom);
        assert!((again.position - camera.position).length() < 1.0e-2);
        assert!((again.target - camera.target).length() < 1.0e-2);
    }

    #[test]
    fn dragging_an_axis_follows_the_line_under_the_pointer() {
        let camera = CameraPose::stage(640, 480);
        let view3d = View3d::new(camera, false, [640.0, 480.0], Affine::IDENTITY, [640, 480]);
        let centre = V3::new(320.0, 240.0, 0.0);
        let [x, y, _] = view3d.project(centre + V3::X * 50.0).unwrap();
        let point = axis_point(&view3d, centre, V3::X, pos2(x, y)).unwrap();
        assert!(
            (point - (centre + V3::X * 50.0)).length() < 1.0e-2,
            "{point:?}"
        );
    }

    #[test]
    fn stage_gizmos_follow_an_oblique_perspective_camera() {
        use crate::app::Action;

        let ctx = egui::Context::default();
        let mut app = EditorApp::default();
        app.handle(&ctx, Action::CreateProject(Default::default()));
        app.handle(&ctx, Action::NewModel);
        let reference = selected(&app).expect("new model is selected on stage");
        app.session.tools3d.orbit = Some(Orbit {
            target: object(&app, reference).unwrap().centre + V3::new(-150.0, 50.0, 0.0),
            yaw: 0.8,
            pitch: 0.35,
            distance: lens(&app.state.project) * 0.7,
        });
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 740.0));
        let size = vec2(
            f32::from(app.state.project.meta.stage_width),
            f32::from(app.state.project.meta.stage_height),
        );
        let scale = (screen.width() / size.x).min(screen.height() / size.y) * 0.8;
        let stage_rect = Rect::from_center_size(screen.center(), size * scale);
        let view = StageView {
            origin: stage_rect.min,
            scale,
            stage_rect,
            edit_transform: Affine::IDENTITY,
        };
        let view3d = stage_view3d(&app, &view);
        let render = |app: &mut EditorApp, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui
                            .allocate_response(ui.available_size(), egui::Sense::click_and_drag());
                        handle(app, &response, &view);
                    });
                },
            );
        };
        let drag = |app: &mut EditorApp, press, at| {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(
                app,
                vec![egui::Event::PointerMoved(press), button(press, true)],
            );
            render(app, vec![egui::Event::PointerMoved(at)]);
            render(app, vec![button(at, false)]);
        };

        app.session.tools3d.tool = Some(Tool3d::Rotate);
        let model = object(&app, reference).unwrap();
        let axes = axes_of(&model, false);
        let g = layout(&view3d, &model, Kind::Rotate, axes).unwrap();
        let press = g.rings[0]
            .iter()
            .map(|(p, _)| *p)
            .find(|p| grip_at(&g, *p) == Some(Grip::Gizmo(Handle::Ring(0))))
            .unwrap();
        let start =
            gizmo::plane_point(&view3d, model.centre, axes[0], press).unwrap() - model.centre;
        let turn = Quat::from_axis_angle(axes[0], 60.0f32.to_radians());
        let at = to_screen(&view3d, model.centre + turn.rotate(start)).unwrap();
        render(&mut app, vec![]);
        drag(&mut app, press, at);
        let rotated = object(&app, reference).unwrap();
        assert!(
            close(rotated.space.tilt_x, 60.0),
            "{}",
            rotated.space.tilt_x
        );
        assert!((rotated.centre - model.centre).length() < 1.0e-3);

        app.handle(&ctx, Action::Undo);
        app.session.tools3d.tool = Some(Tool3d::Scale);
        let model = object(&app, reference).unwrap();
        let axes = axes_of(&model, true);
        let g = layout(&view3d, &model, Kind::Scale, axes).unwrap();
        let press = g.tips[0].unwrap();
        assert_eq!(grip_at(&g, press), Some(Grip::Gizmo(Handle::ScaleAxis(0))));
        let length = gizmo::world_length(&view3d, model.centre, ARROW);
        let at = to_screen(&view3d, model.centre + axes[0] * (2.0 * length)).unwrap();
        drag(&mut app, press, at);
        let scaled = object(&app, reference).unwrap();
        assert!(
            close(scaled.transform.sx, 2.0 * model.transform.sx),
            "{}",
            scaled.transform.sx
        );
        assert!(close(scaled.transform.sy, model.transform.sy));

        let g = layout(&view3d, &scaled, Kind::Scale, axes).unwrap();
        let press = g.plane_centres[2].unwrap();
        assert_eq!(grip_at(&g, press), Some(Grip::Gizmo(Handle::Plane(2))));
        let offset = (axes[0] + axes[1]) * (length * 0.32);
        assert_eq!(Some(press), to_screen(&view3d, scaled.centre + offset));
        let at = to_screen(&view3d, scaled.centre + offset * 1.5).unwrap();
        drag(&mut app, press, at);
        let plane_scaled = object(&app, reference).unwrap();
        assert!(close(plane_scaled.transform.sx, scaled.transform.sx * 1.5));
        assert!(close(plane_scaled.transform.sy, scaled.transform.sy * 1.5));

        let centre = to_screen(&view3d, scaled.centre).unwrap();
        drag(&mut app, centre, centre - vec2(60.0, 0.0));
        let uniformly_scaled = object(&app, reference).unwrap().transform;
        assert!(close(uniformly_scaled.sx, plane_scaled.transform.sx * 0.5));
        assert!(close(uniformly_scaled.sy, plane_scaled.transform.sy * 0.5));
    }

    #[test]
    fn edge_on_ring_turns_along_its_projected_tangent() {
        let camera = CameraPose::stage(640, 480);
        for orthographic in [false, true] {
            let view = View3d::new(
                camera,
                orthographic,
                [640.0, 480.0],
                Affine::IDENTITY,
                [640, 480],
            );
            let pivot = V3::new(320.0, 240.0, 0.0);
            let press = pos2(320.0, 280.0);
            let angle =
                |dy| gizmo::ring_turn(&view, pivot, V3::X, press, press + vec2(0.0, dy)).unwrap();
            assert!(angle(35.0) > 20.0 && angle(35.0) < 40.0);
            assert!(close(angle(-35.0), -angle(35.0)));
        }
    }
}
