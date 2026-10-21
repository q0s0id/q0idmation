//! Physics and particles that q0lang runs while the movie plays
//! (`q0physics.*!`, `q0particles.*!`). Only worlds and emitters marked live
//! in the editor travel in a movie; everything else is baked into keys.

use std::collections::HashMap;

use q0physics::links::LinkStroke;
use q0physics::particles::{follow_point, ParticleStyle, ParticleSystem};
use q0physics::{
    body_collider, fit_bounds, posed_transform, BodyPose, BodyStart, LinkStart, World,
};
use q0s_format::particles::ParticleLook;
use q0s_format::physics::{BodyKind, Momentum, PhysicsBody, PhysicsLink, PhysicsLiquid};
use q0s_format::q0lang::runtime::{LightCommand, ParticlesCommand, PhysicsCommand};
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::transform::Affine;
use q0s_format::v2::{InstanceKey, ProjectV2, Rgba, Transform2D};

/// Collider bounds for a body exported without any.
const FALLBACK_BOUNDS: [f32; 4] = [-5.0, -5.0, 5.0, 5.0];

struct LiveBody {
    key: InstanceKey,
    index: usize,
    kind: BodyKind,
    /// Its settings, for where its collider sits when the timeline moves it.
    settings: PhysicsBody,
    bounds: [f32; 4],
    start_transform: Transform2D,
    start_pose: BodyPose,
}

struct LiveWorld {
    world: World,
    q0rg_id: u16,
    bodies: Vec<LiveBody>,
    liquids: Vec<PhysicsLiquid>,
    /// The links in the world, in the world's order.
    links: Vec<PhysicsLink>,
    running: bool,
}

#[derive(Default)]
pub(crate) struct Live {
    physics: Option<LiveWorld>,
    emitters: HashMap<String, ParticleSystem>,
}

/// A particle as the stage draws it.
pub struct ParticleDraw {
    pub look: ParticleLook,
    pub position: [f32; 2],
    pub style: ParticleStyle,
}

/// What live physics and particles add on top of the movie this frame.
#[derive(Default)]
pub struct LiveDraws {
    /// Liquid outlines in stage pixels, with their colour.
    pub liquids: Vec<(Vec<Vec<[f32; 2]>>, Rgba)>,
    /// Ropes, chains and springs in stage pixels, with their colour.
    pub links: Vec<(Vec<LinkStroke>, Rgba)>,
    pub particles: Vec<ParticleDraw>,
}

impl LiveDraws {
    pub fn is_empty(&self) -> bool {
        self.liquids.is_empty() && self.links.is_empty() && self.particles.is_empty()
    }
}

/// Where the timeline puts object `instance_id` on `frame`.
fn placed(project: &ProjectV2, q0rg_id: u16, instance_id: u32, frame: u16) -> Option<Transform2D> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    q0rg.layers.iter().find_map(|layer| {
        q0s_format::raster::active_placement_states_at(layer, frame)
            .into_iter()
            .find(|state| {
                layer
                    .placements
                    .get(state.index)
                    .is_some_and(|placement| placement.instance_id == instance_id)
            })
            .map(|state| state.transform)
    })
}

impl Live {
    /// Whether anything still moves without the timeline playing.
    pub(crate) fn is_active(&self) -> bool {
        self.physics.as_ref().is_some_and(|live| live.running)
            || self
                .emitters
                .values()
                .any(|system| system.is_emitting() || !system.particles().is_empty())
    }

    /// Forgets everything, for a new scene.
    pub(crate) fn clear(&mut self) {
        self.physics = None;
        self.emitters.clear();
    }

    fn build_world(
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &RuntimeSceneState,
    ) -> Result<LiveWorld, String> {
        let scene = project.runtime.physics.scene(q0rg_id).ok_or(
            "this movie has no live physics: turn on \"Live in q0player\" in the physics editor",
        )?;
        let fps = f32::from(project.meta.fps.max(1));
        let stage = [
            f32::from(project.meta.stage_width.max(1)),
            f32::from(project.meta.stage_height.max(1)),
        ];
        let mut world = World::new(scene, stage, fps);
        let mut bodies = Vec::new();
        for body in project
            .runtime
            .physics
            .bodies
            .iter()
            .filter(|body| body.q0rg_id == q0rg_id)
        {
            let key = InstanceKey::new(q0rg_id, body.instance_id);
            let Some(authored) = placed(project, q0rg_id, body.instance_id, frame) else {
                continue;
            };
            let transform = scene_state.effective_transform(key, authored);
            let bounds = body.bounds.unwrap_or(FALLBACK_BOUNDS);
            let affine = Affine::from_transform(transform);
            let (pose, geometry) =
                body_collider(body, fit_bounds(bounds, affine), affine, &body.outline);
            let (velocity, spin) = match body.momentum {
                Momentum::Manual { velocity, spin } => (velocity, spin.to_radians()),
                Momentum::FromMotion => frame
                    .checked_sub(1)
                    .and_then(|previous| placed(project, q0rg_id, body.instance_id, previous))
                    .map_or(([0.0, 0.0], 0.0), |before| {
                        let before = collider_pose(body, bounds, before);
                        let per_second = fps / scene.time_scale;
                        (
                            [
                                (pose.center[0] - before.center[0]) * per_second,
                                (pose.center[1] - before.center[1]) * per_second,
                            ],
                            (pose.angle - before.angle) * per_second,
                        )
                    }),
            };
            let index = world.add_body(BodyStart {
                body,
                geometry,
                pose,
                velocity,
                spin,
            });
            bodies.push(LiveBody {
                key,
                index,
                kind: body.kind,
                settings: body.clone(),
                bounds,
                start_transform: transform,
                start_pose: pose,
            });
        }
        let liquids = project
            .runtime
            .physics
            .liquids
            .iter()
            .filter(|liquid| liquid.q0rg_id == q0rg_id)
            .cloned()
            .collect::<Vec<_>>();
        for liquid in &liquids {
            world.add_liquid(liquid);
        }
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
            world.add_link(LinkStart { link, from, to });
            links.push(link.clone());
        }
        Ok(LiveWorld {
            world,
            q0rg_id,
            bodies,
            liquids,
            links,
            running: false,
        })
    }

    fn world(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &RuntimeSceneState,
    ) -> Result<&mut LiveWorld, String> {
        if self
            .physics
            .as_ref()
            .is_none_or(|live| live.q0rg_id != q0rg_id)
        {
            self.physics = Some(Self::build_world(project, q0rg_id, frame, scene_state)?);
        }
        Ok(self.physics.as_mut().expect("built above"))
    }

    /// Carries out one `q0physics.*!` command.
    pub(crate) fn physics(
        &mut self,
        command: &PhysicsCommand,
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &mut RuntimeSceneState,
    ) -> Result<(), String> {
        let body_index = |live: &LiveWorld, name: &str| -> Result<usize, String> {
            let instance_id = project
                .runtime
                .instance_names
                .iter()
                .find(|(key, candidate)| key.q0rg_id == q0rg_id && candidate.as_str() == name)
                .map(|(key, _)| key.instance_id)
                .ok_or_else(|| format!("no object is called \"{name}\""))?;
            live.world
                .index_of(instance_id)
                .ok_or_else(|| format!("\"{name}\" is not a body of the live physics"))
        };
        match command {
            PhysicsCommand::Start => {
                self.world(project, q0rg_id, frame, scene_state)?.running = true;
            }
            PhysicsCommand::Pause => {
                if let Some(live) = &mut self.physics {
                    live.running = false;
                }
            }
            PhysicsCommand::Reset => {
                if let Some(live) = self.physics.take() {
                    for body in &live.bodies {
                        scene_state.clear_transform(body.key);
                    }
                }
            }
            PhysicsCommand::Push { body, x, y } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                let index = body_index(live, body)?;
                live.world.push(index, [*x as f32, *y as f32]);
            }
            PhysicsCommand::Velocity { body, x, y } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                let index = body_index(live, body)?;
                live.world.set_velocity(index, [*x as f32, *y as f32]);
            }
            PhysicsCommand::Spin { body, degrees } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                let index = body_index(live, body)?;
                live.world.set_spin(index, (*degrees as f32).to_radians());
            }
            PhysicsCommand::Teleport { body, x, y } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                let index = body_index(live, body)?;
                live.world.teleport(index, [*x as f32, *y as f32]);
            }
            PhysicsCommand::Gravity { x, y } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                live.world.set_gravity([*x as f32, *y as f32]);
            }
            PhysicsCommand::Cut { link } => {
                let live = self.world(project, q0rg_id, frame, scene_state)?;
                let index = live
                    .links
                    .iter()
                    .position(|candidate| candidate.name == *link)
                    .ok_or_else(|| format!("no link of the live physics is called \"{link}\""))?;
                live.world.cut_link(index);
            }
        }
        Ok(())
    }

    /// Carries out one `q0particles.*!` command.
    pub(crate) fn particles(
        &mut self,
        command: &ParticlesCommand,
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &RuntimeSceneState,
    ) -> Result<(), String> {
        let name = match command {
            ParticlesCommand::Start { emitter }
            | ParticlesCommand::Stop { emitter }
            | ParticlesCommand::Burst { emitter, .. }
            | ParticlesCommand::Move { emitter, .. }
            | ParticlesCommand::Clear { emitter } => emitter,
        };
        if !self.emitters.contains_key(name) {
            let emitter = project
                .runtime
                .particles
                .emitters
                .iter()
                .find(|emitter| emitter.name == *name)
                .ok_or_else(|| {
                    format!("no live emitter is called \"{name}\": turn on Live for it in the particle editor")
                })?;
            let stage = [
                f32::from(project.meta.stage_width.max(1)),
                f32::from(project.meta.stage_height.max(1)),
            ];
            let anchor = emitter.follow.and_then(|instance_id| {
                follow_point(project, q0rg_id, frame, instance_id, scene_state, None)
            });
            let mut system = ParticleSystem::new_at(emitter, stage, anchor);
            // Nothing flies until a script says so.
            system.clear();
            system.set_emitting(false);
            self.emitters.insert(name.clone(), system);
        }
        let system = self.emitters.get_mut(name).expect("inserted above");
        follow(system, project, q0rg_id, frame, scene_state);
        match command {
            ParticlesCommand::Start { .. } => {
                system.set_emitting(true);
                let burst = usize::from(system.emitter().burst);
                system.burst(burst);
            }
            ParticlesCommand::Stop { .. } => system.set_emitting(false),
            ParticlesCommand::Burst { count, .. } => {
                system.burst(count.clamp(0.0, 20_000.0) as usize);
            }
            ParticlesCommand::Move { x, y, .. } => system.set_position([*x as f32, *y as f32]),
            ParticlesCommand::Clear { .. } => {
                system.set_emitting(false);
                system.clear();
            }
        }
        Ok(())
    }

    /// Advances one movie frame.
    pub(crate) fn step(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &mut RuntimeSceneState,
    ) -> Result<(), String> {
        // Physics first, so emitters riding on bodies see where they are now.
        let moved = self.step_physics(project, q0rg_id, frame, scene_state);
        let fps = f32::from(project.meta.fps.max(1));
        for system in self.emitters.values_mut() {
            follow(system, project, q0rg_id, frame, scene_state);
            system.step(1.0 / fps);
        }
        moved
    }

    fn step_physics(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        scene_state: &mut RuntimeSceneState,
    ) -> Result<(), String> {
        let Some(live) = self
            .physics
            .as_mut()
            .filter(|live| live.running && live.q0rg_id == q0rg_id)
        else {
            return Ok(());
        };
        // Animated bodies keep following the timeline.
        for body in live
            .bodies
            .iter()
            .filter(|body| body.kind == BodyKind::Animated)
        {
            if let Some(transform) = placed(project, q0rg_id, body.key.instance_id, frame) {
                let pose = collider_pose(&body.settings, body.bounds, transform);
                live.world.drive(body.index, pose);
            }
        }
        if let Err(error) = live.world.step_frame() {
            live.running = false;
            return Err(error);
        }
        for body in live
            .bodies
            .iter()
            .filter(|body| body.kind == BodyKind::Dynamic)
        {
            let pose = live.world.pose(body.index);
            let placed = posed_transform(body.start_transform, body.start_pose, pose);
            scene_state.set_transform(body.key, placed.tx, placed.ty, placed.rotation);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn draws(&self) -> LiveDraws {
        self.draws_with_scene(None)
    }

    pub(crate) fn draws_for_scene(
        &self,
        project: &ProjectV2,
        frame: u16,
        scene_state: &RuntimeSceneState,
    ) -> LiveDraws {
        self.draws_with_scene(Some((project, frame, scene_state)))
    }

    fn draws_with_scene(&self, scene: Option<(&ProjectV2, u16, &RuntimeSceneState)>) -> LiveDraws {
        let mut draws = LiveDraws::default();
        if let Some(live) = &self.physics {
            for (index, liquid) in live.liquids.iter().enumerate() {
                let outlines = q0physics::contour::liquid_outline(
                    live.world.liquid_particles(index),
                    liquid.spacing,
                    liquid.smoothness,
                );
                if !outlines.is_empty() {
                    draws.liquids.push((outlines, liquid.color));
                }
            }
            for (index, link) in live.links.iter().enumerate() {
                if !link.is_drawn() {
                    continue;
                }
                // Scripts, scale/skew animation and live physics all feed the
                // displayed transform. Pin the rope to that exact artwork,
                // including a paused world or a teleport before the next step.
                let anchor = |instance_id, point| {
                    if instance_id == 0 {
                        return Some(point);
                    }
                    let (project, frame, state) = scene?;
                    let body = live
                        .bodies
                        .iter()
                        .find(|body| body.key.instance_id == instance_id)?;
                    let authored = placed(project, live.q0rg_id, instance_id, frame)?;
                    let drawn = state.effective_affine(body.key, Affine::from_transform(authored));
                    Some(q0physics::attachment_point(
                        body.start_transform,
                        body.start_pose,
                        drawn,
                        point,
                    ))
                };
                let lines = live.world.link_lines_to(
                    index,
                    anchor(link.from, link.from_anchor),
                    anchor(link.to, link.to_anchor),
                );
                let strokes = q0physics::links::link_strokes(&lines, link.look, link.width);
                if !strokes.is_empty() {
                    draws.links.push((strokes, link.color));
                }
            }
        }
        for system in self.emitters.values() {
            let look = system.emitter().look;
            draws
                .particles
                .extend(system.draws().into_iter().map(|draw| ParticleDraw {
                    look,
                    position: draw.position,
                    style: draw.style,
                }));
        }
        draws
    }
}

/// Carries out one `q0light.*!` command. Scripts change a copy of the
/// lights kept with the scene's runtime state; a change takes a light over
/// from its keys.
pub(crate) fn light(
    command: &LightCommand,
    project: &ProjectV2,
    q0rg_id: u16,
    scene_state: &mut RuntimeSceneState,
) -> Result<(), String> {
    let lighting = scene_state
        .lighting
        .get_or_insert_with(|| project.runtime.lighting.clone());
    let name = match command {
        LightCommand::Darkness { value } => {
            lighting.setup_mut(q0rg_id).darkness = (*value as f32).clamp(0.0, 1.0);
            return Ok(());
        }
        LightCommand::On { light }
        | LightCommand::Off { light }
        | LightCommand::Move { light, .. }
        | LightCommand::Color { light, .. }
        | LightCommand::Brightness { light, .. }
        | LightCommand::Size { light, .. } => light,
    };
    let light = lighting
        .setups
        .iter_mut()
        .find(|setup| setup.q0rg_id == q0rg_id)
        .and_then(|setup| setup.lights.iter_mut().find(|light| light.name == *name))
        .ok_or_else(|| format!("no light is called \"{name}\": name it in the light editor"))?;
    let channel = |value: f64| value.round().clamp(0.0, 255.0) as u8;
    match command {
        LightCommand::On { .. } => light.enabled = true,
        LightCommand::Off { .. } => light.enabled = false,
        LightCommand::Move { x, y, .. } => {
            light.keys.clear();
            light.follow = None;
            light.position = [*x as f32, *y as f32];
        }
        LightCommand::Color { r, g, b, .. } => {
            light.keys.clear();
            light.color = Rgba {
                r: channel(*r),
                g: channel(*g),
                b: channel(*b),
                a: 255,
            };
        }
        LightCommand::Brightness { value, .. } => {
            light.keys.clear();
            light.intensity = (*value as f32).clamp(0.0, 8.0);
        }
        LightCommand::Size { radius, .. } => {
            light.keys.clear();
            light.radius = (*radius as f32).clamp(0.0, 100_000.0);
        }
        LightCommand::Darkness { .. } => unreachable!("handled above"),
    }
    Ok(())
}

/// Where `body`'s collider sits on an object with `bounds` placed by
/// `transform`.
fn collider_pose(body: &PhysicsBody, bounds: [f32; 4], transform: Transform2D) -> BodyPose {
    let affine = Affine::from_transform(transform);
    body_collider(body, fit_bounds(bounds, affine), affine, &[]).0
}

/// Keeps an emitter that follows an object on that object, wherever the
/// timeline, physics or a script has put it.
fn follow(
    system: &mut ParticleSystem,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    scene_state: &RuntimeSceneState,
) {
    if let Some(instance_id) = system.emitter().follow {
        if let Some(anchor) = follow_point(project, q0rg_id, frame, instance_id, scene_state, None)
        {
            system.set_anchor(anchor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::particles::ParticleEmitter;
    use q0s_format::physics::{PhysicsBody, PhysicsScene};
    use q0s_format::v2::{Layer, Placement, ProjectMeta, Q0rg, Target, Tween};

    fn project() -> ProjectV2 {
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "live".into(),
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
            assets: vec![q0s_format::v2::Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 1,
                paths: vec![q0s_format::v2::Path {
                    anchors: [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)]
                        .into_iter()
                        .map(|(x, y)| q0s_format::v2::Anchor {
                            point: q0s_format::v2::Vec2::new(x, y),
                            in_handle: None,
                            out_handle: None,
                        })
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
            })],
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
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "ball".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![Placement {
                        pivot: None,
                        instance_id: 9,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            tx: 200.0,
                            ty: 50.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        let mut scene = PhysicsScene::new(1);
        scene.live = true;
        project.runtime.physics.scenes.push(scene);
        let mut body = PhysicsBody::new(1, 9);
        body.bounds = Some([-10.0, -10.0, 10.0, 10.0]);
        project.runtime.physics.bodies.push(body);
        project
            .runtime
            .instance_names
            .insert(InstanceKey::new(1, 9), "ball".to_string());
        let mut emitter = ParticleEmitter::new(1, 1, [100.0, 100.0]);
        emitter.name = "sparks".into();
        emitter.live = true;
        project.runtime.particles.emitters.push(emitter);
        project
    }

    #[test]
    fn a_started_world_drops_its_bodies_and_reset_hands_them_back() {
        let project = project();
        let mut live = Live::default();
        let mut scene = RuntimeSceneState::new();
        let key = InstanceKey::new(1, 9);
        live.physics(&PhysicsCommand::Start, &project, 1, 0, &mut scene)
            .unwrap();
        for _ in 0..12 {
            live.step(&project, 1, 0, &mut scene).unwrap();
        }
        let fallen = scene.effective_transform(key, Transform2D::IDENTITY);
        assert!(fallen.ty > 80.0, "it falls: {fallen:?}");
        live.physics(
            &PhysicsCommand::Push {
                body: "ball".into(),
                x: 0.0,
                y: -2000.0,
            },
            &project,
            1,
            0,
            &mut scene,
        )
        .unwrap();
        live.step(&project, 1, 0, &mut scene).unwrap();
        let pushed = scene.effective_transform(key, Transform2D::IDENTITY);
        assert!(pushed.ty < fallen.ty, "a push sends it back up");
        assert!(live
            .physics(
                &PhysicsCommand::Push {
                    body: "nobody".into(),
                    x: 0.0,
                    y: 0.0
                },
                &project,
                1,
                0,
                &mut scene
            )
            .is_err());
        live.physics(&PhysicsCommand::Reset, &project, 1, 0, &mut scene)
            .unwrap();
        assert!(scene.transform_override(key).is_none());
        assert!(!live.is_active());
    }

    #[test]
    fn emitters_wait_for_a_script_then_spray() {
        let project = project();
        let mut live = Live::default();
        let mut scene = RuntimeSceneState::new();
        live.particles(
            &ParticlesCommand::Burst {
                emitter: "sparks".into(),
                count: 7.0,
            },
            &project,
            1,
            0,
            &scene,
        )
        .unwrap();
        assert_eq!(live.draws().particles.len(), 7);
        live.particles(
            &ParticlesCommand::Start {
                emitter: "sparks".into(),
            },
            &project,
            1,
            0,
            &scene,
        )
        .unwrap();
        for _ in 0..24 {
            live.step(&project, 1, 0, &mut scene).unwrap();
        }
        assert!(live.draws().particles.len() > 7);
        assert!(live
            .particles(
                &ParticlesCommand::Start {
                    emitter: "smoke".into()
                },
                &project,
                1,
                0,
                &scene
            )
            .is_err());
    }

    #[test]
    fn scripts_dim_move_and_switch_off_lights() {
        let mut project = project();
        let setup = project.runtime.lighting.setup_mut(1);
        setup.darkness = 1.0;
        let mut lamp = q0s_format::lighting::Light::new(1, [50.0, 50.0]);
        lamp.name = "lamp".into();
        lamp.set_key(0);
        setup.lights.push(lamp);
        let mut scene = RuntimeSceneState::new();
        light(
            &LightCommand::Move {
                light: "lamp".into(),
                x: 120.0,
                y: 80.0,
            },
            &project,
            1,
            &mut scene,
        )
        .unwrap();
        light(
            &LightCommand::Darkness { value: 0.25 },
            &project,
            1,
            &mut scene,
        )
        .unwrap();
        let lit = q0s_format::raster::frame_lighting(&project, 1, 0, &scene).unwrap();
        assert_eq!(
            lit.lights[0].position,
            [120.0, 80.0],
            "a move takes over from keys"
        );
        assert!(lit.shade[0] > 0.7, "less dark: {:?}", lit.shade);
        light(
            &LightCommand::Off {
                light: "lamp".into(),
            },
            &project,
            1,
            &mut scene,
        )
        .unwrap();
        let lit = q0s_format::raster::frame_lighting(&project, 1, 0, &scene).unwrap();
        assert!(lit.lights.is_empty());
        assert!(light(
            &LightCommand::On {
                light: "nobody".into()
            },
            &project,
            1,
            &mut scene
        )
        .is_err());
    }

    #[test]
    fn a_following_emitter_rides_on_a_body_physics_moves() {
        let mut project = project();
        let emitter = &mut project.runtime.particles.emitters[0];
        emitter.follow = Some(9);
        emitter.position = [0.0, 0.0];
        emitter.space = q0s_format::particles::ParticleSpace::Relative;
        emitter.speed = q0s_format::particles::Varied::fixed(0.0);
        emitter.gravity = [0.0, 0.0];
        emitter.rate = 0.0;
        emitter.lifetime = q0s_format::particles::Varied::fixed(30.0);
        let mut live = Live::default();
        let mut scene = RuntimeSceneState::new();
        live.particles(
            &ParticlesCommand::Burst {
                emitter: "sparks".into(),
                count: 1.0,
            },
            &project,
            1,
            0,
            &scene,
        )
        .unwrap();
        let [x, y] = live.draws().particles[0].position;
        assert!((x - 200.0).abs() < 0.5 && (y - 50.0).abs() < 0.5, "{x} {y}");
        live.physics(&PhysicsCommand::Start, &project, 1, 0, &mut scene)
            .unwrap();
        for _ in 0..12 {
            live.step(&project, 1, 0, &mut scene).unwrap();
        }
        let ball = scene.effective_transform(InstanceKey::new(1, 9), Transform2D::IDENTITY);
        let [x, y] = live.draws().particles[0].position;
        assert!(ball.ty > 80.0);
        assert!((y - ball.ty).abs() < 1.0, "{y} follows {}", ball.ty);
        assert!((x - ball.tx).abs() < 1.0);
    }

    #[test]
    fn a_rope_holds_a_body_until_a_script_cuts_it() {
        let mut project = project();
        let mut rope = PhysicsLink::new(1, 1, q0s_format::physics::LinkKind::Rope, 0, 9);
        rope.name = "rope".into();
        rope.from_anchor = [200.0, 0.0];
        rope.to_anchor = [0.0, -10.0];
        rope.length = 40.0;
        project.runtime.physics.links.push(rope);
        let mut live = Live::default();
        let mut scene = RuntimeSceneState::new();
        let key = InstanceKey::new(1, 9);
        live.physics(&PhysicsCommand::Start, &project, 1, 0, &mut scene)
            .unwrap();
        for _ in 0..24 {
            live.step(&project, 1, 0, &mut scene).unwrap();
        }
        let held = scene.effective_transform(key, Transform2D::IDENTITY);
        assert!((held.ty - 50.0).abs() < 4.0, "hangs on the rope: {held:?}");
        assert_eq!(live.draws().links.len(), 1, "the rope is drawn");
        live.physics(
            &PhysicsCommand::Cut {
                link: "rope".into(),
            },
            &project,
            1,
            0,
            &mut scene,
        )
        .unwrap();
        for _ in 0..12 {
            live.step(&project, 1, 0, &mut scene).unwrap();
        }
        let fallen = scene.effective_transform(key, Transform2D::IDENTITY);
        assert!(fallen.ty > held.ty + 30.0, "falls once cut: {fallen:?}");
        assert!(live
            .physics(
                &PhysicsCommand::Cut {
                    link: "nothing".into()
                },
                &project,
                1,
                0,
                &mut scene
            )
            .is_err());
    }
}
