//! The physics editor: objects on a timeline fall, bounce and collide,
//! hang on ropes, springs and hinges, and liquids pour around them; the
//! result is baked into ordinary keyframes.
//! The simulation itself lives in `q0physics`, shared with q0player.
//!
//! It is an opt-in tool. While `Settings::physics_enabled` is off nothing of
//! it shows: no menu entry, no window, no stage overlay.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Context, Painter, RichText, Stroke, Ui};
use q0physics::{
    body_collider, fit_bounds, posed_transform, BodyPose, BodyStart, ColliderGeometry, Fit,
    LinkStart, World,
};
use q0s_format::physics::{
    BodyKind, ColliderShape, LiquidSource, Momentum, PhysicsBody, PhysicsLiquid, PhysicsScene,
    MAX_COLLISION_GROUP, WALLS_ALL, WALL_BOTTOM, WALL_LEFT, WALL_RIGHT, WALL_TOP,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, InstanceKey, Placement, ProjectV2, Rgba, Target, Transform2D, Tween, Vec2,
};

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{RigMode, Selection, TimelineSelection};
use crate::ui::FitToScreen;

/// Longest stretch one bake simulates.
const MAX_SIMULATED_FRAMES: u32 = 20_000;
/// The body list scrolls past this height instead of growing the window.
const LIST_HEIGHT: f32 = 190.0;

/// Window state. Lives in the session, never in the project.
#[derive(Default)]
pub struct PhysicsSession {
    pub open: bool,
    pub mode: RigMode,
    pub selected_body: Option<u32>,
    pub selected_liquid: Option<u16>,
    pub selected_link: Option<u16>,
    /// Shows the simulation on the stage with the real artwork, kept up to
    /// date while the setup changes. Nothing is keyed until a bake.
    pub previewing: bool,
    /// Bake only the selected body; the others still bump into it.
    pub only_selected: bool,
    /// A key every this many frames, tweened in between; 0 and 1 key every
    /// frame.
    pub key_step: u16,
    /// Narrows the body list.
    pub filter: String,
    /// The last result, shown in the window.
    pub(crate) message: String,
    cache: RefCell<FitCache>,
    /// A hitbox handle being dragged on the stage.
    drag: Option<HitboxDrag>,
    /// A link end being dragged on the stage.
    pub(crate) link_drag: Option<crate::physics_links::LinkDrag>,
}

/// Which part of a hitbox the pointer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Move,
    Width,
    Height,
    Radius,
    Turn,
}

/// A hitbox handle drag, with what it started from.
#[derive(Debug, Clone, Copy)]
struct HitboxDrag {
    instance_id: u32,
    handle: Handle,
    press: Vec2,
    center: [f32; 2],
    angle: f32,
    fit_half: [f32; 2],
    offset: [f32; 2],
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
    }
}

/// What a bake keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeOptions {
    /// Key only this body; the others still push it around. Liquids are
    /// left alone then.
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

/// An object found on the timeline, with its collider fitted to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyFit {
    pub layer_id: u16,
    pub transform: Transform2D,
    pub fit: Fit,
    /// Its bounds in its own space.
    pub bounds: [f32; 4],
}

impl BodyFit {
    pub fn pose(&self) -> BodyPose {
        self.fit.pose
    }
}

#[derive(Debug, Clone)]
pub struct Track {
    pub instance_id: u32,
    pub kind: BodyKind,
    /// The collider as it starts, in the body's frame.
    pub geometry: ColliderGeometry,
    pub start: BodyFit,
    /// Where the collider starts: the object's middle moved by the offset.
    pub start_pose: BodyPose,
    /// One pose per frame; the first is the start frame.
    pub poses: Vec<BodyPose>,
}

#[derive(Debug, Clone)]
pub struct LiquidTrack {
    pub liquid_id: u16,
    /// Particle positions per frame; the first is the start frame.
    pub frames: Vec<Vec<[f32; 2]>>,
}

#[derive(Debug, Clone)]
pub struct LinkTrack {
    pub link_id: u16,
    /// Where the link runs on each frame, as lines from point to point
    /// (more than one once it has snapped); the first is the start frame.
    pub frames: Vec<Vec<Vec<[f32; 2]>>>,
    /// Per-frame runs still attached at the original first/last end; broken
    /// tips must remain free when the rendered objects differ from the solver.
    pub attached_ends: Vec<Vec<[bool; 2]>>,
}

#[derive(Debug, Clone)]
pub struct Simulation {
    pub q0rg_id: u16,
    pub start_frame: u16,
    pub tracks: Vec<Track>,
    pub liquids: Vec<LiquidTrack>,
    pub links: Vec<LinkTrack>,
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
            .chain(self.liquids.iter().map(|liquid| liquid.frames.len()))
            .chain(self.links.iter().map(|link| link.frames.len()))
            .max()
            .unwrap_or(1);
        self.start_frame
            .saturating_add(u16::try_from(frames.saturating_sub(1)).unwrap_or(u16::MAX))
    }
}

/// Artwork bounds by target, so one pass over the timeline measures each
/// drawing or symbol once. Measuring a symbol walks its whole contents.
pub type Bounds = HashMap<(bool, u16), Option<[f32; 4]>>;

fn target_bounds(project: &ProjectV2, target: Target, bounds: &mut Bounds) -> Option<[f32; 4]> {
    let key = match target {
        Target::Asset(id) => (false, id),
        Target::Q0rg(id) => (true, id),
    };
    *bounds.entry(key).or_insert_with(|| {
        crate::render::target_local_bbox(project, target, None)
            .map(|(min_x, min_y, max_x, max_y)| [min_x, min_y, max_x, max_y])
            .filter(|[min_x, min_y, max_x, max_y]| {
                [min_x, min_y, max_x, max_y].iter().all(|v| v.is_finite())
                    && max_x >= min_x
                    && max_y >= min_y
            })
    })
}

/// Finds the object `instance_id` on `frame` and fits a collider to it.
pub fn fit_body_with(
    project: &ProjectV2,
    q0rg_id: u16,
    instance_id: u32,
    frame: u16,
    bounds: &mut Bounds,
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
            let own =
                target_bounds(project, placement.target, bounds).unwrap_or([-5.0, -5.0, 5.0, 5.0]);
            return Some(BodyFit {
                layer_id: layer.layer_id,
                transform: state.transform,
                fit: fit_bounds(own, Affine::from_transform(state.transform)),
                bounds: own,
            });
        }
    }
    None
}

pub fn fit_body(
    project: &ProjectV2,
    q0rg_id: u16,
    instance_id: u32,
    frame: u16,
) -> Option<BodyFit> {
    fit_body_with(project, q0rg_id, instance_id, frame, &mut Bounds::new())
}

/// Fits and names for the window and the stage overlay, which ask for them
/// every frame. Any change to the project starts afresh.
#[derive(Default)]
struct FitCache {
    revision: u64,
    q0rg_id: u16,
    bounds: Bounds,
    fits: HashMap<(u32, u16), Option<BodyFit>>,
    colliders: HashMap<(u32, u16), Option<(BodyPose, ColliderGeometry)>>,
    names: HashMap<u32, String>,
}

fn with_cache<R>(app: &EditorApp, work: impl FnOnce(&mut FitCache, &ProjectV2) -> R) -> R {
    let mut cache = app.session.physics.cache.borrow_mut();
    let q0rg_id = app.session.current_q0rg_id;
    if cache.revision != app.state.revision || cache.q0rg_id != q0rg_id {
        *cache = FitCache {
            revision: app.state.revision,
            q0rg_id,
            ..FitCache::default()
        };
    }
    work(&mut cache, &app.state.project)
}

pub(crate) fn cached_fit(app: &EditorApp, instance_id: u32, frame: u16) -> Option<BodyFit> {
    let q0rg_id = app.session.current_q0rg_id;
    with_cache(app, |cache, project| {
        if let Some(fit) = cache.fits.get(&(instance_id, frame)) {
            return *fit;
        }
        let fit = fit_body_with(project, q0rg_id, instance_id, frame, &mut cache.bounds);
        cache.fits.insert((instance_id, frame), fit);
        fit
    })
}

/// The fit, collider pose and collider shape of `body` on `frame`.
pub(crate) fn cached_collider(
    app: &EditorApp,
    body: &PhysicsBody,
    frame: u16,
) -> Option<(BodyFit, BodyPose, ColliderGeometry)> {
    let fit = cached_fit(app, body.instance_id, frame)?;
    let collider = with_cache(app, |cache, project| {
        cache
            .colliders
            .entry((body.instance_id, frame))
            .or_insert_with(|| {
                let parts = body_parts(project, body, frame);
                Some(collider_of(body, &fit, &parts))
            })
            .clone()
    })?;
    Some((fit, collider.0, collider.1))
}

/// The artwork parts a hull or outline collider of `body` takes on `frame`,
/// in the object's own space; nothing for other shapes.
pub fn body_parts(project: &ProjectV2, body: &PhysicsBody, frame: u16) -> Vec<Vec<[f32; 2]>> {
    if !body.shape.follows_artwork() {
        return Vec::new();
    }
    let loops = crate::hitbox::object_loops(project, body.q0rg_id, body.instance_id, frame);
    crate::hitbox::collider_parts(body.shape, &loops)
}

/// Where `body`'s collider sits around the object fitted with `fit`, and
/// its shape.
fn collider_of(
    body: &PhysicsBody,
    fit: &BodyFit,
    parts: &[Vec<[f32; 2]>],
) -> (BodyPose, ColliderGeometry) {
    body_collider(body, fit.fit, Affine::from_transform(fit.transform), parts)
}

/// Where `body`'s collider sits around the object fitted with `fit`.
fn collider_pose(body: &PhysicsBody, fit: &BodyFit) -> BodyPose {
    collider_of(body, fit, &[]).0
}

/// What the window calls object `instance_id` of the open timeline.
pub(crate) fn object_name(app: &EditorApp, instance_id: u32) -> String {
    cached_name(app, instance_id)
}

fn cached_name(app: &EditorApp, instance_id: u32) -> String {
    let q0rg_id = app.session.current_q0rg_id;
    with_cache(app, |cache, project| {
        cache
            .names
            .entry(instance_id)
            .or_insert_with(|| body_name(project, q0rg_id, instance_id))
            .clone()
    })
}

/// How the timeline moves an object into the start frame, as stage pixels
/// per second and radians per second of simulated time.
fn motion_between(
    fps: u16,
    scene: &PhysicsScene,
    before: Option<BodyPose>,
    now: Option<BodyPose>,
) -> ([f32; 2], f32) {
    let (Some(a), Some(b)) = (before, now) else {
        return ([0.0, 0.0], 0.0);
    };
    // One frame of timeline is `time_scale / fps` seconds of world time.
    let per_second = f32::from(fps.max(1)) / scene.time_scale;
    (
        [
            (b.center[0] - a.center[0]) * per_second,
            (b.center[1] - a.center[1]) * per_second,
        ],
        wrap_angle(b.angle - a.angle) * per_second,
    )
}

/// How the timeline moves `instance_id` into `scene`'s start frame.
pub fn motion_into_start(
    project: &ProjectV2,
    scene: &PhysicsScene,
    instance_id: u32,
) -> ([f32; 2], f32) {
    let mut bounds = Bounds::new();
    let default_body = PhysicsBody::new(scene.q0rg_id, instance_id);
    let body = project
        .runtime
        .physics
        .body(scene.q0rg_id, instance_id)
        .unwrap_or(&default_body);
    let now = fit_body_with(
        project,
        scene.q0rg_id,
        instance_id,
        scene.start_frame,
        &mut bounds,
    );
    let before = scene
        .start_frame
        .checked_sub(1)
        .and_then(|frame| fit_body_with(project, scene.q0rg_id, instance_id, frame, &mut bounds));
    motion_between(
        project.meta.fps,
        scene,
        before.map(|fit| collider_pose(body, &fit)),
        now.map(|fit| collider_pose(body, &fit)),
    )
}

pub fn simulate(project: &ProjectV2, q0rg_id: u16) -> Result<Simulation, String> {
    simulate_with(project, q0rg_id, None)
}

/// Runs the physics of `q0rg_id` from its start frame to its end frame.
/// Setting `cancel` stops it early with an error.
pub fn simulate_with(
    project: &ProjectV2,
    q0rg_id: u16,
    cancel: Option<&AtomicBool>,
) -> Result<Simulation, String> {
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
    let fps = project.meta.fps.max(1);
    let stage = [
        f32::from(project.meta.stage_width.max(1)),
        f32::from(project.meta.stage_height.max(1)),
    ];
    let mut world = World::new(scene, stage, f32::from(fps));
    let mut bounds = Bounds::new();
    let mut tracks = Vec::new();
    let mut animated = Vec::<(usize, Vec<BodyPose>)>::new();
    // Bodies whose collider takes the artwork's shape again every frame.
    let mut reshaping = Vec::<(usize, &PhysicsBody, BodyFit)>::new();
    let mut missing = 0;
    for body in project
        .runtime
        .physics
        .bodies
        .iter()
        .filter(|body| body.q0rg_id == q0rg_id)
    {
        let Some(fit) = fit_body_with(project, q0rg_id, body.instance_id, start, &mut bounds)
        else {
            missing += 1;
            continue;
        };
        let parts = body_parts(project, body, start);
        let (pose, geometry) = collider_of(body, &fit, &parts);
        let (velocity, spin) = match body.momentum {
            Momentum::FromMotion => {
                let before = start.checked_sub(1).and_then(|frame| {
                    fit_body_with(project, q0rg_id, body.instance_id, frame, &mut bounds)
                });
                motion_between(
                    fps,
                    scene,
                    before.map(|before| collider_pose(body, &before)),
                    Some(pose),
                )
            }
            Momentum::Manual { velocity, spin } => (velocity, spin.to_radians()),
        };
        let index = world.add_body(BodyStart {
            body,
            geometry: geometry.clone(),
            pose,
            velocity,
            spin,
        });
        if body.kind == BodyKind::Animated {
            // Animated bodies follow the timeline; a frame where the object
            // is gone holds its last pose.
            let mut poses = Vec::with_capacity(frames as usize);
            let mut last = pose;
            for offset in 0..frames {
                if let Some(now) = fit_body_with(
                    project,
                    q0rg_id,
                    body.instance_id,
                    start + offset as u16,
                    &mut bounds,
                ) {
                    last = collider_pose(body, &now);
                }
                poses.push(last);
            }
            animated.push((index, poses));
        }
        if body.follow_art && body.shape.follows_artwork() {
            reshaping.push((index, body, fit));
        }
        tracks.push(Track {
            instance_id: body.instance_id,
            kind: body.kind,
            geometry,
            start: fit,
            start_pose: pose,
            poses: vec![pose],
        });
    }
    let mut liquids = Vec::new();
    for liquid in project
        .runtime
        .physics
        .liquids
        .iter()
        .filter(|liquid| liquid.q0rg_id == q0rg_id)
    {
        let index = world.add_liquid(liquid);
        liquids.push(LiquidTrack {
            liquid_id: liquid.liquid_id,
            frames: vec![world.liquid_particles(index).to_vec()],
        });
    }
    // Links tie bodies that made it into the world; one whose object is
    // gone from the start frame waits for it.
    let mut links = Vec::new();
    for link in project
        .runtime
        .physics
        .links
        .iter()
        .filter(|link| link.q0rg_id == q0rg_id)
    {
        let Some(to) = world.index_of(link.to) else {
            continue;
        };
        let from = match link.from {
            0 => None,
            from => match world.index_of(from) {
                Some(index) => Some(index),
                None => continue,
            },
        };
        let index = world.add_link(LinkStart { link, from, to });
        links.push(LinkTrack {
            link_id: link.link_id,
            frames: vec![world.link_lines(index)],
            attached_ends: vec![world.link_attached_ends(index)],
        });
    }
    if tracks.is_empty() && liquids.is_empty() {
        return Err(if missing > 0 {
            "none of the bodies is on the start frame".to_string()
        } else {
            "add an object or a liquid to the physics first".to_string()
        });
    }

    for offset in 1..frames as usize {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err("cancelled".to_string());
        }
        for (index, poses) in &animated {
            world.drive(*index, poses[offset]);
        }
        let frame = start + offset as u16;
        for (index, body, start_fit) in &reshaping {
            // Animated bodies take the timeline's placement each frame; the
            // others keep the frame they started in, which moves with them.
            let fit = match body.kind {
                BodyKind::Animated => {
                    fit_body_with(project, q0rg_id, body.instance_id, frame, &mut bounds)
                        .unwrap_or(*start_fit)
                }
                _ => *start_fit,
            };
            let parts = body_parts(project, body, frame);
            if !parts.is_empty() {
                world.set_geometry(*index, collider_of(body, &fit, &parts).1);
            }
        }
        world.step_frame()?;
        for (index, track) in tracks.iter_mut().enumerate() {
            track.poses.push(world.pose(index));
        }
        for (index, liquid) in liquids.iter_mut().enumerate() {
            liquid.frames.push(world.liquid_particles(index).to_vec());
        }
        for (index, link) in links.iter_mut().enumerate() {
            link.frames.push(world.link_lines(index));
            link.attached_ends.push(world.link_attached_ends(index));
        }
    }
    Ok(Simulation {
        q0rg_id,
        start_frame: start,
        tracks,
        liquids,
        links,
        missing,
    })
}

/// Keys the simulated frames: falling bodies, and every liquid as a drawing
/// per frame on a layer of its own. Returns how many object keys and liquid
/// frames were written.
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
    let with_liquids = options.only.is_none() && !simulation.liquids.is_empty();
    let with_links = options.only.is_none()
        && simulation.links.iter().any(|track| {
            project
                .runtime
                .physics
                .link(q0rg_id, track.link_id)
                .is_some_and(|link| link.is_drawn())
        });
    if moving.is_empty() && !with_liquids && !with_links {
        return Err(if options.only.is_some() {
            "the selected body does not fall: set it to Falls".to_string()
        } else {
            "nothing moves: make at least one body fall or add a liquid".to_string()
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
    let mut keyed = 0;
    if !moving.is_empty() {
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
                    placement.transform =
                        posed_transform(track.start.transform, track.start_pose, pose);
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
    }
    if with_liquids {
        keyed += bake_liquids(project, simulation)?;
    }
    if with_links {
        keyed += crate::physics_links::bake_links(project, simulation)?;
    }
    Ok(keyed)
}

/// Draws every liquid of `simulation` on its own layer, a filled drawing per
/// frame, replacing what an earlier bake drew over the same frames.
fn bake_liquids(project: &mut ProjectV2, simulation: &Simulation) -> Result<usize, String> {
    use crate::sim_bake;
    let q0rg_id = simulation.q0rg_id;
    let start = simulation.start_frame;
    let mut ids = sim_bake::AssetIds::new(project);
    let mut drawn = 0;
    for track in &simulation.liquids {
        let Some(liquid) = project
            .runtime
            .physics
            .liquids
            .iter()
            .find(|liquid| liquid.q0rg_id == q0rg_id && liquid.liquid_id == track.liquid_id)
            .cloned()
        else {
            continue;
        };
        let layer_id = sim_bake::effect_layer(project, q0rg_id, liquid.layer_id, &liquid.name)?;
        if let Some(stored) = project
            .runtime
            .physics
            .liquid_mut(q0rg_id, liquid.liquid_id)
        {
            stored.layer_id = Some(layer_id);
        }
        let last = start + (track.frames.len().max(1) - 1) as u16;
        let folder = format!("{}/{}", sim_bake::PHYSICS_FOLDER, liquid.name);
        sim_bake::clear_range(
            project,
            q0rg_id,
            layer_id,
            start,
            last,
            sim_bake::PHYSICS_FOLDER,
        );
        for (offset, particles) in track.frames.iter().enumerate() {
            let frame = start + offset as u16;
            let outlines =
                q0physics::contour::liquid_outline(particles, liquid.spacing, liquid.smoothness);
            let placements = if outlines.is_empty() {
                Vec::new()
            } else {
                let paths = outlines
                    .iter()
                    .map(|outline| sim_bake::polygon_path(outline))
                    .collect();
                let asset_id = sim_bake::add_drawing(
                    project,
                    &mut ids,
                    paths,
                    liquid.color,
                    format!("{} {}", liquid.name, u32::from(frame) + 1),
                    &folder,
                )?;
                vec![Placement {
                    pivot: None,
                    instance_id: 0,
                    frame,
                    target: Target::Asset(asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }]
            };
            sim_bake::key_frame(project, q0rg_id, layer_id, frame, placements);
            drawn += 1;
        }
        sim_bake::reach_frame(project, q0rg_id, layer_id, last);
    }
    Ok(drawn)
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
/// or of `only` that one, and the liquids drawn over the same frames.
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
    let mut bounds = Bounds::new();
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
        .filter_map(|body| {
            fit_body_with(
                project,
                q0rg_id,
                body.instance_id,
                scene.start_frame,
                &mut bounds,
            )
        })
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
    if first <= scene.end_frame {
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
    }
    if only.is_none() {
        let mut liquid_layers = project
            .runtime
            .physics
            .liquids
            .iter()
            .filter(|liquid| liquid.q0rg_id == q0rg_id)
            .filter_map(|liquid| liquid.layer_id)
            .collect::<Vec<_>>();
        liquid_layers.extend(crate::physics_links::baked_layers(project, q0rg_id));
        for layer_id in liquid_layers {
            let before = keyframe_count(project, q0rg_id, layer_id);
            crate::sim_bake::clear_range(
                project,
                q0rg_id,
                layer_id,
                scene.start_frame,
                scene.end_frame,
                crate::sim_bake::PHYSICS_FOLDER,
            );
            removed += before.saturating_sub(keyframe_count(project, q0rg_id, layer_id));
        }
    }
    Ok(removed)
}

fn keyframe_count(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> usize {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map_or(0, |layer| layer.keyframe_frames().len())
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

/// A copy of `project` for q0player: only the physics worlds and particle
/// emitters it runs live, with the bounds of the bodies it cannot measure.
pub fn for_player(project: &ProjectV2) -> ProjectV2 {
    let mut copy = project.clone();
    copy.runtime.physics.retain_live();
    copy.runtime.particles.retain_live();
    let mut bounds = Bounds::new();
    let physics = &mut copy.runtime.physics;
    for body in &mut physics.bodies {
        let target = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == body.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .flat_map(|layer| &layer.placements)
                    .find(|placement| placement.instance_id == body.instance_id)
            })
            .map(|placement| placement.target);
        body.bounds = target.and_then(|target| target_bounds(project, target, &mut bounds));
        let frame = physics
            .scenes
            .iter()
            .find(|scene| scene.q0rg_id == body.q0rg_id)
            .map_or(0, |scene| scene.start_frame);
        body.outline = body_parts(project, body, frame);
    }
    copy
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

/// The selected object of `q0rg_id`, given an instance id of its own if it
/// has none yet, so lights and emitters can follow it.
pub(crate) fn follow_target(app: &mut EditorApp, q0rg_id: u16) -> Option<u32> {
    let reference =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
            .into_iter()
            .find(|reference| reference.q0rg_id == q0rg_id)?;
    crate::rigging::ensure_selected_track_instance_id(
        &mut app.state.project,
        q0rg_id,
        reference.layer_id,
        reference.placement_idx,
        app.session.current_frame,
    )
    .map(|(instance_id, _)| instance_id)
}

/// Whether an object of `q0rg_id` is selected.
pub(crate) fn has_selection_on(app: &EditorApp, q0rg_id: u16) -> bool {
    crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
        .into_iter()
        .any(|reference| reference.q0rg_id == q0rg_id)
}

/// The stage area the selected object covers, min x, min y, width, height.
pub fn selection_area(app: &EditorApp) -> Option<[f32; 4]> {
    let project = &app.state.project;
    let reference = crate::selection_edit::selected_object_refs(project, &app.session.selection)
        .into_iter()
        .next()?;
    let placement = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)?;
    let [min_x, min_y, max_x, max_y] =
        target_bounds(project, placement.target, &mut Bounds::new())?;
    let affine = Affine::from_transform(placement.transform);
    let corners = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ]
    .map(|corner| affine.apply(corner));
    let left = corners.iter().map(|c| c.x).fold(f32::INFINITY, f32::min);
    let top = corners.iter().map(|c| c.y).fold(f32::INFINITY, f32::min);
    let right = corners
        .iter()
        .map(|c| c.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = corners
        .iter()
        .map(|c| c.y)
        .fold(f32::NEG_INFINITY, f32::max);
    Some([left, top, (right - left).max(1.0), (bottom - top).max(1.0)])
}

fn body_name(project: &ProjectV2, q0rg_id: u16, instance_id: u32) -> String {
    if let Some(name) = project
        .runtime
        .instance_names
        .get(&InstanceKey::new(q0rg_id, instance_id))
        .filter(|name| !name.is_empty())
    {
        return name.clone();
    }
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
fn body_names(app: &EditorApp, instance_ids: &[u32]) -> Vec<String> {
    let names = instance_ids
        .iter()
        .map(|instance_id| cached_name(app, *instance_id))
        .collect::<Vec<_>>();
    let mut seen = HashMap::<&str, usize>::new();
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
        ColliderShape::Capsule => "Capsule",
        ColliderShape::Hull => "Hull",
        ColliderShape::Outline => "Outline",
    }
}

fn shape_hint(shape: ColliderShape) -> &'static str {
    match shape {
        ColliderShape::Box => "the object's bounds",
        ColliderShape::Circle => "rolls; as wide as the longer side",
        ColliderShape::Capsule => "a pill: rolls on its ends, rests on its sides",
        ColliderShape::Hull => "shrink-wrapped around the drawing's points",
        ColliderShape::Outline => "follows every dent and hole of the drawing",
    }
}

/// Shape, size, offset and turn of a body's collider.
fn render_hitbox(
    ui: &mut Ui,
    edit: &mut Edit,
    body: &mut PhysicsBody,
    mode: RigMode,
    dim: Color32,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Hitbox").strong());
        for shape in ColliderShape::ALL {
            edit.choice(ui, &mut body.shape, shape, shape_label(shape));
        }
    });
    ui.label(RichText::new(shape_hint(body.shape)).small().color(dim));
    egui::Grid::new(("physics_hitbox", mode == RigMode::Pro))
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Size");
            ui.horizontal(|ui| {
                edit.slider(ui, &mut body.shape_scale, 0.2..=3.0, false);
            });
            ui.end_row();
            if mode == RigMode::Pro {
                ui.label("Width, height");
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut body.size[0], 0.01, 0.05..=20.0, "× w");
                    edit.drag(ui, &mut body.size[1], 0.01, 0.05..=20.0, "× h");
                });
                ui.end_row();
            }
            ui.label("Offset");
            ui.horizontal(|ui| {
                edit.drag(ui, &mut body.offset[0], 0.5, -100_000.0..=100_000.0, " x");
                edit.drag(ui, &mut body.offset[1], 0.5, -100_000.0..=100_000.0, " y");
            });
            ui.end_row();
            if mode == RigMode::Pro {
                ui.label("Turn");
                edit.drag(ui, &mut body.turn, 1.0, -36_000.0..=36_000.0, "°");
                ui.end_row();
            }
            if body.shape.follows_artwork() {
                ui.label("");
                edit.checkbox(ui, &mut body.follow_art, "Reshapes as the drawing changes");
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        let plain = body.shape_scale == 1.0
            && body.size == [1.0, 1.0]
            && body.offset == [0.0, 0.0]
            && body.turn == 0.0;
        if ui
            .add_enabled(!plain, egui::Button::new("Reset hitbox").small())
            .clicked()
        {
            body.shape_scale = 1.0;
            body.size = [1.0, 1.0];
            body.offset = [0.0, 0.0];
            body.turn = 0.0;
            edit.snapshot = true;
            edit.changed = true;
        }
        ui.label(
            RichText::new("or drag it on the stage: middle moves, edges resize, the dot turns")
                .small()
                .color(dim),
        );
    });
}

/// Collects what the widgets of one frame did, so a whole drag is one undo
/// step and a click is one too.
#[derive(Default)]
pub(crate) struct Edit {
    pub(crate) snapshot: bool,
    pub(crate) changed: bool,
    /// "Follow the selected object" was pressed; the window attaches it
    /// once it may change the project.
    pub(crate) follow_selection: bool,
}

impl Edit {
    pub(crate) fn note(&mut self, response: &egui::Response) {
        self.snapshot |= response.drag_started()
            || response.gained_focus()
            || (response.changed() && !response.dragged() && !response.has_focus());
        self.changed |= response.changed();
    }

    pub(crate) fn drag(
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

    pub(crate) fn slider(
        &mut self,
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        logarithmic: bool,
    ) {
        let response = ui.add(egui::Slider::new(value, range).logarithmic(logarithmic));
        self.note(&response);
    }

    pub(crate) fn checkbox(&mut self, ui: &mut Ui, value: &mut bool, label: &str) {
        let response = ui.checkbox(value, label);
        self.note(&response);
    }

    pub(crate) fn choice<T: PartialEq>(
        &mut self,
        ui: &mut Ui,
        value: &mut T,
        option: T,
        label: &str,
    ) {
        let response = ui.selectable_value(value, option, label);
        self.note(&response);
    }

    pub(crate) fn text(&mut self, ui: &mut Ui, value: &mut String, width: f32) {
        let response = ui.add(egui::TextEdit::singleline(value).desired_width(width));
        self.note(&response);
    }

    pub(crate) fn color(&mut self, ui: &mut Ui, value: &mut Rgba) {
        let mut color = Color32::from_rgba_unmultiplied(value.r, value.g, value.b, value.a);
        let response = egui::color_picker::color_edit_button_srgba(
            ui,
            &mut color,
            egui::color_picker::Alpha::OnlyBlend,
        );
        if response.changed() {
            let [r, g, b, a] = color.to_srgba_unmultiplied();
            *value = Rgba { r, g, b, a };
        }
        self.note(&response);
    }
}

/// The physics window, shown only while the feature is on.
pub fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.settings.physics_enabled || !app.session.physics.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Physics")
        .open(&mut open)
        .default_size([390.0, 620.0])
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-430.0, 80.0))
        .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| render_contents(app, ui));
    if !open {
        app.session.physics.open = false;
        app.session.physics.stop_preview();
    }
}

/// What the preview job should bake for physics right now, if anything.
pub fn preview_request(app: &EditorApp) -> Option<(u16, BakeOptions)> {
    let physics = &app.session.physics;
    let q0rg_id = app.session.current_q0rg_id;
    (app.settings.physics_enabled
        && physics.open
        && physics.previewing
        && app.state.project.runtime.physics.scene(q0rg_id).is_some())
    .then(|| (q0rg_id, physics.bake_options()))
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
            RigMode::Pro => "every number of the world, each body and each liquid",
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
                "Objects fall and liquids pour from the current frame on; the result becomes ordinary keyframes.",
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
        .id_source("physics_setup")
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
            let world_ui_id = (app.active_project_tab_id(), q0rg_id);
            ui.push_id(world_ui_id, |ui| {
                render_scene(ui, &mut edit, &mut scene, mode, dim);
            });
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

            ui.add_space(8.0);
            crate::theme::section_heading(
                ui,
                "Links",
                "Ropes, chains, springs and hinges that tie objects together",
            );
            crate::physics_links::render_links(app, ui, q0rg_id, mode, dim);

            ui.add_space(8.0);
            crate::theme::section_heading(
                ui,
                "Liquids",
                "Water, honey or slime that pours, splashes and floats things",
            );
            render_liquids(app, ui, q0rg_id, mode, dim);

            if mode == RigMode::Pro {
                ui.add_space(8.0);
                ui.separator();
                if ui
                    .small_button("Remove physics from this timeline")
                    .on_hover_text(
                        "Forget the world, its bodies, links and liquids; keys already baked stay",
                    )
                    .clicked()
                {
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    let physics = &mut app.state.project.runtime.physics;
                    physics.scenes.retain(|scene| scene.q0rg_id != q0rg_id);
                    physics.bodies.retain(|body| body.q0rg_id != q0rg_id);
                    physics.liquids.retain(|liquid| liquid.q0rg_id != q0rg_id);
                    physics.links.retain(|link| link.q0rg_id != q0rg_id);
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
pub(crate) fn selected_timeline_frames(app: &EditorApp, q0rg_id: u16) -> Option<(u16, u16)> {
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

/// The "Frames from to" row and its shortcuts. Returns the picked range when
/// it changed, and whether the change wants an undo step of its own.
pub(crate) fn render_range(
    app: &EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    range: (u16, u16),
    dim: Color32,
) -> (Option<(u16, u16)>, Edit) {
    let fps = app.state.project.meta.fps.max(1);
    let timeline_last = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .map_or(0, |q0rg| q0rg.frame_count.saturating_sub(1));
    let mut edit = Edit::default();
    let (mut start, mut end) = range;
    ui.horizontal_wrapped(|ui| {
        // Frames are numbered from 1, the way the timeline shows them.
        ui.label("Frames");
        let mut first = u32::from(start) + 1;
        let mut last = u32::from(end) + 1;
        let response =
            ui.add(egui::DragValue::new(&mut first).clamp_range(1..=u32::from(u16::MAX)));
        edit.note(&response);
        ui.label("to");
        let response = ui.add(egui::DragValue::new(&mut last).clamp_range(1..=u32::from(u16::MAX)));
        edit.note(&response);
        start = u16::try_from(first - 1).unwrap_or(u16::MAX - 1);
        end = u16::try_from(last - 1).unwrap_or(u16::MAX - 1).max(start);
        let frames = u32::from(end - start) + 1;
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
            pick = Some((current, current.saturating_add(end - start)));
        }
        if ui
            .small_button("To the end")
            .on_hover_text("Run on to the last frame of the timeline")
            .clicked()
        {
            pick = Some((start, timeline_last));
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
        if let Some((first, last)) = pick {
            start = first.min(u16::MAX - 1);
            end = last.max(start);
            edit.snapshot = true;
            edit.changed = true;
        }
    });
    let changed = (start, end) != range;
    edit.changed &= changed;
    ((changed || edit.snapshot).then_some((start, end)), edit)
}

fn render_bake_controls(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    scene: &PhysicsScene,
    dim: Color32,
) {
    let (picked, edit) = render_range(app, ui, q0rg_id, (scene.start_frame, scene.end_frame), dim);
    if let Some((start, end)) = picked {
        let mut range = scene.clone();
        range.start_frame = start;
        range.end_frame = end;
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
            .on_hover_text("Simulate and key the falling bodies and liquids over these frames")
            .clicked()
        {
            bake_into_project(app, q0rg_id);
        }
        if ui
            .button("Remove keys")
            .on_hover_text(
                "Remove the keyframes after the start frame, over these frames, on the layers of the falling bodies, and the liquids drawn there",
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
                .on_hover_text("Key just the body picked below; the others still bump into it, and liquids stay as they are");
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
    let note = crate::sim_preview::status_note(app, crate::sim_preview::Part::Physics);
    let message = &app.session.physics.message;
    let message = note.as_deref().unwrap_or(if message.is_empty() {
        "Put falling objects on their own layers: a bake keys everything on their layers."
    } else {
        message.as_str()
    });
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
                    "Baked {keyed} keys and liquid frames on frames {} to {}. Undo takes them back.",
                    u32::from(start) + 1,
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

pub(crate) fn apply_edit(app: &mut EditorApp, edit: &Edit, change: impl FnOnce(&mut ProjectV2)) {
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
    render_canvas_collisions(ui, edit, scene, mode);
    ui.add_space(4.0);
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
                ui.label("Simulation rate");
                let mut step_rate = u16::from(scene.substeps.clamp(1, 16)) * 60;
                let response = ui
                    .add(
                        egui::DragValue::new(&mut step_rate)
                            .speed(60.0)
                            .clamp_range(60..=960)
                            .suffix(" Hz"),
                    )
                    .on_hover_text(
                        "Steps per simulated second, independent of the movie's fps. Higher rates improve collisions, ropes and liquids; gravity and speed stay the same.",
                    );
                edit.note(&response);
                if response.changed() {
                    scene.substeps = ((step_rate + 30) / 60).clamp(1, 16) as u8;
                }
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
    edit.checkbox(ui, &mut scene.live, "Live in q0player");
    ui.label(
        RichText::new(if scene.live {
            "q0lang can run this world while the movie plays: import q0.physics, then q0physics.start!, push!, gravity!…"
        } else {
            "On, q0lang scripts can run this world live in q0player instead of playing baked keys."
        })
        .small()
        .color(dim),
    );
}

fn render_canvas_collisions(ui: &mut Ui, edit: &mut Edit, scene: &mut PhysicsScene, mode: RigMode) {
    let memory_id = ui.make_persistent_id("physics_canvas_collision_sides");
    if scene.walls != 0 {
        ui.ctx()
            .data_mut(|data| data.insert_temp(memory_id, scene.walls));
    }
    let mut enabled = scene.walls != 0;
    let response = ui
        .checkbox(&mut enabled, "Collide with canvas edges")
        .on_hover_text(
            "Use the original canvas borders as colliders. Turn off to let bodies, ropes and liquids move outside the canvas. Other object collisions stay enabled.",
        );
    edit.note(&response);
    if response.changed() {
        scene.walls = if enabled {
            ui.ctx()
                .data(|data| data.get_temp::<u8>(memory_id))
                .filter(|walls| *walls != 0)
                .unwrap_or(WALLS_ALL)
        } else {
            0
        };
    }
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal_wrapped(|ui| match mode {
            RigMode::Simple => {
                wall_checkbox(ui, edit, &mut scene.walls, WALL_BOTTOM, "Floor");
                let mut sides = scene.walls & (WALL_LEFT | WALL_RIGHT) == WALL_LEFT | WALL_RIGHT;
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
            }
            RigMode::Pro => {
                wall_checkbox(ui, edit, &mut scene.walls, WALL_LEFT, "Left");
                wall_checkbox(ui, edit, &mut scene.walls, WALL_RIGHT, "Right");
                wall_checkbox(ui, edit, &mut scene.walls, WALL_TOP, "Top");
                wall_checkbox(ui, edit, &mut scene.walls, WALL_BOTTOM, "Bottom");
            }
        });
    });
    if scene.walls != 0 {
        ui.ctx()
            .data_mut(|data| data.insert_temp(memory_id, scene.walls));
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
    ui.horizontal(|ui| {
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
                app.session.status =
                    "select objects on the stage that are not bodies yet".to_string();
            }
        }
    });

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
    let names = body_names(app, &ids);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.session.physics.filter)
                .hint_text("Search bodies")
                .desired_width(ui.available_width() - 70.0),
        );
        ui.label(
            RichText::new(format!("{} bodies", bodies.len()))
                .small()
                .color(dim),
        );
    });
    let filter = app.session.physics.filter.trim().to_lowercase();
    let mut remove = None;
    let mut shown = 0;
    egui::ScrollArea::vertical()
        .id_source("physics_body_list")
        .max_height(LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for ((instance_id, kind, group), name) in bodies.iter().zip(&names) {
                if !filter.is_empty()
                    && !name.to_lowercase().contains(&filter)
                    && !kind_label(*kind, mode).to_lowercase().contains(&filter)
                {
                    continue;
                }
                shown += 1;
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 4.5, group_color(*group));
                    let selected = app.session.physics.selected_body == Some(*instance_id);
                    let on_stage = cached_fit(app, *instance_id, scene.start_frame).is_some();
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
            if shown == 0 {
                ui.label(RichText::new("No body matches.").small().color(dim));
            }
        });
    if let Some(instance_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        let physics = &mut app.state.project.runtime.physics;
        physics
            .bodies
            .retain(|body| !(body.q0rg_id == q0rg_id && body.instance_id == instance_id));
        // Links tied to it go with it.
        physics.links.retain(|link| {
            !(link.q0rg_id == q0rg_id && (link.from == instance_id || link.to == instance_id))
        });
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
    ui.separator();
    let name = ids
        .iter()
        .position(|id| *id == instance_id)
        .map_or_else(String::new, |index| names[index].clone());
    ui.label(RichText::new(name).strong());
    let fps = app.state.project.meta.fps;
    let auto = motion_between(
        fps,
        scene,
        scene
            .start_frame
            .checked_sub(1)
            .and_then(|frame| cached_fit(app, instance_id, frame))
            .map(|fit| collider_pose(&body, &fit)),
        cached_fit(app, instance_id, scene.start_frame).map(|fit| collider_pose(&body, &fit)),
    );
    let mut edit = Edit::default();
    if scene.live {
        render_script_name(app, ui, q0rg_id, instance_id, dim);
    }
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

/// The instance name q0lang uses for the body.
fn render_script_name(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    instance_id: u32,
    dim: Color32,
) {
    let key = InstanceKey::new(q0rg_id, instance_id);
    let mut name = app
        .state
        .project
        .runtime
        .instance_names
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let mut edit = Edit::default();
    ui.horizontal(|ui| {
        ui.label("Script name");
        edit.text(ui, &mut name, 160.0);
    });
    if name.trim().is_empty() {
        ui.label(
            RichText::new(
                "give it a name to push it from q0lang: q0physics.push! \"name\", 200, -300",
            )
            .small()
            .color(dim),
        );
    }
    if edit.changed || edit.snapshot {
        apply_edit(app, &edit, |project| {
            let name = name.trim();
            if name.is_empty() {
                project.runtime.instance_names.remove(&key);
            } else {
                project.runtime.instance_names.insert(key, name.to_string());
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
            ui.label("Collides with");
            group_picker(ui, edit, &mut body.group, "physics_body_group");
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
    ui.add_space(4.0);
    render_hitbox(ui, edit, body, RigMode::Simple, dim);
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
            ui.label("Collision group");
            group_number(ui, edit, &mut body.group);
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
    ui.add_space(4.0);
    render_hitbox(ui, edit, body, RigMode::Pro, dim);
    if body.kind == BodyKind::Dynamic {
        render_momentum(ui, edit, body, auto, RigMode::Pro, dim);
    } else {
        ui.label(RichText::new(kind_hint(body.kind)).small().color(dim));
    }
}

fn group_picker(ui: &mut Ui, edit: &mut Edit, group: &mut u8, id: &str) {
    let response = egui::ComboBox::from_id_source(id)
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

pub(crate) fn group_number(ui: &mut Ui, edit: &mut Edit, group: &mut u8) {
    ui.horizontal(|ui| {
        let response = ui.add(egui::DragValue::new(group).clamp_range(0..=MAX_COLLISION_GROUP));
        edit.note(&response);
        ui.label(RichText::new(group_label(*group)).color(group_color(*group)));
    });
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

struct LiquidPreset {
    name: &'static str,
    color: Rgba,
    viscosity: f32,
    stiffness: f32,
    cohesion: f32,
}

const LIQUID_PRESETS: [LiquidPreset; 5] = [
    LiquidPreset {
        name: "Water",
        color: Rgba {
            r: 60,
            g: 140,
            b: 230,
            a: 220,
        },
        viscosity: 0.1,
        stiffness: 1.0,
        cohesion: 0.5,
    },
    LiquidPreset {
        name: "Honey",
        color: Rgba {
            r: 235,
            g: 160,
            b: 30,
            a: 235,
        },
        viscosity: 6.0,
        stiffness: 1.2,
        cohesion: 1.5,
    },
    LiquidPreset {
        name: "Slime",
        color: Rgba {
            r: 110,
            g: 220,
            b: 70,
            a: 230,
        },
        viscosity: 3.0,
        stiffness: 0.8,
        cohesion: 3.0,
    },
    LiquidPreset {
        name: "Oil",
        color: Rgba {
            r: 40,
            g: 35,
            b: 30,
            a: 235,
        },
        viscosity: 1.0,
        stiffness: 1.0,
        cohesion: 0.8,
    },
    LiquidPreset {
        name: "Lava",
        color: Rgba {
            r: 255,
            g: 90,
            b: 20,
            a: 245,
        },
        viscosity: 10.0,
        stiffness: 1.5,
        cohesion: 2.0,
    },
];

const DETAIL_PRESETS: [(&str, f32); 3] = [("Coarse", 12.0), ("Medium", 8.0), ("Fine", 5.0)];

fn render_liquids(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, mode: RigMode, dim: Color32) {
    if ui
        .button("Add liquid")
        .on_hover_text("A block of water in the upper middle of the stage; drag its numbers or use a selected shape's area")
        .clicked()
    {
        let stage = [
            f32::from(app.state.project.meta.stage_width.max(1)),
            f32::from(app.state.project.meta.stage_height.max(1)),
        ];
        let region = selection_area(app)
            .unwrap_or([stage[0] * 0.3, stage[1] * 0.1, stage[0] * 0.4, stage[1] * 0.25]);
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        let physics = &mut app.state.project.runtime.physics;
        let liquid_id = physics.next_liquid_id(q0rg_id);
        physics
            .liquids
            .push(PhysicsLiquid::new(q0rg_id, liquid_id, region));
        app.session.physics.selected_liquid = Some(liquid_id);
        app.state.mark_dirty();
    }
    let liquids = app
        .state
        .project
        .runtime
        .physics
        .liquids
        .iter()
        .filter(|liquid| liquid.q0rg_id == q0rg_id)
        .map(|liquid| (liquid.liquid_id, liquid.name.clone(), liquid.color))
        .collect::<Vec<_>>();
    if liquids.is_empty() {
        ui.label(
            RichText::new("No liquids yet. A liquid is drawn as one filled shape per frame on a layer of its own.")
                .small()
                .color(dim),
        );
        return;
    }
    if app
        .session
        .physics
        .selected_liquid
        .is_none_or(|selected| !liquids.iter().any(|(id, _, _)| *id == selected))
    {
        app.session.physics.selected_liquid = Some(liquids[0].0);
    }
    let mut remove = None;
    for (liquid_id, name, color) in &liquids {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                3.0,
                Color32::from_rgba_unmultiplied(color.r, color.g, color.b, 255),
            );
            let selected = app.session.physics.selected_liquid == Some(*liquid_id);
            if ui.selectable_label(selected, name.as_str()).clicked() {
                app.session.physics.selected_liquid = Some(*liquid_id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(*liquid_id);
                }
            });
        });
    }
    if let Some(liquid_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        app.state
            .project
            .runtime
            .physics
            .liquids
            .retain(|liquid| !(liquid.q0rg_id == q0rg_id && liquid.liquid_id == liquid_id));
        app.state.mark_dirty();
        return;
    }
    let Some(liquid_id) = app.session.physics.selected_liquid else {
        return;
    };
    let Some(mut liquid) = app
        .state
        .project
        .runtime
        .physics
        .liquids
        .iter()
        .find(|liquid| liquid.q0rg_id == q0rg_id && liquid.liquid_id == liquid_id)
        .cloned()
    else {
        return;
    };
    let area = selection_area(app);
    ui.add_space(4.0);
    let mut edit = Edit::default();
    egui::Grid::new("physics_liquid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            edit.text(ui, &mut liquid.name, 150.0);
            ui.end_row();
            ui.label("Pours");
            ui.horizontal(|ui| {
                for source in LiquidSource::ALL {
                    let label = match source {
                        LiquidSource::Block => "All at once",
                        LiquidSource::Stream => "As a stream",
                    };
                    edit.choice(ui, &mut liquid.source, source, label);
                }
            });
            ui.end_row();
            ui.label(match liquid.source {
                LiquidSource::Block => "Area",
                LiquidSource::Stream => "Spout",
            });
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut liquid.region[0], 1.0, -100_000.0..=100_000.0, " x");
                    edit.drag(ui, &mut liquid.region[1], 1.0, -100_000.0..=100_000.0, " y");
                });
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut liquid.region[2], 1.0, 1.0..=100_000.0, " w");
                    edit.drag(ui, &mut liquid.region[3], 1.0, 1.0..=100_000.0, " h");
                    if ui
                        .add_enabled(area.is_some(), egui::Button::new("Selection").small())
                        .on_hover_text("Fill the area the selected object covers")
                        .on_disabled_hover_text("Select a shape on the stage to use its area")
                        .clicked()
                    {
                        if let Some(area) = area {
                            liquid.region = area;
                            edit.snapshot = true;
                            edit.changed = true;
                        }
                    }
                });
            });
            ui.end_row();
            ui.label("Colour");
            edit.color(ui, &mut liquid.color);
            ui.end_row();
            if mode == RigMode::Simple {
                ui.label("Like");
                ui.horizontal_wrapped(|ui| {
                    for preset in &LIQUID_PRESETS {
                        if ui.small_button(preset.name).clicked() {
                            liquid.color = preset.color;
                            liquid.viscosity = preset.viscosity;
                            liquid.stiffness = preset.stiffness;
                            liquid.cohesion = preset.cohesion;
                            edit.snapshot = true;
                            edit.changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Detail");
                ui.horizontal(|ui| {
                    for (label, spacing) in DETAIL_PRESETS {
                        edit.choice(ui, &mut liquid.spacing, spacing, label);
                    }
                });
                ui.end_row();
                ui.label("Thickness");
                edit.slider(ui, &mut liquid.viscosity, 0.0..=20.0, false);
                ui.end_row();
                ui.label("Stickiness");
                edit.slider(ui, &mut liquid.cohesion, 0.0..=5.0, false);
                ui.end_row();
                ui.label("Floats things");
                edit.slider(ui, &mut liquid.buoyancy, 0.0..=3.0, false);
                ui.end_row();
                ui.label("Touches");
                group_picker(ui, &mut edit, &mut liquid.group, "physics_liquid_group");
                ui.end_row();
            } else {
                ui.label("Particle spacing");
                edit.drag(ui, &mut liquid.spacing, 0.1, 1.0..=200.0, " px");
                ui.end_row();
                ui.label("Most particles");
                let response =
                    ui.add(egui::DragValue::new(&mut liquid.max_particles).clamp_range(1..=20_000));
                edit.note(&response);
                ui.end_row();
                ui.label("Viscosity");
                edit.drag(ui, &mut liquid.viscosity, 0.01, 0.0..=20.0, "");
                ui.end_row();
                ui.label("Stiffness");
                edit.drag(ui, &mut liquid.stiffness, 0.01, 0.05..=20.0, "");
                ui.end_row();
                ui.label("Cohesion");
                edit.drag(ui, &mut liquid.cohesion, 0.01, 0.0..=5.0, "");
                ui.end_row();
                ui.label("Gravity scale");
                edit.drag(ui, &mut liquid.gravity_scale, 0.01, -20.0..=20.0, "×");
                ui.end_row();
                ui.label("Buoyancy");
                edit.drag(ui, &mut liquid.buoyancy, 0.01, 0.0..=10.0, "");
                ui.end_row();
                ui.label("Surface smoothness");
                edit.drag(ui, &mut liquid.smoothness, 0.01, 0.3..=4.0, "");
                ui.end_row();
                ui.label("Collision group");
                group_number(ui, &mut edit, &mut liquid.group);
                ui.end_row();
            }
            if liquid.source == LiquidSource::Stream {
                ui.label("Stream");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        edit.drag(
                            ui,
                            &mut liquid.stream_velocity[0],
                            2.0,
                            -100_000.0..=100_000.0,
                            " x",
                        );
                        edit.drag(
                            ui,
                            &mut liquid.stream_velocity[1],
                            2.0,
                            -100_000.0..=100_000.0,
                            " y",
                        );
                        ui.label(RichText::new("px/s").small().color(dim));
                    });
                    ui.horizontal(|ui| {
                        edit.drag(ui, &mut liquid.stream_rate, 1.0, 0.0..=10_000.0, " drops/s");
                    });
                });
                ui.end_row();
                if mode == RigMode::Simple {
                    ui.label("Most particles");
                    let response = ui.add(
                        egui::DragValue::new(&mut liquid.max_particles).clamp_range(1..=20_000),
                    );
                    edit.note(&response);
                    ui.end_row();
                }
            }
        });
    if edit.changed || edit.snapshot {
        apply_edit(app, &edit, |project| {
            if let Some(stored) = project.runtime.physics.liquid_mut(q0rg_id, liquid_id) {
                *stored = liquid.clone();
                project.runtime.physics.sanitize();
            }
        });
    }
}

/// Collider outlines on the stage while the window is open; while
/// previewing, the path each body takes.
pub fn paint_stage_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if !app.settings.physics_enabled || !app.session.physics.open {
        return;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let simulation = crate::sim_preview::physics_simulation(app)
        .filter(|simulation| simulation.q0rg_id == q0rg_id);
    match &simulation {
        Some(simulation) => paint_preview(app, painter, view, simulation),
        None => paint_live_colliders(app, painter, view),
    }
    crate::physics_links::paint_links(app, painter, view, simulation.as_deref());
    paint_liquid_sources(app, painter, view);
}

fn paint_preview(app: &EditorApp, painter: &Painter, view: &StageView, simulation: &Simulation) {
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
            if points.len() <= 400 {
                for point in &points {
                    painter.circle_filled(*point, 1.6, color.gamma_multiply(0.7));
                }
            }
            painter.add(egui::Shape::line(
                points,
                Stroke::new(1.5_f32, color.gamma_multiply(0.7)),
            ));
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
                &track.geometry,
                pose,
                Stroke::new(width, color),
            );
        }
    }
    crate::sim_preview::paint_banner(painter, view, "Physics preview: bake to keep it", 0);
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
        let Some((fit, pose, geometry)) = cached_collider(app, body, frame) else {
            continue;
        };
        let stroke = Stroke::new(
            width,
            if body.kind == BodyKind::Dynamic {
                color
            } else {
                color.gamma_multiply(0.55)
            },
        );
        paint_collider(painter, view, &geometry, pose, stroke);
        if selected == Some(body.instance_id) && !preview_running(app) {
            paint_handles(painter, view, body, &fit, pose, color);
        }
        if let (BodyKind::Dynamic, Momentum::Manual { velocity, .. }, Some(scene)) = (
            body.kind,
            body.momentum,
            project.runtime.physics.scene(q0rg_id),
        ) {
            if frame == scene.start_frame && velocity[0].hypot(velocity[1]) > 0.5 {
                // Where a quarter second of the throw would take it.
                let from = Vec2::new(fit.pose().center[0], fit.pose().center[1]);
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

/// Where each liquid starts: its area, or its spout and the way it pours.
fn paint_liquid_sources(app: &EditorApp, painter: &Painter, view: &StageView) {
    let q0rg_id = app.session.current_q0rg_id;
    let selected = app.session.physics.selected_liquid;
    for liquid in app
        .state
        .project
        .runtime
        .physics
        .liquids
        .iter()
        .filter(|liquid| liquid.q0rg_id == q0rg_id)
    {
        let color =
            Color32::from_rgba_unmultiplied(liquid.color.r, liquid.color.g, liquid.color.b, 255);
        let width = if selected == Some(liquid.liquid_id) {
            2.0_f32
        } else {
            1.0_f32
        };
        let [x, y, w, h] = liquid.region;
        let corners = [
            Vec2::new(x, y),
            Vec2::new(x + w, y),
            Vec2::new(x + w, y + h),
            Vec2::new(x, y + h),
        ]
        .map(|corner| view.stage_to_screen(corner));
        for side in 0..4 {
            let dashes = egui::Shape::dashed_line(
                &[corners[side], corners[(side + 1) % 4]],
                Stroke::new(width, color),
                6.0,
                4.0,
            );
            painter.extend(dashes);
        }
        if liquid.source == LiquidSource::Stream {
            let from = Vec2::new(x + w * 0.5, y + h * 0.5);
            let to = Vec2::new(
                from.x + liquid.stream_velocity[0] * 0.2,
                from.y + liquid.stream_velocity[1] * 0.2,
            );
            painter.arrow(
                view.stage_to_screen(from),
                view.stage_to_screen(to) - view.stage_to_screen(from),
                Stroke::new(2.0_f32, color),
            );
        }
        painter.text(
            corners[0] + egui::vec2(3.0, 2.0),
            egui::Align2::LEFT_TOP,
            &liquid.name,
            egui::FontId::proportional(11.0),
            color,
        );
    }
}

/// Whether the stage shows a running preview instead of the setup.
fn preview_running(app: &EditorApp) -> bool {
    crate::sim_preview::physics_simulation(app).is_some()
}

/// Screen pixels within which a handle catches the pointer.
pub(crate) const HANDLE_REACH: f32 = 9.0;
/// How far above the hitbox its turn handle floats, in screen pixels.
const TURN_LIFT: f32 = 22.0;

/// The hitbox handles of `body` and where they are on the stage.
fn handle_points(
    body: &PhysicsBody,
    fit: &BodyFit,
    pose: BodyPose,
    view: &StageView,
) -> Vec<(Handle, Vec2)> {
    let scale = [
        body.shape_scale * body.size[0],
        body.shape_scale * body.size[1],
    ];
    let half = [
        fit.fit.half_extents[0] * scale[0],
        fit.fit.half_extents[1] * scale[1],
    ];
    let mut handles = vec![(Handle::Move, Vec2::new(pose.center[0], pose.center[1]))];
    if body.shape == ColliderShape::Circle {
        handles.push((
            Handle::Radius,
            body_point(pose, [half[0].max(half[1]), 0.0]),
        ));
        return handles;
    }
    let turned = BodyPose {
        center: pose.center,
        angle: pose.angle + body.turn.to_radians(),
    };
    let lift = TURN_LIFT / view.effective_scale().abs().max(1.0e-3);
    handles.extend([
        (Handle::Width, body_point(turned, [half[0], 0.0])),
        (Handle::Width, body_point(turned, [-half[0], 0.0])),
        (Handle::Height, body_point(turned, [0.0, half[1]])),
        (Handle::Height, body_point(turned, [0.0, -half[1]])),
        (Handle::Turn, body_point(turned, [0.0, -half[1] - lift])),
    ]);
    handles
}

fn paint_handles(
    painter: &Painter,
    view: &StageView,
    body: &PhysicsBody,
    fit: &BodyFit,
    pose: BodyPose,
    color: Color32,
) {
    let outline = Stroke::new(1.5_f32, color);
    for (handle, point) in handle_points(body, fit, pose, view) {
        let at = view.stage_to_screen(point);
        match handle {
            Handle::Move => {
                painter.rect(
                    egui::Rect::from_center_size(at, egui::vec2(9.0, 9.0)),
                    1.0,
                    Color32::from_black_alpha(140),
                    outline,
                );
            }
            Handle::Turn => {
                painter.circle(at, 5.0, Color32::from_black_alpha(140), outline);
            }
            Handle::Width | Handle::Height | Handle::Radius => {
                painter.rect(
                    egui::Rect::from_center_size(at, egui::vec2(7.0, 7.0)),
                    0.0,
                    color,
                    Stroke::new(1.0_f32, Color32::from_black_alpha(180)),
                );
            }
        }
    }
}

/// Drags the selected body's hitbox handles on the stage. Returns whether
/// the pointer is busy with them, so the tools leave it alone.
pub fn handle_stage(
    app: &mut EditorApp,
    response: &egui::Response,
    cursor: Option<Vec2>,
    view: &StageView,
) -> bool {
    if !app.settings.physics_enabled || !app.session.physics.open || preview_running(app) {
        app.session.physics.drag = None;
        app.session.physics.link_drag = None;
        return false;
    }
    if app.session.physics.drag.is_none()
        && crate::physics_links::handle_stage(app, response, cursor, view)
    {
        return true;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(body) = app
        .session
        .physics
        .selected_body
        .and_then(|id| app.state.project.runtime.physics.body(q0rg_id, id))
        .cloned()
    else {
        app.session.physics.drag = None;
        return false;
    };
    let Some((fit, pose, _)) = cached_collider(app, &body, frame) else {
        return false;
    };
    let near = |screen: egui::Pos2| {
        handle_points(&body, &fit, pose, view)
            .into_iter()
            .map(|(handle, point)| (handle, view.stage_to_screen(point).distance(screen)))
            .filter(|(_, distance)| *distance <= HANDLE_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle)
    };
    if response.drag_started_by(egui::PointerButton::Primary) {
        let press = response
            .ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some(handle) = press.and_then(near) {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.session.physics.drag = Some(HitboxDrag {
                instance_id: body.instance_id,
                handle,
                press: press.map_or(Vec2::new(0.0, 0.0), |press| view.screen_to_stage(press)),
                center: pose.center,
                angle: pose.angle,
                fit_half: fit.fit.half_extents,
                offset: body.offset,
            });
            return true;
        }
        return false;
    }
    let Some(drag) = app.session.physics.drag else {
        if let Some(handle) = response.hover_pos().and_then(near) {
            response.ctx.set_cursor_icon(match handle {
                Handle::Move => egui::CursorIcon::Move,
                Handle::Turn => egui::CursorIcon::Alias,
                _ => egui::CursorIcon::Crosshair,
            });
        }
        return false;
    };
    if response.drag_stopped() || !response.dragged() {
        app.session.physics.drag = None;
        return true;
    }
    let Some(cursor) = cursor else {
        return true;
    };
    let snap = response.ctx.input(|input| input.modifiers.shift);
    let mut changed = body.clone();
    let (sin, cos) = drag.angle.sin_cos();
    let away = [cursor.x - drag.center[0], cursor.y - drag.center[1]];
    match drag.handle {
        Handle::Move => {
            let delta = [cursor.x - drag.press.x, cursor.y - drag.press.y];
            changed.offset = [
                drag.offset[0] + cos * delta[0] + sin * delta[1],
                drag.offset[1] - sin * delta[0] + cos * delta[1],
            ];
            if snap {
                changed.offset = changed.offset.map(|value| value.round());
            }
        }
        Handle::Width | Handle::Height => {
            let turn = drag.angle + body.turn.to_radians();
            let (turn_sin, turn_cos) = turn.sin_cos();
            let (axis, along) = if drag.handle == Handle::Width {
                (0, away[0] * turn_cos + away[1] * turn_sin)
            } else {
                (1, -away[0] * turn_sin + away[1] * turn_cos)
            };
            let base = (drag.fit_half[axis] * body.shape_scale).max(1.0e-3);
            changed.size[axis] = (along.abs() / base).clamp(0.05, 20.0);
            if snap {
                changed.size[axis] = (changed.size[axis] * 20.0).round() / 20.0;
            }
        }
        Handle::Radius => {
            let distance = away[0].hypot(away[1]);
            let base = (drag.fit_half[0] * body.size[0])
                .max(drag.fit_half[1] * body.size[1])
                .max(1.0e-3);
            changed.shape_scale = (distance / base).clamp(0.05, 20.0);
        }
        Handle::Turn => {
            let pointing = away[1].atan2(away[0]) + std::f32::consts::FRAC_PI_2 - drag.angle;
            let mut degrees = pointing.to_degrees();
            degrees = (degrees + 180.0).rem_euclid(360.0) - 180.0;
            if snap {
                degrees = (degrees / 15.0).round() * 15.0;
            }
            changed.turn = degrees;
        }
    }
    if changed != body {
        if let Some(stored) = app
            .state
            .project
            .runtime
            .physics
            .body_mut(q0rg_id, drag.instance_id)
        {
            *stored = changed;
        }
        app.state.project.runtime.physics.sanitize();
        app.state.mark_dirty();
    }
    true
}

/// A straight stretch of outline, from one point to another.
type Edge = [[f32; 2]; 2];

thread_local! {
    /// Outer edges of the part shapes drawn lately, by a hash of the parts.
    static BOUNDARIES: RefCell<Vec<(u64, Vec<Edge>)>> = const { RefCell::new(Vec::new()) };
}

/// [`part_boundary`] of `parts`, remembered while the parts stay the same.
fn cached_boundary(parts: &[Vec<[f32; 2]>]) -> Vec<Edge> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        part.len().hash(&mut hasher);
        for [x, y] in part {
            x.to_bits().hash(&mut hasher);
            y.to_bits().hash(&mut hasher);
        }
    }
    let key = hasher.finish();
    BOUNDARIES.with(|boundaries| {
        let mut boundaries = boundaries.borrow_mut();
        if let Some(at) = boundaries.iter().position(|(stored, _)| *stored == key) {
            let entry = boundaries.remove(at);
            let edges = entry.1.clone();
            boundaries.push(entry);
            return edges;
        }
        let edges = part_boundary(parts);
        if boundaries.len() >= 32 {
            boundaries.remove(0);
        }
        boundaries.push((key, edges.clone()));
        edges
    })
}

/// The stretches of the parts' edges on the outside of the shape they make
/// together: seams where parts touch and edges buried in another part are
/// left out, so the drawing shows one outline.
fn part_boundary(parts: &[Vec<[f32; 2]>]) -> Vec<Edge> {
    let parts = parts
        .iter()
        .filter(|part| part.len() >= 3)
        .map(|part| {
            let doubled_area = part
                .iter()
                .zip(part.iter().cycle().skip(1))
                .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                .sum::<f32>();
            let bounds = part
                .iter()
                .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                });
            (part, doubled_area.signum(), bounds)
        })
        .filter(|(_, turn, _)| *turn != 0.0)
        .collect::<Vec<_>>();
    let extent = parts
        .iter()
        .map(|(_, _, b)| (b[2] - b[0]).max(b[3] - b[1]))
        .fold(0.0f32, f32::max)
        .max(1.0e-3);
    // How far past an edge to look for a neighbouring part, and how finely
    // edges are cut to find the stretches another part covers.
    let probe = extent * 2.0e-3;
    let piece = extent / 96.0;
    let covered = |own: usize, point: [f32; 2]| {
        parts.iter().enumerate().any(|(index, (part, turn, b))| {
            index != own
                && point[0] >= b[0]
                && point[0] <= b[2]
                && point[1] >= b[1]
                && point[1] <= b[3]
                && (0..part.len()).all(|i| {
                    let (a, c) = (part[i], part[(i + 1) % part.len()]);
                    let side =
                        (c[0] - a[0]) * (point[1] - a[1]) - (c[1] - a[1]) * (point[0] - a[0]);
                    side * turn >= 0.0
                })
        })
    };
    let mut edges = Vec::new();
    for (own, (part, turn, _)) in parts.iter().enumerate() {
        for i in 0..part.len() {
            let (a, b) = (part[i], part[(i + 1) % part.len()]);
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            if length <= 1.0e-6 {
                continue;
            }
            // Outward: to the right of the edge when the part turns left.
            let out = [
                (b[1] - a[1]) / length * turn * probe,
                -(b[0] - a[0]) / length * turn * probe,
            ];
            let pieces = ((length / piece).ceil() as usize).clamp(1, 256);
            let at = |t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let mut run: Option<f32> = None;
            for k in 0..pieces {
                let middle = at((k as f32 + 0.5) / pieces as f32);
                let outside = !covered(own, [middle[0] + out[0], middle[1] + out[1]]);
                let start = k as f32 / pieces as f32;
                match (outside, run) {
                    (true, None) => run = Some(start),
                    (false, Some(from)) => {
                        edges.push([at(from), at(start)]);
                        run = None;
                    }
                    _ => {}
                }
            }
            if let Some(from) = run {
                edges.push([at(from), b]);
            }
        }
    }
    edges
}

/// A point of the body's frame on the stage.
fn body_point(pose: BodyPose, [x, y]: [f32; 2]) -> Vec2 {
    let (sin, cos) = pose.angle.sin_cos();
    Vec2::new(
        pose.center[0] + cos * x - sin * y,
        pose.center[1] + sin * x + cos * y,
    )
}

fn paint_collider(
    painter: &Painter,
    view: &StageView,
    geometry: &ColliderGeometry,
    pose: BodyPose,
    stroke: Stroke,
) {
    let center = view.stage_to_screen(Vec2::new(pose.center[0], pose.center[1]));
    let parts = geometry.outline();
    let to_screen = |point: [f32; 2]| view.stage_to_screen(body_point(pose, point));
    if let ColliderGeometry::Parts(parts) = geometry {
        // Tinted inside, so a shape made of parts reads as one solid. Plain
        // triangles and separate edges: egui's outlines and soft edges
        // stretch sharp corners into long spikes.
        let tint = stroke.color.gamma_multiply(0.18);
        let mut mesh = egui::Mesh::default();
        for part in parts.iter().filter(|part| part.len() >= 3) {
            let first = mesh.vertices.len() as u32;
            for point in part {
                mesh.colored_vertex(to_screen(*point), tint);
            }
            for i in 1..part.len() as u32 - 1 {
                mesh.add_triangle(first, first + i, first + i + 1);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        let edge = Stroke::new(stroke.width * 0.75, stroke.color);
        for [from, to] in cached_boundary(parts) {
            painter.line_segment([to_screen(from), to_screen(to)], edge);
        }
    } else {
        for part in &parts {
            let points = part.iter().map(|point| to_screen(*point)).collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
    }
    if let ColliderGeometry::Circle { radius } = geometry {
        // A spoke shows how far a round body has turned.
        let spoke = view.stage_to_screen(body_point(pose, [*radius, 0.0]));
        painter.line_segment([center, spoke], stroke);
    }
    painter.circle_filled(center, 2.5, stroke.color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::physics::PhysicsScene;
    use q0s_format::v2::{
        Anchor, Asset, Layer, LibraryItemReference, Path, Placement, ProjectMeta, Q0rg, Rgba,
        Target, Tween, VectorAsset,
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
            pivot: None,
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
    fn an_offset_hitbox_lands_the_object_higher() {
        let rest = |offset: [f32; 2]| {
            let mut project = project(vec![layer(1, vec![placed(7, 0, 200.0, 50.0)])]);
            project.runtime.physics.scenes.push(scene(47));
            let mut body = PhysicsBody::new(1, 7);
            body.bounce = 0.0;
            body.offset = offset;
            project.runtime.physics.bodies.push(body);
            let simulation = simulate(&project, 1).unwrap();
            bake(&mut project, &simulation, BakeOptions::default()).unwrap();
            project.q0rgs[0].layers[0]
                .placements
                .iter()
                .find(|placement| placement.frame == 47)
                .unwrap()
                .transform
                .ty
        };
        let plain = rest([0.0, 0.0]);
        let lowered = rest([0.0, 15.0]);
        assert!((plain - 290.0).abs() < 2.0, "{plain}");
        assert!(
            (plain - lowered - 15.0).abs() < 2.0,
            "a hitbox moved down 15 px holds the drawing 15 px up: {plain} vs {lowered}"
        );
    }

    #[test]
    fn outline_hitboxes_follow_the_drawing_and_travel_to_the_player() {
        // A U-shaped cup drawing.
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        let mut project = project(vec![layer(1, vec![placed(7, 0, 200.0, 240.0)])]);
        project.assets[0] = Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![Path {
                anchors: [
                    (-50.0, -30.0),
                    (-40.0, -30.0),
                    (-40.0, 20.0),
                    (40.0, 20.0),
                    (40.0, -30.0),
                    (50.0, -30.0),
                    (50.0, 30.0),
                    (-50.0, 30.0),
                ]
                .into_iter()
                .map(|(x, y)| corner(x, y))
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        });
        let mut scene = scene(23);
        scene.live = true;
        project.runtime.physics.scenes.push(scene);
        let mut cup = PhysicsBody::new(1, 7);
        cup.kind = BodyKind::Static;
        cup.shape = ColliderShape::Outline;
        project.runtime.physics.bodies.push(cup);

        let simulation = simulate(&project, 1).unwrap();
        let ColliderGeometry::Parts(parts) = &simulation.tracks[0].geometry else {
            panic!("an outline is made of parts");
        };
        assert!(parts.len() >= 3, "a U needs three convex parts at least");

        let exported = for_player(&project);
        let body = &exported.runtime.physics.bodies[0];
        assert!(body.outline.len() >= 3, "the parts travel with the movie");
        assert!(body.bounds.is_some());
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

    #[test]
    fn liquids_bake_into_one_drawing_per_frame_on_their_own_layer() {
        let mut project = project(vec![layer(1, Vec::new())]);
        project.runtime.physics.scenes.push(scene(5));
        project
            .runtime
            .physics
            .liquids
            .push(PhysicsLiquid::new(1, 1, [150.0, 50.0, 100.0, 60.0]));
        let simulation = simulate(&project, 1).unwrap();
        assert_eq!(simulation.liquids[0].frames.len(), 6);
        assert_eq!(
            bake(&mut project, &simulation, BakeOptions::default()).unwrap(),
            6
        );
        let layer_id = project.runtime.physics.liquids[0]
            .layer_id
            .expect("bake made a layer");
        let liquid_layer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        assert_eq!(liquid_layer.name, "liquid1");
        assert_eq!(liquid_layer.keyframe_frames(), (0..=5).collect::<Vec<_>>());
        assert_eq!(liquid_layer.placements.len(), 6);
        let drawings = project.assets.len();
        assert_eq!(drawings, 7, "one drawing a frame beside the square");
        let first = match liquid_layer.placements[0].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("drawings, not symbols"),
        };
        assert_eq!(
            project
                .runtime
                .library_item_folders
                .get(&LibraryItemReference::Asset(first)),
            Some(&"q0 physics/liquid1".to_string())
        );

        // Baking again replaces the drawings instead of piling them up.
        let again = simulate(&project, 1).unwrap();
        bake(&mut project, &again, BakeOptions::default()).unwrap();
        assert_eq!(project.assets.len(), drawings);

        assert!(clear_baked(&mut project, 1, None).unwrap() > 0);
        assert_eq!(project.assets.len(), 1, "only the square is left");
    }

    #[test]
    fn the_player_copy_keeps_live_worlds_with_measured_bodies() {
        let mut project = project(vec![layer(1, vec![placed(1, 0, 200.0, 50.0)])]);
        project.runtime.physics.scenes.push(scene(5));
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 1));
        let baked_only = for_player(&project);
        assert!(
            baked_only.runtime.physics.is_empty(),
            "baked physics stays behind"
        );

        project.runtime.physics.scenes[0].live = true;
        let live = for_player(&project);
        assert_eq!(live.runtime.physics.bodies.len(), 1);
        assert_eq!(
            live.runtime.physics.bodies[0].bounds,
            Some([-10.0, -10.0, 10.0, 10.0])
        );
    }

    #[test]
    fn a_shape_made_of_parts_is_outlined_without_its_seams() {
        // A 20 wide square cut into two triangles, and a sliver poking out.
        let parts = vec![
            vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0]],
            vec![[0.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
        ];
        let edges = part_boundary(&parts);
        let total = edges
            .iter()
            .map(|[a, b]| (b[0] - a[0]).hypot(b[1] - a[1]))
            .sum::<f32>();
        assert!((total - 80.0).abs() < 0.5, "only the four sides: {total}");
        for [a, b] in &edges {
            let middle = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let on_side = middle[0].abs() < 1.0e-3
                || middle[1].abs() < 1.0e-3
                || (middle[0] - 20.0).abs() < 1.0e-3
                || (middle[1] - 20.0).abs() < 1.0e-3;
            assert!(on_side, "no diagonal seam: {a:?} {b:?}");
        }
        // A part buried in another adds no outline.
        let mut buried = parts.clone();
        buried.push(vec![[5.0, 5.0], [10.0, 5.0], [10.0, 10.0]]);
        let total_buried = part_boundary(&buried)
            .iter()
            .map(|[a, b]| (b[0] - a[0]).hypot(b[1] - a[1]))
            .sum::<f32>();
        assert!((total_buried - 80.0).abs() < 0.5, "{total_buried}");
    }

    #[test]
    fn a_rope_holds_a_falling_box_and_bakes_into_a_drawing_per_frame() {
        let mut project = project(vec![
            layer(1, vec![placed(7, 0, 200.0, 40.0)]),
            layer(2, vec![placed(8, 0, 200.0, 100.0)]),
        ]);
        project.runtime.physics.scenes.push(scene(23));
        let mut holder = PhysicsBody::new(1, 7);
        holder.kind = BodyKind::Static;
        project.runtime.physics.bodies.push(holder);
        project.runtime.physics.bodies.push(PhysicsBody::new(1, 8));
        let mut rope =
            q0s_format::physics::PhysicsLink::new(1, 1, q0s_format::physics::LinkKind::Rope, 7, 8);
        rope.from_anchor = [0.0, 10.0];
        rope.to_anchor = [0.0, -10.0];
        rope.length = 100.0;
        project.runtime.physics.links.push(rope);

        let simulation = simulate(&project, 1).unwrap();
        let hang = simulation.tracks[1].poses.last().unwrap().center[1];
        assert!(
            (hang - 160.0).abs() < 6.0,
            "hangs a rope's length below: {hang}"
        );
        assert_eq!(simulation.links[0].frames.len(), 24);

        bake(&mut project, &simulation, BakeOptions::default()).unwrap();
        let link = &project.runtime.physics.links[0];
        let layer_id = link.layer_id.expect("the rope got a layer");
        let layer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        assert_eq!(layer.name, "rope1");
        let keyed = layer.placements.iter().filter(|p| p.frame == 12).count();
        assert_eq!(keyed, 1, "a drawing on every frame");
        let Target::Asset(asset_id) = layer.placements[0].target else {
            panic!("a drawing");
        };
        let Some(Asset::Vector(drawing)) = project.assets.iter().find(|a| a.id() == asset_id)
        else {
            panic!("a vector drawing");
        };
        assert!(drawing.fill.is_none() && drawing.stroke.is_some());

        // Baking again reuses the layer and its drawings' folder.
        let layers = project.q0rgs[0].layers.len();
        let assets = project.assets.len();
        bake(&mut project, &simulation, BakeOptions::default()).unwrap();
        assert_eq!(project.q0rgs[0].layers.len(), layers);
        assert_eq!(project.assets.len(), assets);
        clear_baked(&mut project, 1, None).unwrap();
        let layer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        assert!(layer.placements.iter().all(|p| p.frame == 0));
    }
}
