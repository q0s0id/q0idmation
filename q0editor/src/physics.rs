//! The physics editor: objects on a timeline fall, bounce and collide in a
//! rapier2d world, and the result is baked into ordinary keyframes.
//!
//! It is an opt-in tool. While `Settings::physics_enabled` is off nothing of
//! it shows: no menu entry, no window, no stage overlay.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Color32, Context, Painter, RichText, Stroke, Ui};
use q0s_format::physics::{
    BodyKind, ColliderShape, Momentum, PhysicsBody, PhysicsScene, MAX_COLLISION_GROUP, WALL_BOTTOM,
    WALL_LEFT, WALL_RIGHT, WALL_TOP,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, ProjectV2, Target, Transform2D, Tween, Vec2};
use rapier2d::prelude::{
    ColliderBuilder, Group, InteractionGroups, InteractionTestMode, PhysicsWorld, Pose,
    RigidBodyBuilder, RigidBodyHandle, Vector,
};

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{RigMode, Selection, TimelineSelection};
use crate::ui::FitToScreen;

/// Stage pixels in one rapier length unit, so its tolerances suit objects a
/// few dozen to a few hundred pixels across.
const PIXELS_PER_UNIT: f32 = 100.0;
/// Colliders never shrink below this, so lines and dots still collide.
const MIN_HALF_EXTENT: f32 = 0.5;
/// Longest stretch one bake simulates.
const MAX_SIMULATED_FRAMES: u32 = 20_000;

/// Window and preview state. Lives in the session, never in the project.
#[derive(Default)]
pub struct PhysicsSession {
    pub open: bool,
    pub mode: RigMode,
    pub selected_body: Option<u32>,
    /// Shows the simulation on the stage with the real artwork, kept up to
    /// date while the setup changes. Nothing is keyed until a bake.
    pub previewing: bool,
    preview: Option<Preview>,
    preview_serial: u64,
    /// Bake only the selected body; the others still bump into it.
    pub only_selected: bool,
    /// A key every this many frames, tweened in between; 0 and 1 key every
    /// frame.
    pub key_step: u16,
    /// The last result, shown in the window.
    message: String,
}

impl PhysicsSession {
    pub fn bake_options(&self) -> BakeOptions {
        BakeOptions {
            only: self.selected_body.filter(|_| self.only_selected),
            key_step: self.key_step.max(1),
        }
    }

    pub fn stop_preview(&mut self) {
        self.previewing = false;
        self.preview = None;
    }
}

/// A disposable copy of the project with the simulation baked in, drawn on
/// the stage in place of the real one.
struct Preview {
    simulation: Arc<Simulation>,
    project: Arc<ProjectV2>,
    /// Render revision of the copy; never one the real project reaches.
    revision: u64,
    /// Revision of the real project it was made from.
    source_revision: u64,
    options: BakeOptions,
    q0rg_id: u16,
}

/// What a bake keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeOptions {
    /// Key only this body; the others still push it around.
    pub only: Option<u32>,
    /// A key every this many frames, with motion tweens in between.
    pub key_step: u16,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            only: None,
            key_step: 1,
        }
    }
}

/// Where a collider sits and how far it is turned on one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyPose {
    pub center: [f32; 2],
    pub angle: f32,
}

/// An object found on the timeline, with its collider fitted to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyFit {
    pub layer_id: u16,
    pub transform: Transform2D,
    pub pose: BodyPose,
    /// Half the width and height of its bounds as they show on the stage.
    pub half_extents: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct Track {
    pub instance_id: u32,
    pub kind: BodyKind,
    pub shape: ColliderShape,
    /// Collider size, shape scale included.
    pub half_extents: [f32; 2],
    pub start: BodyFit,
    /// One pose per frame; the first is the start frame.
    pub poses: Vec<BodyPose>,
}

#[derive(Debug, Clone)]
pub struct Simulation {
    pub q0rg_id: u16,
    pub start_frame: u16,
    pub tracks: Vec<Track>,
    /// Bodies whose object is not on the start frame.
    pub missing: usize,
}

impl Simulation {
    pub fn pose_at(&self, track: &Track, frame: u16) -> Option<BodyPose> {
        let offset = frame.checked_sub(self.start_frame)?;
        track.poses.get(usize::from(offset)).copied()
    }

    pub fn last_frame(&self) -> u16 {
        let frames = self
            .tracks
            .iter()
            .map(|track| track.poses.len())
            .max()
            .unwrap_or(1);
        self.start_frame
            .saturating_add(u16::try_from(frames.saturating_sub(1)).unwrap_or(u16::MAX))
    }
}

/// Finds the object `instance_id` on `frame` and fits a collider to it.
pub fn fit_body(
    project: &ProjectV2,
    q0rg_id: u16,
    instance_id: u32,
    frame: u16,
) -> Option<BodyFit> {
    if instance_id == 0 {
        return None;
    }
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    for layer in &q0rg.layers {
        for state in q0s_format::raster::active_placement_states_at(layer, frame) {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            if placement.instance_id != instance_id {
                continue;
            }
            let (min_x, min_y, max_x, max_y) =
                crate::render::placement_local_bbox(project, placement)
                    .filter(|(min_x, min_y, max_x, max_y)| {
                        [min_x, min_y, max_x, max_y].iter().all(|v| v.is_finite())
                            && max_x >= min_x
                            && max_y >= min_y
                    })
                    .unwrap_or((-5.0, -5.0, 5.0, 5.0));
            let affine = Affine::from_transform(state.transform);
            let center = affine.apply(Vec2::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5));
            let x_axis = affine.a11.hypot(affine.a21);
            let y_axis = affine.a12.hypot(affine.a22);
            return Some(BodyFit {
                layer_id: layer.layer_id,
                transform: state.transform,
                pose: BodyPose {
                    center: [center.x, center.y],
                    angle: affine.a21.atan2(affine.a11),
                },
                half_extents: [
                    ((max_x - min_x) * 0.5 * x_axis).max(MIN_HALF_EXTENT),
                    ((max_y - min_y) * 0.5 * y_axis).max(MIN_HALF_EXTENT),
                ],
            });
        }
    }
    None
}

/// The collider of `body` around an object fitted as `fit`.
pub fn collider_half_extents(body: &PhysicsBody, fit: &BodyFit) -> [f32; 2] {
    let [hx, hy] = fit.half_extents;
    match body.shape {
        ColliderShape::Box => [
            (hx * body.shape_scale).max(MIN_HALF_EXTENT),
            (hy * body.shape_scale).max(MIN_HALF_EXTENT),
        ],
        ColliderShape::Circle => {
            let radius = (hx.max(hy) * body.shape_scale).max(MIN_HALF_EXTENT);
            [radius, radius]
        }
    }
}

/// How the timeline moves `instance_id` into `scene`'s start frame, as stage
/// pixels per second and radians per second of simulated time.
pub fn motion_into_start(
    project: &ProjectV2,
    scene: &PhysicsScene,
    instance_id: u32,
) -> ([f32; 2], f32) {
    let start = scene.start_frame;
    let (Some(previous_frame), Some(now)) = (
        start.checked_sub(1),
        fit_body(project, scene.q0rg_id, instance_id, start),
    ) else {
        return ([0.0, 0.0], 0.0);
    };
    let Some(before) = fit_body(project, scene.q0rg_id, instance_id, previous_frame) else {
        return ([0.0, 0.0], 0.0);
    };
    // One frame of timeline is `time_scale / fps` seconds of world time.
    let per_second = f32::from(project.meta.fps.max(1)) / scene.time_scale;
    (
        [
            (now.pose.center[0] - before.pose.center[0]) * per_second,
            (now.pose.center[1] - before.pose.center[1]) * per_second,
        ],
        wrap_angle(now.pose.angle - before.pose.angle) * per_second,
    )
}

fn start_velocity(
    project: &ProjectV2,
    scene: &PhysicsScene,
    body: &PhysicsBody,
) -> ([f32; 2], f32) {
    match body.momentum {
        Momentum::FromMotion => motion_into_start(project, scene, body.instance_id),
        Momentum::Manual { velocity, spin } => (velocity, spin.to_radians()),
    }
}

/// Collision groups as rapier sees them: walls are the last bit and meet
/// every body that collides at all; a body meets only its own group.
fn body_groups(group: u8) -> InteractionGroups {
    if group == 0 || group > MAX_COLLISION_GROUP {
        return InteractionGroups::new(Group::NONE, Group::NONE, InteractionTestMode::And);
    }
    let own = Group::from_bits_truncate(1 << (group - 1));
    InteractionGroups::new(own, own | Group::GROUP_32, InteractionTestMode::And)
}

fn wall_groups() -> InteractionGroups {
    InteractionGroups::new(Group::GROUP_32, Group::ALL, InteractionTestMode::And)
}

/// Runs the physics of `q0rg_id` from its start frame to its end frame.
pub fn simulate(project: &ProjectV2, q0rg_id: u16) -> Result<Simulation, String> {
    let scene = project
        .runtime
        .physics
        .scene(q0rg_id)
        .ok_or("set up physics for this timeline first")?;
    let start = scene.start_frame;
    let frames = u32::from(scene.end_frame.saturating_sub(start)) + 1;
    if frames > MAX_SIMULATED_FRAMES {
        return Err(format!(
            "simulate at most {MAX_SIMULATED_FRAMES} frames at once"
        ));
    }
    let substeps = u32::from(scene.substeps.max(1));
    let fps = f32::from(project.meta.fps.max(1));

    let mut world = PhysicsWorld::new();
    world.gravity = Vector::new(scene.gravity[0], scene.gravity[1]);
    world.integration_parameters.length_unit = PIXELS_PER_UNIT;
    world.integration_parameters.dt = scene.time_scale / fps / substeps as f32;
    add_walls(&mut world, project, scene);

    let mut tracks = Vec::new();
    let mut handles = Vec::<RigidBodyHandle>::new();
    let mut animated = Vec::<Option<Vec<BodyPose>>>::new();
    let mut missing = 0;
    for body in project
        .runtime
        .physics
        .bodies
        .iter()
        .filter(|body| body.q0rg_id == q0rg_id)
    {
        let Some(fit) = fit_body(project, q0rg_id, body.instance_id, start) else {
            missing += 1;
            continue;
        };
        let half_extents = collider_half_extents(body, &fit);
        let [x, y] = fit.pose.center;
        let builder = match body.kind {
            BodyKind::Dynamic => {
                let (velocity, spin) = start_velocity(project, scene, body);
                let builder = RigidBodyBuilder::dynamic()
                    .linvel(Vector::new(velocity[0], velocity[1]))
                    .angvel(spin)
                    .linear_damping(body.linear_damping + scene.air_drag)
                    .angular_damping(body.angular_damping + scene.air_drag)
                    .gravity_scale(body.gravity_scale)
                    .ccd_enabled(true);
                if body.fixed_rotation {
                    builder.lock_rotations()
                } else {
                    builder
                }
            }
            BodyKind::Static => RigidBodyBuilder::fixed(),
            BodyKind::Animated => RigidBodyBuilder::kinematic_position_based(),
        }
        .translation(Vector::new(x, y))
        .rotation(fit.pose.angle);
        let collider = match body.shape {
            ColliderShape::Box => ColliderBuilder::cuboid(half_extents[0], half_extents[1]),
            ColliderShape::Circle => ColliderBuilder::ball(half_extents[0]),
        }
        .restitution(body.bounce)
        .friction(body.friction)
        .mass(body.mass)
        .collision_groups(body_groups(body.group));
        let (handle, _) = world.insert(builder, collider);
        handles.push(handle);
        animated.push((body.kind == BodyKind::Animated).then(|| {
            // Animated bodies follow the timeline; a frame where the object
            // is gone holds its last pose.
            let mut poses = Vec::with_capacity(frames as usize);
            let mut last = fit.pose;
            for offset in 0..frames {
                if let Some(now) =
                    fit_body(project, q0rg_id, body.instance_id, start + offset as u16)
                {
                    last = now.pose;
                }
                poses.push(last);
            }
            poses
        }));
        tracks.push(Track {
            instance_id: body.instance_id,
            kind: body.kind,
            shape: body.shape,
            half_extents,
            start: fit,
            poses: vec![fit.pose],
        });
    }
    if tracks.is_empty() {
        return Err(if missing > 0 {
            "none of the bodies is on the start frame".to_string()
        } else {
            "add an object to the physics first".to_string()
        });
    }

    for offset in 1..frames as usize {
        for substep in 1..=substeps {
            let t = substep as f32 / substeps as f32;
            for (handle, poses) in handles.iter().zip(&animated) {
                let Some(poses) = poses else {
                    continue;
                };
                let from = poses[offset - 1];
                let to = poses[offset];
                let center = Vector::new(
                    from.center[0] + (to.center[0] - from.center[0]) * t,
                    from.center[1] + (to.center[1] - from.center[1]) * t,
                );
                let angle = from.angle + wrap_angle(to.angle - from.angle) * t;
                if let Some(body) = world.bodies.get_mut(*handle) {
                    body.set_next_kinematic_position(Pose::new(center, angle));
                }
            }
            world.step();
        }
        for (track, handle) in tracks.iter_mut().zip(&handles) {
            let Some(body) = world.bodies.get(*handle) else {
                continue;
            };
            let center = body.translation();
            // Angles carry on past a half turn instead of wrapping, so the
            // keys spin the way the body did and tweens between them do too.
            let previous = track.poses.last().map_or(0.0, |pose| pose.angle);
            let pose = BodyPose {
                center: [center.x, center.y],
                angle: previous + wrap_angle(body.rotation().angle() - previous),
            };
            if !pose.center.iter().all(|v| v.is_finite()) || !pose.angle.is_finite() {
                return Err("the simulation blew up; try more steps per frame".to_string());
            }
            track.poses.push(pose);
        }
    }
    Ok(Simulation {
        q0rg_id,
        start_frame: start,
        tracks,
        missing,
    })
}

fn add_walls(world: &mut PhysicsWorld, project: &ProjectV2, scene: &PhysicsScene) {
    let width = f32::from(project.meta.stage_width.max(1));
    let height = f32::from(project.meta.stage_height.max(1));
    // Thick walls that run well past the corners, so nothing slips out
    // between two of them or punches through one.
    let thick = width.max(height);
    let walls = [
        (
            WALL_LEFT,
            [-thick * 0.5, height * 0.5],
            [thick * 0.5, height * 0.5 + thick],
        ),
        (
            WALL_RIGHT,
            [width + thick * 0.5, height * 0.5],
            [thick * 0.5, height * 0.5 + thick],
        ),
        (
            WALL_TOP,
            [width * 0.5, -thick * 0.5],
            [width * 0.5 + thick, thick * 0.5],
        ),
        (
            WALL_BOTTOM,
            [width * 0.5, height + thick * 0.5],
            [width * 0.5 + thick, thick * 0.5],
        ),
    ];
    for (bit, [x, y], [hx, hy]) in walls {
        if scene.walls & bit == 0 {
            continue;
        }
        world.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(x, y)),
            ColliderBuilder::cuboid(hx, hy)
                .restitution(scene.wall_bounce)
                .friction(scene.wall_friction)
                .collision_groups(wall_groups()),
        );
    }
}

/// The transform that shows an object fitted as `start` with its collider
/// moved and turned to `pose`.
pub fn posed_transform(start: &BodyFit, pose: BodyPose) -> Transform2D {
    let turn = pose.angle - start.pose.angle;
    let (sin, cos) = turn.sin_cos();
    let offset_x = start.transform.tx - start.pose.center[0];
    let offset_y = start.transform.ty - start.pose.center[1];
    Transform2D {
        tx: pose.center[0] + cos * offset_x - sin * offset_y,
        ty: pose.center[1] + sin * offset_x + cos * offset_y,
        rotation: start.transform.rotation + turn,
        ..start.transform
    }
}

/// Keys the simulated frames of the falling bodies. Returns how many object
/// keys were written.
pub fn bake(
    project: &mut ProjectV2,
    simulation: &Simulation,
    options: BakeOptions,
) -> Result<usize, String> {
    let q0rg_id = simulation.q0rg_id;
    let start = simulation.start_frame;
    let step = options.key_step.max(1);
    let moving = simulation
        .tracks
        .iter()
        .filter(|track| {
            track.kind == BodyKind::Dynamic
                && track.poses.len() > 1
                && options.only.is_none_or(|only| only == track.instance_id)
        })
        .collect::<Vec<_>>();
    if moving.is_empty() {
        return Err(if options.only.is_some() {
            "the selected body does not fall: set it to Falls".to_string()
        } else {
            "nothing moves: make at least one body fall".to_string()
        });
    }
    let layers = moving
        .iter()
        .map(|track| track.start.layer_id)
        .collect::<BTreeSet<_>>();
    if let Some(locked) = layers
        .iter()
        .find(|layer_id| project.layer_is_locked(q0rg_id, **layer_id))
    {
        return Err(format!(
            "unlock layer {} first",
            layer_name(project, q0rg_id, *locked)
        ));
    }
    let cells = moving
        .iter()
        .flat_map(|track| {
            let last = start + (track.poses.len() - 1) as u16;
            key_frames(start, last, step).map(move |frame| (track.start.layer_id, frame))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    crate::timeline_edit::materialize_keyframes_at_cells(project, q0rg_id, &cells)
        .ok_or("could not add keyframes there")?;
    let q0rg = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .ok_or("the timeline is gone")?;
    let mut keyed = 0;
    for track in moving {
        let Some(layer) = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == track.start.layer_id)
        else {
            continue;
        };
        let last = start + (track.poses.len() - 1) as u16;
        // Every key of the body in the range follows the simulation, keys
        // that were there already included, and tweens to the next one.
        // An authored blank key leaves the object gone on that frame.
        let mut keys = layer
            .placements
            .iter()
            .enumerate()
            .filter(|(_, placement)| {
                placement.instance_id == track.instance_id
                    && (start..=last).contains(&placement.frame)
            })
            .map(|(index, placement)| (placement.frame, index))
            .collect::<Vec<_>>();
        keys.sort_unstable();
        for (position, &(frame, index)) in keys.iter().enumerate() {
            let placement = &mut layer.placements[index];
            if frame > start {
                let pose = track.poses[usize::from(frame - start)];
                placement.transform = posed_transform(&track.start, pose);
                keyed += 1;
            }
            if frame < last {
                let next = keys.get(position + 1).map_or(last, |(next, _)| *next);
                placement.tween = if next > frame + 1 {
                    Tween::Linear { to_frame: next }
                } else {
                    Tween::None
                };
            }
        }
    }
    Ok(keyed)
}

/// The frames a bake keys: every frame after the start, or with a wider
/// step the start itself, every step-th frame and the last one.
fn key_frames(start: u16, last: u16, step: u16) -> impl Iterator<Item = u16> {
    let step = step.max(1);
    let first = if step == 1 {
        start.saturating_add(1)
    } else {
        start
    };
    (first..=last)
        .filter(move |frame| step == 1 || (frame - start).is_multiple_of(step) || *frame == last)
}

/// Removes the keys after the start frame on the layers of falling bodies,
/// or of `only` that one.
pub fn clear_baked(
    project: &mut ProjectV2,
    q0rg_id: u16,
    only: Option<u32>,
) -> Result<usize, String> {
    let scene = project
        .runtime
        .physics
        .scene(q0rg_id)
        .cloned()
        .ok_or("this timeline has no physics")?;
    let first = scene.start_frame.saturating_add(1);
    if first > scene.end_frame {
        return Ok(0);
    }
    let layers = project
        .runtime
        .physics
        .bodies
        .iter()
        .filter(|body| {
            body.q0rg_id == q0rg_id
                && body.kind == BodyKind::Dynamic
                && only.is_none_or(|only| only == body.instance_id)
        })
        .filter_map(|body| fit_body(project, q0rg_id, body.instance_id, scene.start_frame))
        .map(|fit| fit.layer_id)
        .collect::<BTreeSet<_>>();
    if let Some(locked) = layers
        .iter()
        .find(|layer_id| project.layer_is_locked(q0rg_id, **layer_id))
    {
        return Err(format!(
            "unlock layer {} first",
            layer_name(project, q0rg_id, *locked)
        ));
    }
    let mut removed = 0;
    for layer_id in layers {
        removed += crate::timeline_edit::remove_selected_keyframes(
            project,
            q0rg_id,
            TimelineSelection {
                anchor_layer_id: layer_id,
                anchor_frame: first,
                focus_layer_id: layer_id,
                focus_frame: scene.end_frame,
            },
        )
        .unwrap_or(0);
    }
    Ok(removed)
}

fn layer_name(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> String {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map_or_else(
            || format!("#{layer_id}"),
            |layer| format!("\"{}\"", layer.name),
        )
}

fn wrap_angle(angle: f32) -> f32 {
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
    wrapped - std::f32::consts::PI
}

/// Makes the selected objects of the open timeline physics bodies. Returns
/// how many were added.
fn add_selected(app: &mut EditorApp) -> usize {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let refs =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection);
    let mut added = 0;
    for reference in refs
        .into_iter()
        .filter(|reference| reference.q0rg_id == q0rg_id)
    {
        let Some((instance_id, _)) = crate::rigging::ensure_selected_track_instance_id(
            &mut app.state.project,
            q0rg_id,
            reference.layer_id,
            reference.placement_idx,
            frame,
        ) else {
            continue;
        };
        let physics = &mut app.state.project.runtime.physics;
        if physics.body(q0rg_id, instance_id).is_none() {
            physics.bodies.push(PhysicsBody::new(q0rg_id, instance_id));
            app.session.physics.selected_body = Some(instance_id);
            added += 1;
        }
    }
    added
}

fn body_name(project: &ProjectV2, q0rg_id: u16, instance_id: u32) -> String {
    let target = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .flat_map(|layer| &layer.placements)
                .find(|placement| placement.instance_id == instance_id)
        })
        .map(|placement| placement.target);
    match target {
        // Named the way the library names them.
        Some(Target::Asset(id)) => project.asset_names.get(&id).cloned().unwrap_or_else(|| {
            match project.assets.iter().find(|asset| asset.id() == id) {
                Some(Asset::Bitmap(_)) => format!("Bitmap {id}"),
                Some(Asset::Q0v(_)) => format!("Video {id}"),
                _ => format!("Vector {id}"),
            }
        }),
        Some(target) => crate::rigging::target_label(project, target),
        None => format!("Object {instance_id}"),
    }
}

/// Names for `instance_ids`, numbered where several copies of one symbol or
/// drawing would otherwise read the same.
fn body_names(project: &ProjectV2, q0rg_id: u16, instance_ids: &[u32]) -> Vec<String> {
    let names = instance_ids
        .iter()
        .map(|instance_id| body_name(project, q0rg_id, *instance_id))
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashMap::<&str, usize>::new();
    names
        .iter()
        .map(|name| {
            let copies = names.iter().filter(|other| *other == name).count();
            if copies < 2 {
                return name.clone();
            }
            let number = seen.entry(name.as_str()).or_insert(0);
            *number += 1;
            format!("{name} ({number})")
        })
        .collect()
}

/// Selects the object of `instance_id` on the stage at the current frame.
fn select_on_stage(app: &mut EditorApp, instance_id: u32) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(q0rg) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return;
    };
    for layer in &q0rg.layers {
        for state in q0s_format::raster::active_placement_states_at(layer, frame) {
            if layer
                .placements
                .get(state.index)
                .is_some_and(|placement| placement.instance_id == instance_id)
            {
                app.session.selection = Selection::Placement {
                    q0rg_id,
                    layer_id: layer.layer_id,
                    placement_idx: state.index,
                };
                return;
            }
        }
    }
}

pub fn group_color(group: u8) -> Color32 {
    if group == 0 {
        return Color32::from_gray(150);
    }
    // Group 1 starts on teal, well away from the red accent of most themes.
    let hue = (0.47 + f32::from(group - 1) * 0.618_034).fract();
    egui::ecolor::Hsva::new(hue, 0.7, 0.95, 1.0).into()
}

fn group_label(group: u8) -> String {
    if group == 0 {
        "No collisions".to_string()
    } else {
        format!("Group {group}")
    }
}

fn kind_label(kind: BodyKind, mode: RigMode) -> &'static str {
    match (kind, mode) {
        (BodyKind::Dynamic, RigMode::Simple) => "Falls",
        (BodyKind::Static, RigMode::Simple) => "Solid",
        (BodyKind::Animated, RigMode::Simple) => "Animated",
        (BodyKind::Dynamic, RigMode::Pro) => "Dynamic",
        (BodyKind::Static, RigMode::Pro) => "Static",
        (BodyKind::Animated, RigMode::Pro) => "Kinematic",
    }
}

fn kind_hint(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Dynamic => "falls, bounces and gets pushed around; gets the baked keys",
        BodyKind::Static => "stays where it is on the start frame; others hit it",
        BodyKind::Animated => "moves the way you animated it and shoves others aside",
    }
}

fn shape_label(shape: ColliderShape) -> &'static str {
    match shape {
        ColliderShape::Box => "Box",
        ColliderShape::Circle => "Circle",
    }
}

/// Collects what the widgets of one frame did, so a whole drag is one undo
/// step and a click is one too.
#[derive(Default)]
struct Edit {
    snapshot: bool,
    changed: bool,
}

impl Edit {
    fn note(&mut self, response: &egui::Response) {
        self.snapshot |= response.drag_started()
            || response.gained_focus()
            || (response.changed() && !response.dragged() && !response.has_focus());
        self.changed |= response.changed();
    }

    fn drag(
        &mut self,
        ui: &mut Ui,
        value: &mut f32,
        speed: f64,
        range: std::ops::RangeInclusive<f32>,
        suffix: &str,
    ) {
        let response = ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .clamp_range(range)
                .max_decimals(3)
                .suffix(suffix),
        );
        self.note(&response);
    }

    fn slider(
        &mut self,
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        logarithmic: bool,
    ) {
        let response = ui.add(egui::Slider::new(value, range).logarithmic(logarithmic));
        self.note(&response);
    }

    fn checkbox(&mut self, ui: &mut Ui, value: &mut bool, label: &str) {
        let response = ui.checkbox(value, label);
        self.note(&response);
    }

    fn choice<T: PartialEq>(&mut self, ui: &mut Ui, value: &mut T, option: T, label: &str) {
        let response = ui.selectable_value(value, option, label);
        self.note(&response);
    }
}

/// The physics window, shown only while the feature is on.
pub fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.settings.physics_enabled || !app.session.physics.open {
        return;
    }
    refresh_preview(app, ctx);
    let mut open = true;
    egui::Window::new("Physics")
        .open(&mut open)
        .default_size([380.0, 600.0])
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-420.0, 80.0))
        .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| render_contents(app, ui));
    if !open {
        app.session.physics.open = false;
        app.session.physics.stop_preview();
    }
}

/// The real project with the simulation baked in, for the stage to draw
/// while previewing. `None` while the preview is out of date.
pub fn stage_preview(app: &EditorApp) -> Option<(Arc<ProjectV2>, u64)> {
    if !app.settings.physics_enabled || !app.session.physics.open {
        return None;
    }
    let preview = app.session.physics.preview.as_ref()?;
    (preview.source_revision == app.state.revision)
        .then(|| (Arc::clone(&preview.project), preview.revision))
}

/// Re-simulates the preview after any change, once a drag has let go.
fn refresh_preview(app: &mut EditorApp, ctx: &Context) {
    let q0rg_id = app.session.current_q0rg_id;
    let physics = &app.session.physics;
    if !physics.previewing {
        return;
    }
    let options = physics.bake_options();
    let fresh = physics.preview.as_ref().is_some_and(|preview| {
        preview.source_revision == app.state.revision
            && preview.options == options
            && preview.q0rg_id == q0rg_id
    });
    if fresh || ctx.input(|input| input.pointer.any_down()) {
        return;
    }
    let result = simulate(&app.state.project, q0rg_id).and_then(|simulation| {
        let mut project = app.state.project.clone();
        let keyed = bake(&mut project, &simulation, options)?;
        Ok((simulation, project, keyed))
    });
    let physics = &mut app.session.physics;
    match result {
        Ok((simulation, project, keyed)) => {
            physics.preview_serial = physics.preview_serial.wrapping_add(1);
            physics.message = format!(
                "Previewing {keyed} keys on frames {} to {}. Play or scrub to watch; nothing is keyed until you bake.",
                u32::from(simulation.start_frame) + 2,
                u32::from(simulation.last_frame()) + 1
            );
            physics.preview = Some(Preview {
                simulation: Arc::new(simulation),
                project: Arc::new(project),
                revision: (1 << 62) | physics.preview_serial,
                source_revision: app.state.revision,
                options,
                q0rg_id,
            });
        }
        Err(error) => {
            physics.stop_preview();
            physics.message = format!("Preview stopped: {error}");
        }
    }
    ctx.request_repaint();
}

fn render_contents(app: &mut EditorApp, ui: &mut Ui) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    ui.horizontal(|ui| {
        ui.label("Mode");
        for mode in RigMode::ALL {
            ui.selectable_value(&mut app.session.physics.mode, mode, mode.label());
        }
    });
    let mode = app.session.physics.mode;
    ui.label(
        RichText::new(match mode {
            RigMode::Simple => "pick objects, say how they behave, bake them into keys",
            RigMode::Pro => "every number of the world and of each body",
        })
        .small()
        .color(dim),
    );
    ui.separator();

    let Some(scene) = app.state.project.runtime.physics.scene(q0rg_id).cloned() else {
        let timeline = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .map_or_else(String::new, |q0rg| q0rg.name.clone());
        ui.label(format!("\"{timeline}\" has no physics yet."));
        ui.label(
            RichText::new(
                "Objects fall from the current frame on; the result becomes ordinary keyframes.",
            )
            .small()
            .color(dim),
        );
        if ui.button("Set up physics here").clicked() {
            let fps = app.state.project.meta.fps.max(1);
            let start = app.session.current_frame;
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            let scene = app.state.project.runtime.physics.scene_mut(q0rg_id);
            scene.start_frame = start;
            scene.end_frame = start
                .saturating_add(fps.saturating_mul(2) - 1)
                .min(u16::MAX - 1);
            app.state.mark_dirty();
            app.session.status = "physics set up".to_string();
        }
        return;
    };

    // Range and bake stay in view; the setup below scrolls.
    render_bake_controls(app, ui, q0rg_id, &scene, dim);
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(mut scene) = app.state.project.runtime.physics.scene(q0rg_id).cloned() else {
                return;
            };
            let mut edit = Edit::default();
            crate::theme::section_heading(
                ui,
                "World",
                "What pulls on the bodies and what holds them in",
            );
            render_scene(ui, &mut edit, &mut scene, mode, dim);
            if edit.changed || edit.snapshot {
                store_scene(app, &edit, scene.clone());
            }

            ui.add_space(8.0);
            crate::theme::section_heading(
                ui,
                "Bodies",
                "Objects that take part, and how they behave",
            );
            render_bodies(app, ui, q0rg_id, &scene, dim);

            if mode == RigMode::Pro {
                ui.add_space(8.0);
                ui.separator();
                if ui
                    .small_button("Remove physics from this timeline")
                    .on_hover_text("Forget the world and its bodies; keys already baked stay")
                    .clicked()
                {
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    let physics = &mut app.state.project.runtime.physics;
                    physics.scenes.retain(|scene| scene.q0rg_id != q0rg_id);
                    physics.bodies.retain(|body| body.q0rg_id != q0rg_id);
                    app.state.mark_dirty();
                    app.session.physics.stop_preview();
                    app.session.status = "physics removed from this timeline".to_string();
                }
            }
        });
}

fn store_scene(app: &mut EditorApp, edit: &Edit, scene: PhysicsScene) {
    apply_edit(app, edit, |project| {
        if let Some(stored) = project
            .runtime
            .physics
            .scenes
            .iter_mut()
            .find(|stored| stored.q0rg_id == scene.q0rg_id)
        {
            *stored = scene;
            project.runtime.physics.sanitize();
        }
    });
}

/// The frames selected on the timeline of `q0rg_id`, first and last.
fn selected_timeline_frames(app: &EditorApp, q0rg_id: u16) -> Option<(u16, u16)> {
    if let Some((selected_q0rg, cells)) = &app.session.timeline_cell_selection {
        if *selected_q0rg == q0rg_id && !cells.is_empty() {
            let first = cells.iter().map(|(_, frame)| *frame).min()?;
            let last = cells.iter().map(|(_, frame)| *frame).max()?;
            return Some((first, last));
        }
    }
    let selection = app.session.timeline_selection?;
    Some((
        selection.anchor_frame.min(selection.focus_frame),
        selection.anchor_frame.max(selection.focus_frame),
    ))
}

fn render_bake_controls(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    scene: &PhysicsScene,
    dim: Color32,
) {
    let fps = app.state.project.meta.fps.max(1);
    let timeline_last = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .map_or(0, |q0rg| q0rg.frame_count.saturating_sub(1));
    let mut range = scene.clone();
    let mut edit = Edit::default();
    ui.horizontal_wrapped(|ui| {
        // Frames are numbered from 1, the way the timeline shows them.
        ui.label("Frames");
        let mut first = u32::from(range.start_frame) + 1;
        let mut last = u32::from(range.end_frame) + 1;
        let response =
            ui.add(egui::DragValue::new(&mut first).clamp_range(1..=u32::from(u16::MAX)));
        edit.note(&response);
        ui.label("to");
        let response = ui.add(egui::DragValue::new(&mut last).clamp_range(1..=u32::from(u16::MAX)));
        edit.note(&response);
        range.start_frame = u16::try_from(first - 1).unwrap_or(u16::MAX - 1);
        range.end_frame = u16::try_from(last - 1)
            .unwrap_or(u16::MAX - 1)
            .max(range.start_frame);
        let frames = u32::from(range.end_frame - range.start_frame) + 1;
        ui.label(
            RichText::new(format!(
                "{frames} frames, {:.1} s",
                frames as f32 / f32::from(fps)
            ))
            .small()
            .color(dim),
        );
    });
    ui.horizontal_wrapped(|ui| {
        let current = app.session.current_frame;
        let selection = selected_timeline_frames(app, q0rg_id);
        let mut pick = None;
        if ui
            .small_button("From here")
            .on_hover_text("Start on the current frame and keep the length")
            .clicked()
        {
            let length = range.end_frame - range.start_frame;
            pick = Some((current, current.saturating_add(length)));
        }
        if ui
            .small_button("To the end")
            .on_hover_text("Run on to the last frame of the timeline")
            .clicked()
        {
            pick = Some((range.start_frame, timeline_last));
        }
        if ui
            .small_button("Whole timeline")
            .on_hover_text("From the first frame to the last")
            .clicked()
        {
            pick = Some((0, timeline_last));
        }
        if ui
            .add_enabled(
                selection.is_some(),
                egui::Button::new("Selected frames").small(),
            )
            .on_hover_text("The frames selected on the timeline")
            .on_disabled_hover_text("Select frames on the timeline first")
            .clicked()
        {
            pick = selection;
        }
        if let Some((start, end)) = pick {
            range.start_frame = start.min(u16::MAX - 1);
            range.end_frame = end.max(range.start_frame);
            edit.snapshot = true;
            edit.changed = range != *scene;
        }
    });
    if edit.changed || edit.snapshot {
        store_scene(app, &edit, range);
    }

    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        let previewing = app.session.physics.previewing;
        if ui
            .add(egui::Button::new("Preview").selected(previewing))
            .on_hover_text(
                "Watch the result with the real artwork: play or scrub the timeline. It follows every change, and nothing is keyed until you bake.",
            )
            .clicked()
        {
            if previewing {
                app.session.physics.stop_preview();
                app.session.physics.message.clear();
            } else {
                app.session.physics.previewing = true;
            }
        }
        if ui
            .button(RichText::new("Bake keys").strong())
            .on_hover_text("Simulate and key the falling bodies over these frames")
            .clicked()
        {
            bake_into_project(app, q0rg_id);
        }
        if ui
            .button("Remove keys")
            .on_hover_text(
                "Remove the keyframes after the start frame, over these frames, on the layers of the falling bodies",
            )
            .clicked()
        {
            clear_from_project(app, q0rg_id);
        }
    });
    ui.horizontal_wrapped(|ui| {
        let has_selected = app.session.physics.selected_body.is_some();
        ui.add_enabled_ui(has_selected, |ui| {
            ui.checkbox(&mut app.session.physics.only_selected, "Only the selected body")
                .on_hover_text("Key just the body picked below; the others still bump into it");
        });
        if app.session.physics.mode == RigMode::Pro {
            ui.label("Key every");
            let mut step = app.session.physics.key_step.max(1);
            let unit = if step == 1 { " frame" } else { " frames" };
            ui.add(
                egui::DragValue::new(&mut step)
                    .clamp_range(1..=120)
                    .suffix(unit),
            )
            .on_hover_text(
                "1 keys every frame. More leaves gaps that motion tweens fill: fewer keys, a little less exact.",
            );
            app.session.physics.key_step = step;
        }
    });
    let message = &app.session.physics.message;
    let message = if message.is_empty() {
        "Put falling objects on their own layers: a bake keys everything on their layers."
    } else {
        message.as_str()
    };
    ui.label(RichText::new(message).small().color(dim));
}

/// Shows `message` in the window and the status bar.
fn report(app: &mut EditorApp, message: String) {
    app.session.status = message.clone();
    app.session.physics.message = message;
}

fn bake_into_project(app: &mut EditorApp, q0rg_id: u16) {
    let options = app.session.physics.bake_options();
    let before = app.state.project.clone();
    let result = simulate(&app.state.project, q0rg_id).and_then(|simulation| {
        let keyed = bake(&mut app.state.project, &simulation, options)?;
        Ok((keyed, simulation.start_frame, simulation.last_frame()))
    });
    match result {
        Ok((keyed, start, last)) => {
            app.history.snapshot_with_session(&before, &app.session);
            app.state.mark_dirty();
            app.textures.invalidate();
            app.session.physics.stop_preview();
            report(
                app,
                format!(
                    "Baked {keyed} keys on frames {} to {}. Undo takes them back.",
                    u32::from(start) + 2,
                    u32::from(last) + 1
                ),
            );
        }
        Err(error) => {
            app.state.project = before;
            report(app, error);
        }
    }
}

fn clear_from_project(app: &mut EditorApp, q0rg_id: u16) {
    let only = app.session.physics.bake_options().only;
    let before = app.state.project.clone();
    match clear_baked(&mut app.state.project, q0rg_id, only) {
        Ok(0) => report(app, "No keys to remove on these frames.".to_string()),
        Ok(removed) => {
            app.history.snapshot_with_session(&before, &app.session);
            app.state.mark_dirty();
            app.textures.invalidate();
            report(app, format!("Removed {removed} keyframes."));
        }
        Err(error) => {
            app.state.project = before;
            report(app, error);
        }
    }
}

fn apply_edit(app: &mut EditorApp, edit: &Edit, change: impl FnOnce(&mut ProjectV2)) {
    if edit.snapshot {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
    }
    if edit.changed {
        change(&mut app.state.project);
        app.state.mark_dirty();
    }
}

const GRAVITY_PRESETS: [(&str, [f32; 2]); 4] = [
    ("Earth", [0.0, 980.0]),
    ("Moon", [0.0, 160.0]),
    ("Space", [0.0, 0.0]),
    ("Upside down", [0.0, -980.0]),
];

const SPEED_PRESETS: [(&str, f32); 3] = [("Slow motion", 0.4), ("Normal", 1.0), ("Fast", 2.0)];

fn render_scene(
    ui: &mut Ui,
    edit: &mut Edit,
    scene: &mut PhysicsScene,
    mode: RigMode,
    dim: Color32,
) {
    egui::Grid::new("physics_scene")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| match mode {
            RigMode::Simple => {
                ui.label("Gravity");
                ui.horizontal_wrapped(|ui| {
                    for (label, gravity) in GRAVITY_PRESETS {
                        edit.choice(ui, &mut scene.gravity, gravity, label);
                    }
                });
                ui.end_row();
                ui.label("Speed");
                ui.horizontal(|ui| {
                    for (label, speed) in SPEED_PRESETS {
                        edit.choice(ui, &mut scene.time_scale, speed, label);
                    }
                });
                ui.end_row();
                ui.label("Walls");
                ui.horizontal_wrapped(|ui| {
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_BOTTOM, "Floor");
                    let mut sides =
                        scene.walls & (WALL_LEFT | WALL_RIGHT) == WALL_LEFT | WALL_RIGHT;
                    let before = sides;
                    edit.checkbox(ui, &mut sides, "Sides");
                    if sides != before {
                        scene.walls = if sides {
                            scene.walls | WALL_LEFT | WALL_RIGHT
                        } else {
                            scene.walls & !(WALL_LEFT | WALL_RIGHT)
                        };
                    }
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_TOP, "Ceiling");
                });
                ui.end_row();
            }
            RigMode::Pro => {
                ui.label("Gravity");
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut scene.gravity[0], 5.0, -100_000.0..=100_000.0, " x");
                    edit.drag(ui, &mut scene.gravity[1], 5.0, -100_000.0..=100_000.0, " y");
                    ui.label(RichText::new("px/s²").small().color(dim));
                });
                ui.end_row();
                ui.label("Air drag");
                edit.drag(ui, &mut scene.air_drag, 0.01, 0.0..=100.0, " /s");
                ui.end_row();
                ui.label("Time scale");
                edit.drag(ui, &mut scene.time_scale, 0.01, 0.05..=8.0, "×");
                ui.end_row();
                ui.label("Steps per frame");
                let response =
                    ui.add(egui::DragValue::new(&mut scene.substeps).clamp_range(1..=16));
                edit.note(&response);
                ui.end_row();
                ui.label("Walls");
                ui.horizontal_wrapped(|ui| {
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_LEFT, "Left");
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_RIGHT, "Right");
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_TOP, "Top");
                    wall_checkbox(ui, edit, &mut scene.walls, WALL_BOTTOM, "Bottom");
                });
                ui.end_row();
                ui.label("Wall bounce");
                edit.slider(ui, &mut scene.wall_bounce, 0.0..=1.0, false);
                ui.end_row();
                ui.label("Wall friction");
                edit.slider(ui, &mut scene.wall_friction, 0.0..=4.0, false);
                ui.end_row();
            }
        });
    if mode == RigMode::Simple
        && !GRAVITY_PRESETS
            .iter()
            .any(|(_, gravity)| *gravity == scene.gravity)
    {
        ui.label(
            RichText::new(format!(
                "custom gravity {:.0}, {:.0} px/s² (set in Pro)",
                scene.gravity[0], scene.gravity[1]
            ))
            .small()
            .color(dim),
        );
    }
}

fn wall_checkbox(ui: &mut Ui, edit: &mut Edit, walls: &mut u8, bit: u8, label: &str) {
    let mut on = *walls & bit != 0;
    edit.checkbox(ui, &mut on, label);
    if on {
        *walls |= bit;
    } else {
        *walls &= !bit;
    }
}

fn render_bodies(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    scene: &PhysicsScene,
    dim: Color32,
) {
    if ui
        .button("Add selected objects")
        .on_hover_text("Selected objects of this timeline join the physics")
        .clicked()
    {
        let before = app.state.project.clone();
        let added = add_selected(app);
        if added > 0 {
            app.history.snapshot_with_session(&before, &app.session);
            app.state.mark_dirty();
            app.session.status = format!("{added} physics bodies added");
        } else {
            app.state.project = before;
            app.session.status = "select objects on the stage that are not bodies yet".to_string();
        }
    }

    let bodies = app
        .state
        .project
        .runtime
        .physics
        .bodies
        .iter()
        .filter(|body| body.q0rg_id == q0rg_id)
        .map(|body| (body.instance_id, body.kind, body.group))
        .collect::<Vec<_>>();
    if bodies.is_empty() {
        ui.label(
            RichText::new("No bodies yet: select objects on the stage and add them.")
                .small()
                .color(dim),
        );
        return;
    }
    if app
        .session
        .physics
        .selected_body
        .is_none_or(|selected| !bodies.iter().any(|(id, _, _)| *id == selected))
    {
        app.session.physics.selected_body = Some(bodies[0].0);
    }
    let mode = app.session.physics.mode;
    let ids = bodies.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
    let names = body_names(&app.state.project, q0rg_id, &ids);
    let mut remove = None;
    for ((instance_id, kind, group), name) in bodies.iter().zip(&names) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 4.5, group_color(*group));
            let selected = app.session.physics.selected_body == Some(*instance_id);
            let on_stage =
                fit_body(&app.state.project, q0rg_id, *instance_id, scene.start_frame).is_some();
            let label = if on_stage {
                format!("{name} · {}", kind_label(*kind, mode))
            } else {
                format!("{name} · not on the start frame")
            };
            if ui.selectable_label(selected, label).clicked() {
                app.session.physics.selected_body = Some(*instance_id);
                select_on_stage(app, *instance_id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(*instance_id);
                }
            });
        });
    }
    if let Some(instance_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        app.state
            .project
            .runtime
            .physics
            .bodies
            .retain(|body| !(body.q0rg_id == q0rg_id && body.instance_id == instance_id));
        app.state.mark_dirty();
        return;
    }

    let Some(instance_id) = app.session.physics.selected_body else {
        return;
    };
    let Some(mut body) = app
        .state
        .project
        .runtime
        .physics
        .body(q0rg_id, instance_id)
        .cloned()
    else {
        return;
    };
    ui.add_space(6.0);
    let name = ids
        .iter()
        .position(|id| *id == instance_id)
        .map_or_else(String::new, |index| names[index].clone());
    ui.label(RichText::new(name).strong());
    let auto = motion_into_start(&app.state.project, scene, instance_id);
    let mut edit = Edit::default();
    match mode {
        RigMode::Simple => render_body_simple(ui, &mut edit, &mut body, auto, dim),
        RigMode::Pro => render_body_pro(ui, &mut edit, &mut body, auto, dim),
    }
    if edit.changed || edit.snapshot {
        apply_edit(app, &edit, |project| {
            if let Some(stored) = project.runtime.physics.body_mut(q0rg_id, instance_id) {
                *stored = body.clone();
                project.runtime.physics.sanitize();
            }
        });
    }
}

struct Material {
    name: &'static str,
    mass: f32,
    bounce: f32,
    friction: f32,
    linear_damping: f32,
    angular_damping: f32,
}

const MATERIALS: [Material; 5] = [
    Material {
        name: "Rubber",
        mass: 1.0,
        bounce: 0.85,
        friction: 0.9,
        linear_damping: 0.0,
        angular_damping: 0.05,
    },
    Material {
        name: "Wood",
        mass: 1.0,
        bounce: 0.3,
        friction: 0.5,
        linear_damping: 0.0,
        angular_damping: 0.05,
    },
    Material {
        name: "Stone",
        mass: 4.0,
        bounce: 0.1,
        friction: 0.7,
        linear_damping: 0.0,
        angular_damping: 0.1,
    },
    Material {
        name: "Ice",
        mass: 1.0,
        bounce: 0.05,
        friction: 0.02,
        linear_damping: 0.0,
        angular_damping: 0.0,
    },
    Material {
        name: "Feather",
        mass: 0.1,
        bounce: 0.1,
        friction: 0.6,
        linear_damping: 3.0,
        angular_damping: 2.0,
    },
];

fn render_body_simple(
    ui: &mut Ui,
    edit: &mut Edit,
    body: &mut PhysicsBody,
    auto: ([f32; 2], f32),
    dim: Color32,
) {
    egui::Grid::new("physics_body_simple")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Behaves");
            ui.horizontal(|ui| {
                for kind in BodyKind::ALL {
                    edit.choice(ui, &mut body.kind, kind, kind_label(kind, RigMode::Simple));
                }
            });
            ui.end_row();
            ui.label("");
            ui.label(RichText::new(kind_hint(body.kind)).small().color(dim));
            ui.end_row();
            ui.label("Shape");
            ui.horizontal(|ui| {
                for shape in ColliderShape::ALL {
                    edit.choice(ui, &mut body.shape, shape, shape_label(shape));
                }
            });
            ui.end_row();
            ui.label("Collides with");
            group_picker(ui, edit, &mut body.group);
            ui.end_row();
            if body.kind == BodyKind::Dynamic {
                ui.label("Material");
                ui.horizontal_wrapped(|ui| {
                    for material in &MATERIALS {
                        if ui.small_button(material.name).clicked() {
                            body.mass = material.mass;
                            body.bounce = material.bounce;
                            body.friction = material.friction;
                            body.linear_damping = material.linear_damping;
                            body.angular_damping = material.angular_damping;
                            edit.snapshot = true;
                            edit.changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Weight");
                edit.slider(ui, &mut body.mass, 0.05..=50.0, true);
                ui.end_row();
                ui.label("Bounciness");
                edit.slider(ui, &mut body.bounce, 0.0..=1.0, false);
                ui.end_row();
                ui.label("Grip");
                edit.slider(ui, &mut body.friction, 0.0..=2.0, false);
                ui.end_row();
                ui.label("");
                edit.checkbox(ui, &mut body.fixed_rotation, "Stay upright");
                ui.end_row();
            } else {
                ui.label("Bounciness");
                edit.slider(ui, &mut body.bounce, 0.0..=1.0, false);
                ui.end_row();
            }
        });
    if body.kind == BodyKind::Dynamic {
        render_momentum(ui, edit, body, auto, RigMode::Simple, dim);
    }
}

fn render_body_pro(
    ui: &mut Ui,
    edit: &mut Edit,
    body: &mut PhysicsBody,
    auto: ([f32; 2], f32),
    dim: Color32,
) {
    egui::Grid::new("physics_body_pro")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Type");
            ui.horizontal(|ui| {
                for kind in BodyKind::ALL {
                    edit.choice(ui, &mut body.kind, kind, kind_label(kind, RigMode::Pro));
                }
            });
            ui.end_row();
            ui.label("Collider");
            ui.horizontal(|ui| {
                for shape in ColliderShape::ALL {
                    edit.choice(ui, &mut body.shape, shape, shape_label(shape));
                }
                edit.drag(ui, &mut body.shape_scale, 0.01, 0.05..=20.0, "× size");
            });
            ui.end_row();
            ui.label("Collision group");
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::DragValue::new(&mut body.group).clamp_range(0..=MAX_COLLISION_GROUP),
                );
                edit.note(&response);
                ui.label(RichText::new(group_label(body.group)).color(group_color(body.group)));
            });
            ui.end_row();
            ui.label("Mass");
            edit.drag(ui, &mut body.mass, 0.01, 0.001..=100_000.0, "");
            ui.end_row();
            ui.label("Restitution");
            edit.drag(ui, &mut body.bounce, 0.005, 0.0..=1.0, "");
            ui.end_row();
            ui.label("Friction");
            edit.drag(ui, &mut body.friction, 0.005, 0.0..=4.0, "");
            ui.end_row();
            ui.label("Linear damping");
            edit.drag(ui, &mut body.linear_damping, 0.01, 0.0..=100.0, " /s");
            ui.end_row();
            ui.label("Angular damping");
            edit.drag(ui, &mut body.angular_damping, 0.01, 0.0..=100.0, " /s");
            ui.end_row();
            ui.label("Gravity scale");
            edit.drag(ui, &mut body.gravity_scale, 0.01, -20.0..=20.0, "×");
            ui.end_row();
            ui.label("Rotation");
            edit.checkbox(ui, &mut body.fixed_rotation, "Locked");
            ui.end_row();
        });
    if body.kind == BodyKind::Dynamic {
        render_momentum(ui, edit, body, auto, RigMode::Pro, dim);
    } else {
        ui.label(RichText::new(kind_hint(body.kind)).small().color(dim));
    }
}

fn group_picker(ui: &mut Ui, edit: &mut Edit, group: &mut u8) {
    let response = egui::ComboBox::from_id_source("physics_group")
        .selected_text(RichText::new(group_label(*group)).color(group_color(*group)))
        .show_ui(ui, |ui| {
            for candidate in 0..=8 {
                edit.choice(ui, group, candidate, &group_label(candidate));
            }
        });
    response.response.on_hover_text(
        "Bodies collide only with bodies of the same group. Every group hits the walls.",
    );
}

fn render_momentum(
    ui: &mut Ui,
    edit: &mut Edit,
    body: &mut PhysicsBody,
    auto: ([f32; 2], f32),
    mode: RigMode,
    dim: Color32,
) {
    ui.add_space(4.0);
    let manual = matches!(body.momentum, Momentum::Manual { .. });
    ui.horizontal(|ui| {
        ui.label("Momentum");
        let mut choice = manual;
        edit.choice(ui, &mut choice, false, "From earlier frames");
        edit.choice(
            ui,
            &mut choice,
            true,
            if mode == RigMode::Simple {
                "Throw"
            } else {
                "Manual"
            },
        );
        if choice != manual {
            body.momentum = if choice {
                // Start from what the timeline gives, so switching keeps the throw.
                Momentum::Manual {
                    velocity: auto.0,
                    spin: auto.1.to_degrees(),
                }
            } else {
                Momentum::FromMotion
            };
        }
    });
    match &mut body.momentum {
        Momentum::FromMotion => {
            let ([vx, vy], spin) = auto;
            ui.label(
                RichText::new(if vx.hypot(vy) < 0.5 && spin.abs() < 0.01 {
                    "starts at rest: the frame before the start has it in the same place"
                        .to_string()
                } else {
                    format!(
                        "carries on at {:.0} px/s ({:.0}, {:.0}), spinning {:.0}°/s",
                        vx.hypot(vy),
                        vx,
                        vy,
                        spin.to_degrees()
                    )
                })
                .small()
                .color(dim),
            );
        }
        Momentum::Manual { velocity, spin } => {
            ui.horizontal(|ui| {
                ui.label("Velocity");
                edit.drag(ui, &mut velocity[0], 2.0, -1_000_000.0..=1_000_000.0, " x");
                edit.drag(ui, &mut velocity[1], 2.0, -1_000_000.0..=1_000_000.0, " y");
                ui.label(RichText::new("px/s").small().color(dim));
            });
            ui.horizontal(|ui| {
                ui.label("Spin");
                edit.drag(ui, spin, 1.0, -100_000.0..=100_000.0, "°/s");
            });
            if mode == RigMode::Simple {
                ui.label(
                    RichText::new(
                        "positive x is right, positive y is down, positive spin is clockwise",
                    )
                    .small()
                    .color(dim),
                );
            }
        }
    }
}

/// Collider outlines on the stage while the window is open; while
/// previewing, the path each body takes.
pub fn paint_stage_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if !app.settings.physics_enabled || !app.session.physics.open {
        return;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let preview = app.session.physics.preview.as_ref().filter(|preview| {
        preview.source_revision == app.state.revision && preview.q0rg_id == q0rg_id
    });
    match preview {
        Some(preview) => paint_preview(app, painter, view, preview),
        None => paint_live_colliders(app, painter, view),
    }
}

fn paint_preview(app: &EditorApp, painter: &Painter, view: &StageView, preview: &Preview) {
    let simulation = &preview.simulation;
    let frame = app.session.current_frame;
    let selected = app.session.physics.selected_body;
    let project = &app.state.project;
    for track in &simulation.tracks {
        let group = project
            .runtime
            .physics
            .body(simulation.q0rg_id, track.instance_id)
            .map_or(0, |body| body.group);
        let color = group_color(group);
        if track.kind == BodyKind::Dynamic {
            let points = track
                .poses
                .iter()
                .map(|pose| view.stage_to_screen(Vec2::new(pose.center[0], pose.center[1])))
                .collect::<Vec<_>>();
            painter.add(egui::Shape::line(
                points.clone(),
                Stroke::new(1.5_f32, color.gamma_multiply(0.7)),
            ));
            if points.len() <= 400 {
                for point in &points {
                    painter.circle_filled(*point, 1.6, color.gamma_multiply(0.7));
                }
            }
        }
        if let Some(pose) = simulation.pose_at(track, frame) {
            let width = if selected == Some(track.instance_id) {
                2.0_f32
            } else {
                1.0_f32
            };
            paint_collider(
                painter,
                view,
                track.shape,
                track.half_extents,
                pose,
                Stroke::new(width, color),
                false,
            );
        }
    }
    let galley = painter.layout_no_wrap(
        "Physics preview: bake to keep it".to_string(),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let at = view.stage_rect.left_top() + egui::vec2(8.0, 8.0);
    let backdrop = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0));
    painter.rect_filled(backdrop, 4.0, Color32::from_black_alpha(170));
    painter.galley(at, galley, Color32::WHITE);
}

fn paint_live_colliders(app: &EditorApp, painter: &Painter, view: &StageView) {
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let selected = app.session.physics.selected_body;
    for body in project
        .runtime
        .physics
        .bodies
        .iter()
        .filter(|body| body.q0rg_id == q0rg_id)
    {
        let color = group_color(body.group);
        let width = if selected == Some(body.instance_id) {
            2.5_f32
        } else {
            1.5_f32
        };
        let Some(fit) = fit_body(project, q0rg_id, body.instance_id, frame) else {
            continue;
        };
        let half_extents = collider_half_extents(body, &fit);
        let stroke = Stroke::new(
            width,
            if body.kind == BodyKind::Dynamic {
                color
            } else {
                color.gamma_multiply(0.55)
            },
        );
        paint_collider(
            painter,
            view,
            body.shape,
            half_extents,
            fit.pose,
            stroke,
            false,
        );
        if let (BodyKind::Dynamic, Momentum::Manual { velocity, .. }, Some(scene)) = (
            body.kind,
            body.momentum,
            project.runtime.physics.scene(q0rg_id),
        ) {
            if frame == scene.start_frame && velocity[0].hypot(velocity[1]) > 0.5 {
                // Where a quarter second of the throw would take it.
                let from = Vec2::new(fit.pose.center[0], fit.pose.center[1]);
                let to = Vec2::new(from.x + velocity[0] * 0.25, from.y + velocity[1] * 0.25);
                painter.arrow(
                    view.stage_to_screen(from),
                    view.stage_to_screen(to) - view.stage_to_screen(from),
                    Stroke::new(2.0_f32, color),
                );
            }
        }
    }
}

fn paint_collider(
    painter: &Painter,
    view: &StageView,
    shape: ColliderShape,
    half_extents: [f32; 2],
    pose: BodyPose,
    stroke: Stroke,
    ghost: bool,
) {
    let [cx, cy] = pose.center;
    let center = view.stage_to_screen(Vec2::new(cx, cy));
    let fill = if ghost {
        stroke.color.gamma_multiply(0.18)
    } else {
        Color32::TRANSPARENT
    };
    match shape {
        ColliderShape::Circle => {
            let edge = view.stage_to_screen(Vec2::new(cx + half_extents[0], cy));
            let radius = center.distance(edge);
            painter.circle(center, radius, fill, stroke);
            let (sin, cos) = pose.angle.sin_cos();
            let spoke = view.stage_to_screen(Vec2::new(
                cx + cos * half_extents[0],
                cy + sin * half_extents[0],
            ));
            painter.line_segment([center, spoke], stroke);
        }
        ColliderShape::Box => {
            let (sin, cos) = pose.angle.sin_cos();
            let [hx, hy] = half_extents;
            let points = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)]
                .into_iter()
                .map(|(x, y)| {
                    view.stage_to_screen(Vec2::new(cx + cos * x - sin * y, cy + sin * x + cos * y))
                })
                .collect::<Vec<_>>();
            painter.add(egui::Shape::convex_polygon(points, fill, stroke));
        }
    }
    painter.circle_filled(center, 2.5, stroke.color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::physics::PhysicsScene;
    use q0s_format::v2::{
        Anchor, Asset, Layer, Path, Placement, ProjectMeta, Q0rg, Rgba, Target, Tween, VectorAsset,
    };

    /// A 20×20 square drawing.
    fn square_asset(asset_id: u16) -> Asset {
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![Path {
                anchors: vec![
                    corner(-10.0, -10.0),
                    corner(10.0, -10.0),
                    corner(10.0, 10.0),
                    corner(-10.0, 10.0),
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 200,
                g: 40,
                b: 40,
                a: 255,
            }),
            stroke: None,
        })
    }

    fn placed(instance_id: u32, frame: u16, x: f32, y: f32) -> Placement {
        Placement {
            instance_id,
            frame,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: x,
                ty: y,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
        }
    }

    fn layer(layer_id: u16, placements: Vec<Placement>) -> Layer {
        Layer {
            layer_id,
            timeline_frame_count: None,
            name: format!("layer {layer_id}"),
            explicit_keyframes: vec![0],
            placements,
        }
    }

    /// A 400×300 stage with one layer per object.
    fn project(layers: Vec<Layer>) -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "physics".into(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                background_color: Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![square_asset(1)],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 48,
                script: String::new(),
                layers,
            }],
        }
    }

    fn scene(end_frame: u16) -> PhysicsScene {
        PhysicsScene {
            end_frame,
            ..PhysicsScene::new(1)
        }
    }

    #[test]
    fn a_dropped_box_lands_on_the_floor() {
        let mut project = project(vec![layer(1, vec![placed(7, 0, 200.0, 50.0)])]);
        project.runtime.physics.scenes.push(scene(47));
        let mut body = PhysicsBody::new(1, 7);
        body.bounce = 0.0;
        project.runtime.physics.bodies.push(body);

        let simulation = simulate(&project, 1).unwrap();
        let poses = &simulation.tracks[0].poses;
        assert_eq!(poses.len(), 48);
        assert!(poses[3].center[1] > poses[0].center[1], "it falls");
        let rest = poses.last().unwrap().center[1];
        assert!((rest - 290.0).abs() < 2.0, "rests on the floor, got {rest}");
    }

    #[test]
    fn only_bodies_of_the_same_group_collide() {
        let dropped = |group_above: u8| {
            let mut project = project(vec![
                layer(1, vec![placed(1, 0, 200.0, 150.0)]),
                layer(2, vec![placed(2, 0, 200.0, 60.0)]),
            ]);
            let mut world = scene(30);
            world.walls = 0;
            project.runtime.physics.scenes.push(world);
            let mut floor = PhysicsBody::new(1, 1);
            floor.kind = BodyKind::Static;
            project.runtime.physics.bodies.push(floor);
            let mut falling = PhysicsBody::new(1, 2);
            falling.group = group_above;
            project.runtime.physics.bodies.push(falling);
            let simulation = simulate(&project, 1).unwrap();
            simulation.tracks[1].poses.last().unwrap().center[1]
        };
        let same = dropped(1);
        assert!(
            (same - 130.0).abs() < 2.0,
            "lands on the box below, got {same}"
        );
        assert!(dropped(2) > 200.0, "another group falls through");
        assert!(dropped(0) > 200.0, "no group falls through");
    }

    #[test]
    fn momentum_carries_on_from_the_frames_before() {
        let mut project = project(vec![layer(
            1,
            vec![placed(5, 0, 100.0, 100.0), placed(5, 1, 110.0, 100.0)],
        )]);
        let mut world = scene(13);
        world.start_frame = 1;
        world.gravity = [0.0, 0.0];
        world.walls = 0;
        project.runtime.physics.scenes.push(world.clone());
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 5));

        let (velocity, spin) = motion_into_start(&project, &world, 5);
        assert!(
            (velocity[0] - 240.0).abs() < 0.01,
            "10 px a frame at 24 fps"
        );
        assert_eq!(velocity[1], 0.0);
        assert_eq!(spin, 0.0);
        let simulation = simulate(&project, 1).unwrap();
        let x = simulation.tracks[0].poses[12].center[0];
        assert!((x - 230.0).abs() < 0.5, "keeps 10 px a frame, got {x}");

        project.runtime.physics.bodies[0].momentum = Momentum::Manual {
            velocity: [-48.0, 0.0],
            spin: 0.0,
        };
        let simulation = simulate(&project, 1).unwrap();
        let x = simulation.tracks[0].poses[12].center[0];
        assert!((x - 86.0).abs() < 0.5, "manual momentum wins, got {x}");
    }

    #[test]
    fn baking_keys_every_frame_of_the_falling_body_only() {
        let mut project = project(vec![
            layer(1, vec![placed(1, 0, 200.0, 50.0)]),
            layer(2, vec![placed(2, 0, 100.0, 280.0)]),
        ]);
        project.runtime.physics.scenes.push(scene(11));
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 1));
        let mut solid = PhysicsBody::new(1, 2);
        solid.kind = BodyKind::Static;
        project.runtime.physics.bodies.push(solid);

        let simulation = simulate(&project, 1).unwrap();
        let keyed = bake(&mut project, &simulation, BakeOptions::default()).unwrap();
        assert_eq!(keyed, 11);
        let falling = &project.q0rgs[0].layers[0];
        assert_eq!(falling.keyframe_frames(), (0..=11).collect::<Vec<_>>());
        let last = falling.placements.iter().find(|p| p.frame == 11).unwrap();
        let expected = simulation.tracks[0].poses[11];
        assert!((last.transform.ty - expected.center[1]).abs() < 0.01);
        assert_eq!(
            project.q0rgs[0].layers[1].keyframe_frames(),
            vec![0],
            "solid stays put"
        );

        // Baking again from the same start gives the same keys.
        let again = simulate(&project, 1).unwrap();
        let before = project.clone();
        bake(&mut project, &again, BakeOptions::default()).unwrap();
        assert_eq!(project, before);

        assert_eq!(clear_baked(&mut project, 1, None).unwrap(), 11);
        assert_eq!(project.q0rgs[0].layers[0].keyframe_frames(), vec![0]);
    }

    #[test]
    fn turning_keeps_the_object_around_its_collider() {
        let start = BodyFit {
            layer_id: 1,
            transform: Transform2D {
                tx: 10.0,
                ty: 0.0,
                ..Transform2D::IDENTITY
            },
            pose: BodyPose {
                center: [20.0, 0.0],
                angle: 0.0,
            },
            half_extents: [10.0, 10.0],
        };
        let turned = posed_transform(
            &start,
            BodyPose {
                center: [20.0, 0.0],
                angle: std::f32::consts::FRAC_PI_2,
            },
        );
        // The origin sat 10 px left of the center; a quarter turn puts it above.
        assert!((turned.tx - 20.0).abs() < 1.0e-4);
        assert!((turned.ty + 10.0).abs() < 1.0e-4);
        assert!((turned.rotation - std::f32::consts::FRAC_PI_2).abs() < 1.0e-5);
    }

    #[test]
    fn a_wider_key_step_keys_fewer_frames_and_tweens_between() {
        let mut project = project(vec![layer(1, vec![placed(1, 0, 200.0, 50.0)])]);
        project.runtime.physics.scenes.push(scene(11));
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 1));
        let simulation = simulate(&project, 1).unwrap();
        let options = BakeOptions {
            key_step: 4,
            ..BakeOptions::default()
        };
        assert_eq!(bake(&mut project, &simulation, options).unwrap(), 3);
        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(layer.keyframe_frames(), vec![0, 4, 8, 11]);
        let tween_of = |frame: u16| {
            layer
                .placements
                .iter()
                .find(|placement| placement.frame == frame)
                .unwrap()
                .tween
        };
        assert_eq!(tween_of(0), Tween::Linear { to_frame: 4 });
        assert_eq!(tween_of(4), Tween::Linear { to_frame: 8 });
        assert_eq!(tween_of(8), Tween::Linear { to_frame: 11 });
        assert_eq!(tween_of(11), Tween::None);
    }

    #[test]
    fn baking_only_the_selected_body_leaves_the_others_unkeyed() {
        let mut project = project(vec![
            layer(1, vec![placed(1, 0, 100.0, 50.0)]),
            layer(2, vec![placed(2, 0, 300.0, 50.0)]),
        ]);
        project.runtime.physics.scenes.push(scene(9));
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 1));
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 2));
        let simulation = simulate(&project, 1).unwrap();
        let options = BakeOptions {
            only: Some(2),
            ..BakeOptions::default()
        };
        assert_eq!(bake(&mut project, &simulation, options).unwrap(), 9);
        assert_eq!(project.q0rgs[0].layers[0].keyframe_frames(), vec![0]);
        assert_eq!(project.q0rgs[0].layers[1].keyframe_frames().len(), 10);
        assert_eq!(clear_baked(&mut project, 1, Some(1)).unwrap(), 0);
        assert_eq!(clear_baked(&mut project, 1, Some(2)).unwrap(), 9);
    }

    #[test]
    fn spinning_keys_keep_turning_past_a_half_turn() {
        let mut project = project(vec![layer(1, vec![placed(1, 0, 200.0, 150.0)])]);
        let mut world = scene(23);
        world.gravity = [0.0, 0.0];
        world.walls = 0;
        project.runtime.physics.scenes.push(world);
        let mut body = PhysicsBody::new(1, 1);
        body.angular_damping = 0.0;
        body.momentum = Momentum::Manual {
            velocity: [0.0, 0.0],
            spin: 720.0,
        };
        project.runtime.physics.bodies.push(body);
        let simulation = simulate(&project, 1).unwrap();
        bake(&mut project, &simulation, BakeOptions::default()).unwrap();
        let mut rotations = project.q0rgs[0].layers[0]
            .placements
            .iter()
            .map(|placement| (placement.frame, placement.transform.rotation))
            .collect::<Vec<_>>();
        rotations.sort_by_key(|(frame, _)| *frame);
        assert!(
            rotations.windows(2).all(|pair| pair[1].1 > pair[0].1),
            "turns one way all along: {rotations:?}"
        );
        let total = rotations.last().unwrap().1 - rotations[0].1;
        assert!(
            (total - 23.0 / 24.0 * 4.0 * std::f32::consts::PI).abs() < 0.05,
            "two turns a second, got {total}"
        );
    }
}
