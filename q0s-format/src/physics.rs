//! Physics authored in the editor.
//!
//! A simulation runs in the editor and is baked into ordinary keyframes, so
//! players only ever play keys back. What is stored here is the setup, kept
//! so a scene can be changed and baked again.

use crate::error::Error;
use crate::io::{write_string_u16, Cursor};
use crate::v2::Rgba;

/// Stage edges that act as walls, as bits of [`PhysicsScene::walls`].
pub const WALL_LEFT: u8 = 1;
pub const WALL_RIGHT: u8 = 2;
pub const WALL_TOP: u8 = 4;
pub const WALL_BOTTOM: u8 = 8;
pub const WALLS_ALL: u8 = WALL_LEFT | WALL_RIGHT | WALL_TOP | WALL_BOTTOM;

/// Collision groups a body can join. Group 0 collides with nothing.
pub const MAX_COLLISION_GROUP: u8 = 31;
/// The most points a body's outline may hold, over all its parts.
pub const MAX_OUTLINE_POINTS: usize = 4096;

/// Opens a v41 physics block written with a layout version; the first
/// development layout started straight with the scene count.
const LAYOUT_MARK: u16 = u16::MAX;
/// 2 added collider extras to bodies, 3 added links.
const LAYOUT_VERSION: u8 = 3;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicsData {
    /// One world per timeline that has physics.
    pub scenes: Vec<PhysicsScene>,
    pub bodies: Vec<PhysicsBody>,
    pub liquids: Vec<PhysicsLiquid>,
    /// Ropes, springs, hinges and welds tying bodies together.
    pub links: Vec<PhysicsLink>,
}

impl PhysicsData {
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
            && self.bodies.is_empty()
            && self.liquids.is_empty()
            && self.links.is_empty()
    }

    /// Whether anything here needs q1s v41: liquids, links, worlds q0player
    /// runs live, collider bounds handed to it, or colliders shaped beyond a
    /// plain box or circle.
    pub fn uses_v41(&self) -> bool {
        !self.liquids.is_empty()
            || !self.links.is_empty()
            || self.scenes.iter().any(|scene| scene.live)
            || self
                .bodies
                .iter()
                .any(|body| body.bounds.is_some() || body.has_collider_extras())
    }

    /// Keeps only the worlds q0player runs live, with their bodies and
    /// liquids. Baked physics is plain keyframes and needs none of this.
    pub fn retain_live(&mut self) {
        let live = self
            .scenes
            .iter()
            .filter(|scene| scene.live)
            .map(|scene| scene.q0rg_id)
            .collect::<std::collections::HashSet<_>>();
        self.scenes.retain(|scene| live.contains(&scene.q0rg_id));
        self.bodies.retain(|body| live.contains(&body.q0rg_id));
        self.liquids.retain(|liquid| live.contains(&liquid.q0rg_id));
        self.links.retain(|link| live.contains(&link.q0rg_id));
    }

    pub fn link(&self, q0rg_id: u16, link_id: u16) -> Option<&PhysicsLink> {
        self.links
            .iter()
            .find(|link| link.q0rg_id == q0rg_id && link.link_id == link_id)
    }

    pub fn link_mut(&mut self, q0rg_id: u16, link_id: u16) -> Option<&mut PhysicsLink> {
        self.links
            .iter_mut()
            .find(|link| link.q0rg_id == q0rg_id && link.link_id == link_id)
    }

    /// The lowest link id not used in `q0rg_id` yet.
    pub fn next_link_id(&self, q0rg_id: u16) -> u16 {
        (1..=u16::MAX)
            .find(|id| self.link(q0rg_id, *id).is_none())
            .unwrap_or(u16::MAX)
    }

    pub fn liquid_mut(&mut self, q0rg_id: u16, liquid_id: u16) -> Option<&mut PhysicsLiquid> {
        self.liquids
            .iter_mut()
            .find(|liquid| liquid.q0rg_id == q0rg_id && liquid.liquid_id == liquid_id)
    }

    /// The lowest liquid id not used in `q0rg_id` yet.
    pub fn next_liquid_id(&self, q0rg_id: u16) -> u16 {
        (1..=u16::MAX)
            .find(|id| {
                !self
                    .liquids
                    .iter()
                    .any(|liquid| liquid.q0rg_id == q0rg_id && liquid.liquid_id == *id)
            })
            .unwrap_or(u16::MAX)
    }

    pub fn scene(&self, q0rg_id: u16) -> Option<&PhysicsScene> {
        self.scenes.iter().find(|scene| scene.q0rg_id == q0rg_id)
    }

    /// The scene of `q0rg_id`, made with defaults when it has none yet.
    pub fn scene_mut(&mut self, q0rg_id: u16) -> &mut PhysicsScene {
        let index = match self
            .scenes
            .iter()
            .position(|scene| scene.q0rg_id == q0rg_id)
        {
            Some(index) => index,
            None => {
                self.scenes.push(PhysicsScene::new(q0rg_id));
                self.scenes.len() - 1
            }
        };
        &mut self.scenes[index]
    }

    pub fn body(&self, q0rg_id: u16, instance_id: u32) -> Option<&PhysicsBody> {
        self.bodies
            .iter()
            .find(|body| body.q0rg_id == q0rg_id && body.instance_id == instance_id)
    }

    pub fn body_mut(&mut self, q0rg_id: u16, instance_id: u32) -> Option<&mut PhysicsBody> {
        self.bodies
            .iter_mut()
            .find(|body| body.q0rg_id == q0rg_id && body.instance_id == instance_id)
    }

    /// Keeps every value in range and drops entries that cannot be told
    /// apart, so a hand-edited or damaged file still simulates sanely.
    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.scenes.retain(|scene| seen.insert(scene.q0rg_id));
        for scene in &mut self.scenes {
            scene.sanitize();
        }
        let mut seen = std::collections::HashSet::new();
        self.bodies
            .retain(|body| body.instance_id != 0 && seen.insert((body.q0rg_id, body.instance_id)));
        for body in &mut self.bodies {
            body.sanitize();
        }
        let mut seen = std::collections::HashSet::new();
        self.liquids
            .retain(|liquid| seen.insert((liquid.q0rg_id, liquid.liquid_id)));
        for liquid in &mut self.liquids {
            liquid.sanitize();
        }
        let mut seen = std::collections::HashSet::new();
        self.links.retain(|link| {
            link.to != 0 && link.from != link.to && seen.insert((link.q0rg_id, link.link_id))
        });
        for link in &mut self.links {
            link.sanitize();
        }
    }
}

/// How one timeline simulates.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsScene {
    pub q0rg_id: u16,
    /// Bodies start where the timeline has them on this frame...
    pub start_frame: u16,
    /// ...and every later frame up to this one is simulated and keyed.
    pub end_frame: u16,
    /// Stage pixels per second squared; positive y points down.
    pub gravity: [f32; 2],
    /// Slowing every body feels from the air, per second.
    pub air_drag: f32,
    /// Solver quality, in multiples of 60 steps per simulated second. Four
    /// targets 240 Hz regardless of movie fps; frames are subdivided evenly.
    /// More keep fast bodies from tunnelling without changing gravity or speed.
    pub substeps: u8,
    /// Speeds the world up or slows it down without changing the frame rate.
    pub time_scale: f32,
    /// Stage edges that act as walls, as `WALL_*` bits.
    pub walls: u8,
    pub wall_bounce: f32,
    pub wall_friction: f32,
    /// q0player can run this world live, driven by q0lang (`q0physics.*!`).
    pub live: bool,
}

impl PhysicsScene {
    pub fn new(q0rg_id: u16) -> Self {
        Self {
            q0rg_id,
            start_frame: 0,
            end_frame: 47,
            gravity: [0.0, 980.0],
            air_drag: 0.0,
            substeps: 4,
            time_scale: 1.0,
            walls: WALL_BOTTOM | WALL_LEFT | WALL_RIGHT,
            wall_bounce: 0.3,
            wall_friction: 0.5,
            live: false,
        }
    }

    fn sanitize(&mut self) {
        if self.end_frame < self.start_frame {
            std::mem::swap(&mut self.start_frame, &mut self.end_frame);
        }
        for axis in &mut self.gravity {
            *axis = finite_clamp(*axis, -100_000.0, 100_000.0, 0.0);
        }
        self.air_drag = finite_clamp(self.air_drag, 0.0, 100.0, 0.0);
        self.substeps = self.substeps.clamp(1, 16);
        self.time_scale = finite_clamp(self.time_scale, 0.05, 8.0, 1.0);
        self.walls &= WALLS_ALL;
        self.wall_bounce = finite_clamp(self.wall_bounce, 0.0, 1.0, 0.3);
        self.wall_friction = finite_clamp(self.wall_friction, 0.0, 4.0, 0.5);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyKind {
    /// Falls, bounces and gets pushed around.
    #[default]
    Dynamic,
    /// Stays where it is on the start frame; others hit it.
    Static,
    /// Moves the way the timeline animates it and pushes others aside.
    Animated,
}

impl BodyKind {
    pub const ALL: [Self; 3] = [Self::Dynamic, Self::Static, Self::Animated];

    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Dynamic => 0,
            Self::Static => 1,
            Self::Animated => 2,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Dynamic),
            1 => Some(Self::Static),
            2 => Some(Self::Animated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColliderShape {
    /// The object's bounds, turning with it.
    #[default]
    Box,
    /// A circle as wide as the longer side of the object's bounds.
    Circle,
    /// A pill along the longer side of the object's bounds: rolls on its
    /// round ends, rests on its flat sides. Good for characters.
    Capsule,
    /// Shrink-wrapped around the object's drawn points.
    Hull,
    /// The object's exact outline, hollows and all, as convex parts.
    Outline,
}

impl ColliderShape {
    pub const ALL: [Self; 5] = [
        Self::Box,
        Self::Circle,
        Self::Capsule,
        Self::Hull,
        Self::Outline,
    ];

    /// Shapes taken from the artwork's own points.
    pub const fn follows_artwork(self) -> bool {
        matches!(self, Self::Hull | Self::Outline)
    }

    /// What q1s v40 already knew.
    const fn is_v40(self) -> bool {
        matches!(self, Self::Box | Self::Circle)
    }

    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Box => 0,
            Self::Circle => 1,
            Self::Capsule => 2,
            Self::Hull => 3,
            Self::Outline => 4,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Box),
            1 => Some(Self::Circle),
            2 => Some(Self::Capsule),
            3 => Some(Self::Hull),
            4 => Some(Self::Outline),
            _ => None,
        }
    }
}

/// How a body is moving on the start frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Momentum {
    /// Carry on the way the timeline moved it into the start frame.
    #[default]
    FromMotion,
    /// Stage pixels per second, and degrees per second clockwise.
    Manual { velocity: [f32; 2], spin: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsBody {
    pub q0rg_id: u16,
    /// The placed object, the same instance across its keyframes.
    pub instance_id: u32,
    pub kind: BodyKind,
    pub shape: ColliderShape,
    /// Grows or shrinks the collider around the object's bounds.
    pub shape_scale: f32,
    /// Stretches the collider's width and height on top of `shape_scale`.
    pub size: [f32; 2],
    /// Moves the collider from the middle of the object, in stage pixels
    /// along the object's own axes.
    pub offset: [f32; 2],
    /// Degrees the collider is turned against the object.
    pub turn: f32,
    /// Hull and outline colliders take the artwork's shape again on every
    /// frame, so a morphing or animated drawing collides as it looks.
    pub follow_art: bool,
    /// Hull and outline colliders as convex parts in the object's own
    /// space. Written for q0player; the editor measures the artwork itself.
    pub outline: Vec<Vec<[f32; 2]>>,
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
    pub fixed_rotation: bool,
    /// Bodies collide only with bodies of the same group; 0 is none.
    pub group: u8,
    pub momentum: Momentum,
    /// The object's bounds in its own space, min x, min y, max x, max y.
    /// Written for q0player, which has no way to measure artwork; the
    /// editor measures it itself.
    pub bounds: Option<[f32; 4]>,
}

impl PhysicsBody {
    pub fn new(q0rg_id: u16, instance_id: u32) -> Self {
        Self {
            q0rg_id,
            instance_id,
            kind: BodyKind::Dynamic,
            shape: ColliderShape::Box,
            shape_scale: 1.0,
            size: [1.0, 1.0],
            offset: [0.0, 0.0],
            turn: 0.0,
            follow_art: false,
            outline: Vec::new(),
            mass: 1.0,
            bounce: 0.3,
            friction: 0.5,
            linear_damping: 0.0,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            fixed_rotation: false,
            group: 1,
            momentum: Momentum::FromMotion,
            bounds: None,
        }
    }

    /// Whether the collider is shaped beyond what q1s v40 could store.
    pub fn has_collider_extras(&self) -> bool {
        !self.shape.is_v40()
            || self.size != [1.0, 1.0]
            || self.offset != [0.0, 0.0]
            || self.turn != 0.0
            || self.follow_art
            || !self.outline.is_empty()
    }

    fn sanitize(&mut self) {
        self.shape_scale = finite_clamp(self.shape_scale, 0.05, 20.0, 1.0);
        for value in &mut self.size {
            *value = finite_clamp(*value, 0.05, 20.0, 1.0);
        }
        for value in &mut self.offset {
            *value = finite_clamp(*value, -100_000.0, 100_000.0, 0.0);
        }
        self.turn = finite_clamp(self.turn, -36_000.0, 36_000.0, 0.0);
        self.outline
            .retain(|part| part.len() >= 3 && part.iter().flatten().all(|value| value.is_finite()));
        let mut points = 0;
        self.outline.retain(|part| {
            points += part.len();
            points <= MAX_OUTLINE_POINTS
        });
        self.mass = finite_clamp(self.mass, 0.001, 100_000.0, 1.0);
        self.bounce = finite_clamp(self.bounce, 0.0, 1.0, 0.3);
        self.friction = finite_clamp(self.friction, 0.0, 4.0, 0.5);
        self.linear_damping = finite_clamp(self.linear_damping, 0.0, 100.0, 0.0);
        self.angular_damping = finite_clamp(self.angular_damping, 0.0, 100.0, 0.05);
        self.gravity_scale = finite_clamp(self.gravity_scale, -20.0, 20.0, 1.0);
        self.group = self.group.min(MAX_COLLISION_GROUP);
        if self.bounds.is_some_and(|[min_x, min_y, max_x, max_y]| {
            ![min_x, min_y, max_x, max_y].iter().all(|v| v.is_finite())
                || max_x < min_x
                || max_y < min_y
        }) {
            self.bounds = None;
        }
        if let Momentum::Manual { velocity, spin } = &mut self.momentum {
            for axis in velocity.iter_mut() {
                *axis = finite_clamp(*axis, -1_000_000.0, 1_000_000.0, 0.0);
            }
            *spin = finite_clamp(*spin, -100_000.0, 100_000.0, 0.0);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiquidSource {
    /// The region is full of liquid on the start frame.
    #[default]
    Block,
    /// Liquid pours out of the region's middle for as long as it runs.
    Stream,
}

impl LiquidSource {
    pub const ALL: [Self; 2] = [Self::Block, Self::Stream];

    const fn code(self) -> u8 {
        match self {
            Self::Block => 0,
            Self::Stream => 1,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Block),
            1 => Some(Self::Stream),
            _ => None,
        }
    }
}

/// A body of liquid: many small particles that push apart and cling
/// together, baked into one filled drawing per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsLiquid {
    pub q0rg_id: u16,
    pub liquid_id: u16,
    pub name: String,
    pub source: LiquidSource,
    /// Stage x, y, width, height.
    pub region: [f32; 4],
    /// Pixels per second the stream leaves at.
    pub stream_velocity: [f32; 2],
    /// Particles a second a stream adds.
    pub stream_rate: f32,
    pub max_particles: u16,
    /// Gap between particles at rest, in pixels: the liquid's resolution.
    pub spacing: f32,
    pub color: Rgba,
    /// How thick it flows: 0 is water, higher is honey.
    pub viscosity: f32,
    /// How hard it resists being squeezed.
    pub stiffness: f32,
    /// How much it holds together in drops instead of spraying apart.
    pub cohesion: f32,
    pub gravity_scale: f32,
    /// How hard it pushes floating bodies around; 0 leaves them alone.
    pub buoyancy: f32,
    /// Collides with bodies of this group and with the walls; 0 is walls
    /// only.
    pub group: u8,
    /// How round and merged the drawn surface looks around each particle.
    pub smoothness: f32,
    /// The layer the last bake drew into, reused by the next bake.
    pub layer_id: Option<u16>,
}

impl PhysicsLiquid {
    pub fn new(q0rg_id: u16, liquid_id: u16, region: [f32; 4]) -> Self {
        Self {
            q0rg_id,
            liquid_id,
            name: format!("liquid{liquid_id}"),
            source: LiquidSource::Block,
            region,
            stream_velocity: [0.0, 200.0],
            stream_rate: 120.0,
            max_particles: 1500,
            spacing: 8.0,
            color: Rgba {
                r: 60,
                g: 140,
                b: 230,
                a: 220,
            },
            viscosity: 0.1,
            stiffness: 1.0,
            cohesion: 0.5,
            gravity_scale: 1.0,
            buoyancy: 0.5,
            group: 1,
            smoothness: 1.0,
            layer_id: None,
        }
    }

    fn sanitize(&mut self) {
        for value in &mut self.region {
            *value = finite_clamp(*value, -100_000.0, 100_000.0, 0.0);
        }
        self.region[2] = self.region[2].max(1.0);
        self.region[3] = self.region[3].max(1.0);
        for value in &mut self.stream_velocity {
            *value = finite_clamp(*value, -100_000.0, 100_000.0, 0.0);
        }
        self.stream_rate = finite_clamp(self.stream_rate, 0.0, 10_000.0, 120.0);
        self.max_particles = self.max_particles.clamp(1, 20_000);
        self.spacing = finite_clamp(self.spacing, 1.0, 200.0, 8.0);
        self.viscosity = finite_clamp(self.viscosity, 0.0, 20.0, 0.1);
        self.stiffness = finite_clamp(self.stiffness, 0.05, 20.0, 1.0);
        self.cohesion = finite_clamp(self.cohesion, 0.0, 5.0, 0.5);
        self.gravity_scale = finite_clamp(self.gravity_scale, -20.0, 20.0, 1.0);
        self.buoyancy = finite_clamp(self.buoyancy, 0.0, 10.0, 0.5);
        self.group = self.group.min(MAX_COLLISION_GROUP);
        self.smoothness = finite_clamp(self.smoothness, 0.3, 4.0, 1.0);
        if self.name.chars().count() > 64 {
            self.name = self.name.chars().take(64).collect();
        }
    }
}

/// How a link holds two objects together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkKind {
    /// A rope or chain: never longer than its length, slack when shorter.
    /// It sags, swings and drags what hangs on it along.
    #[default]
    Rope,
    /// A springy band that pulls back to its length and bobs about it.
    Spring,
    /// A pin through both objects: the tied one swings around it.
    Hinge,
    /// Glued where they touch; a less rigid weld wobbles like jelly.
    Weld,
}

impl LinkKind {
    pub const ALL: [Self; 4] = [Self::Rope, Self::Spring, Self::Hinge, Self::Weld];

    /// Kinds that run from one point to another and have a length.
    pub const fn spans(self) -> bool {
        matches!(self, Self::Rope | Self::Spring)
    }

    const fn code(self) -> u8 {
        match self {
            Self::Rope => 0,
            Self::Spring => 1,
            Self::Hinge => 2,
            Self::Weld => 3,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Rope),
            1 => Some(Self::Spring),
            2 => Some(Self::Hinge),
            3 => Some(Self::Weld),
            _ => None,
        }
    }
}

/// How a link is drawn in the movie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkLook {
    /// Not drawn: the objects just behave as if tied.
    Hidden,
    /// A smooth line.
    #[default]
    Line,
    /// Oval links, every other one seen edge on.
    Chain,
    /// A zigzag coil.
    Coil,
}

impl LinkLook {
    pub const ALL: [Self; 4] = [Self::Hidden, Self::Line, Self::Chain, Self::Coil];

    const fn code(self) -> u8 {
        match self {
            Self::Hidden => 0,
            Self::Line => 1,
            Self::Chain => 2,
            Self::Coil => 3,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Hidden),
            1 => Some(Self::Line),
            2 => Some(Self::Chain),
            3 => Some(Self::Coil),
            _ => None,
        }
    }
}

/// Ties object `to` to object `from`, or to a point on the stage.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsLink {
    pub q0rg_id: u16,
    pub link_id: u16,
    /// What scripts and the baked layer call it.
    pub name: String,
    pub kind: LinkKind,
    /// The object it hangs from; 0 ties it to the stage.
    pub from: u32,
    /// The object tied on.
    pub to: u32,
    /// Where a rope or spring holds `from`: stage pixels from the middle of
    /// its hitbox along the object's own axes, or a stage point when `from`
    /// is 0. Hinges and welds hold both objects at `to_anchor`.
    pub from_anchor: [f32; 2],
    /// Where it holds `to`, from the middle of its hitbox along its axes.
    pub to_anchor: [f32; 2],
    /// Stage pixels: how long a rope is, or how long a spring rests.
    pub length: f32,
    /// 0 to 1. Ropes: floppy string to stiff wire. Springs: how hard they
    /// pull. Hinges: how hard they swing back. Welds: jelly to solid.
    pub stiffness: f32,
    /// 0 to 1: how fast wobbling and swinging die down.
    pub damping: f32,
    /// 0 to 1: how far a rope gives when pulled, like a bungee.
    pub stretch: f32,
    /// Pieces a rope sags and swings in; 0 keeps it a straight tether.
    pub segments: u8,
    /// How heavy the whole rope is, like a body's mass.
    pub mass: f32,
    /// Hinges swing at most this many degrees either way; 0 is free.
    pub swing_limit: f32,
    /// It snaps when pulled harder than this; 0 never snaps.
    pub break_force: f32,
    /// The two objects still bump into each other.
    pub collide: bool,
    /// Rope pieces collide with bodies of this group and the walls; 0 none.
    pub group: u8,
    pub look: LinkLook,
    pub color: Rgba,
    /// Stage pixels.
    pub width: f32,
    /// The layer the last bake drew into, reused by the next bake.
    pub layer_id: Option<u16>,
}

impl PhysicsLink {
    pub fn new(q0rg_id: u16, link_id: u16, kind: LinkKind, from: u32, to: u32) -> Self {
        let name = match kind {
            LinkKind::Rope => "rope",
            LinkKind::Spring => "spring",
            LinkKind::Hinge => "hinge",
            LinkKind::Weld => "weld",
        };
        Self {
            q0rg_id,
            link_id,
            name: format!("{name}{link_id}"),
            kind,
            from,
            to,
            from_anchor: [0.0, 0.0],
            to_anchor: [0.0, 0.0],
            length: 100.0,
            stiffness: match kind {
                LinkKind::Rope => 0.0,
                LinkKind::Spring => 0.4,
                LinkKind::Hinge => 0.0,
                LinkKind::Weld => 0.6,
            },
            damping: 0.15,
            stretch: 0.0,
            segments: if kind == LinkKind::Rope { 10 } else { 0 },
            mass: 0.2,
            swing_limit: 0.0,
            break_force: 0.0,
            collide: false,
            group: 0,
            look: match kind {
                LinkKind::Rope => LinkLook::Line,
                LinkKind::Spring => LinkLook::Coil,
                LinkKind::Hinge | LinkKind::Weld => LinkLook::Hidden,
            },
            color: Rgba {
                r: 90,
                g: 70,
                b: 50,
                a: 255,
            },
            width: 3.0,
            layer_id: None,
        }
    }

    /// Whether a bake draws it.
    pub fn is_drawn(&self) -> bool {
        self.kind.spans() && self.look != LinkLook::Hidden
    }

    fn sanitize(&mut self) {
        for value in self.from_anchor.iter_mut().chain(&mut self.to_anchor) {
            *value = finite_clamp(*value, -100_000.0, 100_000.0, 0.0);
        }
        self.length = finite_clamp(self.length, 1.0, 100_000.0, 100.0);
        self.stiffness = finite_clamp(self.stiffness, 0.0, 1.0, 0.0);
        self.damping = finite_clamp(self.damping, 0.0, 1.0, 0.15);
        self.stretch = finite_clamp(self.stretch, 0.0, 1.0, 0.0);
        self.segments = self.segments.min(MAX_LINK_SEGMENTS);
        self.mass = finite_clamp(self.mass, 0.001, 100_000.0, 0.2);
        self.swing_limit = finite_clamp(self.swing_limit, 0.0, 180.0, 0.0);
        self.break_force = finite_clamp(self.break_force, 0.0, 1.0e9, 0.0);
        self.group = self.group.min(MAX_COLLISION_GROUP);
        self.width = finite_clamp(self.width, 0.1, 500.0, 3.0);
        if self.name.chars().count() > 64 {
            self.name = self.name.chars().take(64).collect();
        }
    }
}

/// The most pieces a rope sags in.
pub const MAX_LINK_SEGMENTS: u8 = 64;

/// Writes the physics block. `extended` adds what q1s v41 knows: live
/// worlds, body bounds, liquids and links.
pub(crate) fn write(out: &mut Vec<u8>, data: &PhysicsData, extended: bool) -> Result<(), Error> {
    let count = u16::try_from(data.scenes.len())
        .ok()
        .filter(|count| *count != LAYOUT_MARK)
        .ok_or(Error::Overflow("physics scene count exceeds u16"))?;
    if extended {
        out.extend_from_slice(&LAYOUT_MARK.to_le_bytes());
        out.push(LAYOUT_VERSION);
    }
    out.extend_from_slice(&count.to_le_bytes());
    for scene in &data.scenes {
        out.extend_from_slice(&scene.q0rg_id.to_le_bytes());
        out.extend_from_slice(&scene.start_frame.to_le_bytes());
        out.extend_from_slice(&scene.end_frame.to_le_bytes());
        for value in [scene.gravity[0], scene.gravity[1], scene.air_drag] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(scene.substeps);
        out.extend_from_slice(&scene.time_scale.to_le_bytes());
        out.push(scene.walls);
        out.extend_from_slice(&scene.wall_bounce.to_le_bytes());
        out.extend_from_slice(&scene.wall_friction.to_le_bytes());
        if extended {
            out.push(u8::from(scene.live));
        }
    }
    let count = u16::try_from(data.bodies.len())
        .map_err(|_| Error::Overflow("physics body count exceeds u16"))?;
    out.extend_from_slice(&count.to_le_bytes());
    for body in &data.bodies {
        out.extend_from_slice(&body.q0rg_id.to_le_bytes());
        out.extend_from_slice(&body.instance_id.to_le_bytes());
        out.push(body.kind.code());
        // A v40 file cannot name the newer shapes; the writer only picks
        // v40 when no body uses them, so this is a safety net.
        let shape = if extended || body.shape.is_v40() {
            body.shape
        } else {
            ColliderShape::Box
        };
        out.push(shape.code());
        for value in [
            body.shape_scale,
            body.mass,
            body.bounce,
            body.friction,
            body.linear_damping,
            body.angular_damping,
            body.gravity_scale,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(u8::from(body.fixed_rotation));
        out.push(body.group);
        match body.momentum {
            Momentum::FromMotion => out.push(0),
            Momentum::Manual { velocity, spin } => {
                out.push(1);
                for value in [velocity[0], velocity[1], spin] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        if extended {
            match body.bounds {
                None => out.push(0),
                Some(bounds) => {
                    out.push(1);
                    for value in bounds {
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            for value in body
                .size
                .iter()
                .chain(&body.offset)
                .chain(std::iter::once(&body.turn))
            {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(u8::from(body.follow_art));
            let parts = u16::try_from(body.outline.len())
                .map_err(|_| Error::Overflow("collider part count exceeds u16"))?;
            out.extend_from_slice(&parts.to_le_bytes());
            for part in &body.outline {
                let points = u16::try_from(part.len())
                    .map_err(|_| Error::Overflow("collider point count exceeds u16"))?;
                out.extend_from_slice(&points.to_le_bytes());
                for value in part.iter().flatten() {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
    if extended {
        let count = u16::try_from(data.liquids.len())
            .map_err(|_| Error::Overflow("liquid count exceeds u16"))?;
        out.extend_from_slice(&count.to_le_bytes());
        for liquid in &data.liquids {
            out.extend_from_slice(&liquid.q0rg_id.to_le_bytes());
            out.extend_from_slice(&liquid.liquid_id.to_le_bytes());
            write_string_u16(out, &liquid.name)?;
            out.push(liquid.source.code());
            for value in liquid
                .region
                .iter()
                .chain(&liquid.stream_velocity)
                .chain(&[liquid.stream_rate])
            {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&liquid.max_particles.to_le_bytes());
            out.extend_from_slice(&liquid.spacing.to_le_bytes());
            out.extend_from_slice(&[
                liquid.color.r,
                liquid.color.g,
                liquid.color.b,
                liquid.color.a,
            ]);
            for value in [
                liquid.viscosity,
                liquid.stiffness,
                liquid.cohesion,
                liquid.gravity_scale,
                liquid.buoyancy,
            ] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(liquid.group);
            out.extend_from_slice(&liquid.smoothness.to_le_bytes());
            match liquid.layer_id {
                None => out.push(0),
                Some(layer_id) => {
                    out.push(1);
                    out.extend_from_slice(&layer_id.to_le_bytes());
                }
            }
        }
        let count = u16::try_from(data.links.len())
            .map_err(|_| Error::Overflow("link count exceeds u16"))?;
        out.extend_from_slice(&count.to_le_bytes());
        for link in &data.links {
            out.extend_from_slice(&link.q0rg_id.to_le_bytes());
            out.extend_from_slice(&link.link_id.to_le_bytes());
            write_string_u16(out, &link.name)?;
            out.push(link.kind.code());
            out.extend_from_slice(&link.from.to_le_bytes());
            out.extend_from_slice(&link.to.to_le_bytes());
            for value in link.from_anchor.iter().chain(&link.to_anchor).chain(&[
                link.length,
                link.stiffness,
                link.damping,
                link.stretch,
            ]) {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(link.segments);
            for value in [link.mass, link.swing_limit, link.break_force] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(u8::from(link.collide));
            out.push(link.group);
            out.push(link.look.code());
            out.extend_from_slice(&[link.color.r, link.color.g, link.color.b, link.color.a]);
            out.extend_from_slice(&link.width.to_le_bytes());
            match link.layer_id {
                None => out.push(0),
                Some(layer_id) => {
                    out.push(1);
                    out.extend_from_slice(&layer_id.to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn read(c: &mut Cursor<'_>, extended: bool) -> Result<PhysicsData, Error> {
    let mut data = PhysicsData::default();
    let mut layout = 1;
    let mut scenes = c.read_u16()?;
    if extended && scenes == LAYOUT_MARK {
        layout = c.read_u8()?;
        if !(2..=LAYOUT_VERSION).contains(&layout) {
            return Err(Error::Validation("unknown physics layout"));
        }
        scenes = c.read_u16()?;
    }
    for _ in 0..scenes {
        data.scenes.push(PhysicsScene {
            q0rg_id: c.read_u16()?,
            start_frame: c.read_u16()?,
            end_frame: c.read_u16()?,
            gravity: [c.read_f32()?, c.read_f32()?],
            air_drag: c.read_f32()?,
            substeps: c.read_u8()?,
            time_scale: c.read_f32()?,
            walls: c.read_u8()?,
            wall_bounce: c.read_f32()?,
            wall_friction: c.read_f32()?,
            live: extended && c.read_u8()? != 0,
        });
    }
    for _ in 0..c.read_u16()? {
        let q0rg_id = c.read_u16()?;
        let instance_id = c.read_u32()?;
        let kind = BodyKind::from_code(c.read_u8()?)
            .ok_or(Error::Validation("unknown physics body kind"))?;
        let shape = ColliderShape::from_code(c.read_u8()?)
            .ok_or(Error::Validation("unknown physics collider shape"))?;
        let mut body = PhysicsBody {
            kind,
            shape,
            shape_scale: c.read_f32()?,
            mass: c.read_f32()?,
            bounce: c.read_f32()?,
            friction: c.read_f32()?,
            linear_damping: c.read_f32()?,
            angular_damping: c.read_f32()?,
            gravity_scale: c.read_f32()?,
            ..PhysicsBody::new(q0rg_id, instance_id)
        };
        body.fixed_rotation = c.read_u8()? != 0;
        body.group = c.read_u8()?;
        body.momentum = match c.read_u8()? {
            0 => Momentum::FromMotion,
            1 => Momentum::Manual {
                velocity: [c.read_f32()?, c.read_f32()?],
                spin: c.read_f32()?,
            },
            _ => return Err(Error::Validation("unknown physics momentum kind")),
        };
        if extended && c.read_u8()? != 0 {
            body.bounds = Some([c.read_f32()?, c.read_f32()?, c.read_f32()?, c.read_f32()?]);
        }
        if extended && layout >= 2 {
            body.size = [c.read_f32()?, c.read_f32()?];
            body.offset = [c.read_f32()?, c.read_f32()?];
            body.turn = c.read_f32()?;
            body.follow_art = c.read_u8()? != 0;
            for _ in 0..c.read_u16()? {
                let points = c.read_u16()?;
                let mut part = Vec::with_capacity(usize::from(points));
                for _ in 0..points {
                    part.push([c.read_f32()?, c.read_f32()?]);
                }
                body.outline.push(part);
            }
        }
        data.bodies.push(body);
    }
    if extended {
        for _ in 0..c.read_u16()? {
            let q0rg_id = c.read_u16()?;
            let liquid_id = c.read_u16()?;
            let name = c.read_string_u16()?;
            let source = LiquidSource::from_code(c.read_u8()?)
                .ok_or(Error::Validation("unknown liquid source"))?;
            let region = [c.read_f32()?, c.read_f32()?, c.read_f32()?, c.read_f32()?];
            let mut liquid = PhysicsLiquid {
                name,
                source,
                stream_velocity: [c.read_f32()?, c.read_f32()?],
                stream_rate: c.read_f32()?,
                max_particles: c.read_u16()?,
                spacing: c.read_f32()?,
                color: Rgba {
                    r: c.read_u8()?,
                    g: c.read_u8()?,
                    b: c.read_u8()?,
                    a: c.read_u8()?,
                },
                viscosity: c.read_f32()?,
                stiffness: c.read_f32()?,
                cohesion: c.read_f32()?,
                gravity_scale: c.read_f32()?,
                buoyancy: c.read_f32()?,
                group: c.read_u8()?,
                smoothness: c.read_f32()?,
                ..PhysicsLiquid::new(q0rg_id, liquid_id, region)
            };
            liquid.layer_id = match c.read_u8()? {
                0 => None,
                _ => Some(c.read_u16()?),
            };
            data.liquids.push(liquid);
        }
    }
    if extended && layout >= 3 {
        for _ in 0..c.read_u16()? {
            let q0rg_id = c.read_u16()?;
            let link_id = c.read_u16()?;
            let name = c.read_string_u16()?;
            let kind =
                LinkKind::from_code(c.read_u8()?).ok_or(Error::Validation("unknown link kind"))?;
            let from = c.read_u32()?;
            let to = c.read_u32()?;
            let mut link = PhysicsLink {
                name,
                from_anchor: [c.read_f32()?, c.read_f32()?],
                to_anchor: [c.read_f32()?, c.read_f32()?],
                length: c.read_f32()?,
                stiffness: c.read_f32()?,
                damping: c.read_f32()?,
                stretch: c.read_f32()?,
                segments: c.read_u8()?,
                mass: c.read_f32()?,
                swing_limit: c.read_f32()?,
                break_force: c.read_f32()?,
                collide: c.read_u8()? != 0,
                group: c.read_u8()?,
                look: LinkLook::from_code(c.read_u8()?)
                    .ok_or(Error::Validation("unknown link look"))?,
                color: Rgba {
                    r: c.read_u8()?,
                    g: c.read_u8()?,
                    b: c.read_u8()?,
                    a: c.read_u8()?,
                },
                width: c.read_f32()?,
                ..PhysicsLink::new(q0rg_id, link_id, kind, from, to)
            };
            link.layer_id = match c.read_u8()? {
                0 => None,
                _ => Some(c.read_u16()?),
            };
            data.links.push(link);
        }
    }
    data.sanitize();
    Ok(data)
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaped_colliders_round_trip_and_need_v41() {
        let mut data = PhysicsData::default();
        let mut body = PhysicsBody::new(1, 7);
        assert!(!body.has_collider_extras());
        body.shape = ColliderShape::Outline;
        body.size = [1.5, 0.5];
        body.offset = [4.0, -2.0];
        body.turn = 30.0;
        body.follow_art = true;
        body.outline = vec![
            vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
            vec![[10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        ];
        data.bodies.push(body);
        assert!(data.uses_v41());
        let mut bytes = Vec::new();
        write(&mut bytes, &data, true).unwrap();
        let mut cursor = Cursor::new(&bytes);
        assert_eq!(read(&mut cursor, true).unwrap(), data);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn links_round_trip_and_need_v41() {
        let mut data = PhysicsData::default();
        let mut link = PhysicsLink::new(1, 3, LinkKind::Rope, 7, 8);
        link.from_anchor = [0.0, 20.0];
        link.to_anchor = [0.0, -15.0];
        link.length = 140.0;
        link.stiffness = 0.3;
        link.stretch = 0.5;
        link.break_force = 900.0;
        link.look = LinkLook::Chain;
        link.layer_id = Some(4);
        data.links.push(link);
        data.links
            .push(PhysicsLink::new(1, 4, LinkKind::Hinge, 0, 8));
        assert!(data.uses_v41());
        let mut bytes = Vec::new();
        write(&mut bytes, &data, true).unwrap();
        let mut cursor = Cursor::new(&bytes);
        assert_eq!(read(&mut cursor, true).unwrap(), data);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn a_layout_2_block_loads_without_links() {
        let mut data = PhysicsData::default();
        data.scenes.push(PhysicsScene::new(1));
        data.bodies.push(PhysicsBody::new(1, 7));
        let mut bytes = Vec::new();
        write(&mut bytes, &data, true).unwrap();
        // The same block as layout 2 wrote it: its version byte, no links.
        bytes[2] = 2;
        bytes.truncate(bytes.len() - 2);
        let mut cursor = Cursor::new(&bytes);
        assert_eq!(read(&mut cursor, true).unwrap(), data);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn the_first_v41_layout_still_loads() {
        // A scene-less block with one plain body and no liquids, written
        // the way the first liquid build did: no layout mark, no extras.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&7u32.to_le_bytes());
        bytes.push(0);
        bytes.push(1);
        for value in [1.0f32, 2.0, 0.3, 0.5, 0.0, 0.05, 1.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 1, 0, 0]);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let mut cursor = Cursor::new(&bytes);
        let data = read(&mut cursor, true).unwrap();
        assert_eq!(cursor.remaining(), 0);
        assert_eq!(data.bodies[0].shape, ColliderShape::Circle);
        assert_eq!(data.bodies[0].mass, 2.0);
        assert_eq!(data.bodies[0].size, [1.0, 1.0]);
    }

    #[test]
    fn sanitizing_repairs_out_of_range_and_duplicate_entries() {
        let mut data = PhysicsData::default();
        let mut scene = PhysicsScene::new(1);
        scene.start_frame = 40;
        scene.end_frame = 10;
        scene.substeps = 0;
        scene.gravity = [f32::NAN, 5.0];
        data.scenes.push(scene.clone());
        data.scenes.push(scene);
        let mut body = PhysicsBody::new(1, 7);
        body.bounce = 3.0;
        body.group = 200;
        body.mass = -1.0;
        data.bodies.push(body.clone());
        data.bodies.push(body);
        data.bodies.push(PhysicsBody::new(1, 0));
        let mut link = PhysicsLink::new(1, 1, LinkKind::Spring, 7, 9);
        link.stiffness = 4.0;
        link.length = -5.0;
        data.links.push(link.clone());
        data.links.push(link);
        data.links
            .push(PhysicsLink::new(1, 2, LinkKind::Rope, 9, 9));
        data.links
            .push(PhysicsLink::new(1, 3, LinkKind::Rope, 9, 0));
        data.sanitize();

        assert_eq!(data.scenes.len(), 1);
        let scene = &data.scenes[0];
        assert_eq!((scene.start_frame, scene.end_frame), (10, 40));
        assert_eq!(scene.substeps, 1);
        assert_eq!(scene.gravity, [0.0, 5.0]);
        assert_eq!(data.bodies.len(), 1, "duplicates and instance 0 go");
        let body = &data.bodies[0];
        assert_eq!(body.bounce, 1.0);
        assert_eq!(body.group, MAX_COLLISION_GROUP);
        assert!(body.mass > 0.0);
        assert_eq!(data.links.len(), 1, "duplicates and self or empty ties go");
        assert_eq!(data.links[0].stiffness, 1.0);
        assert_eq!(data.links[0].length, 1.0);
    }
}
