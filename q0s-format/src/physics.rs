//! Physics authored in the editor.
//!
//! A simulation runs in the editor and is baked into ordinary keyframes, so
//! players only ever play keys back. What is stored here is the setup, kept
//! so a scene can be changed and baked again.

use crate::error::Error;
use crate::io::Cursor;

/// Stage edges that act as walls, as bits of [`PhysicsScene::walls`].
pub const WALL_LEFT: u8 = 1;
pub const WALL_RIGHT: u8 = 2;
pub const WALL_TOP: u8 = 4;
pub const WALL_BOTTOM: u8 = 8;
pub const WALLS_ALL: u8 = WALL_LEFT | WALL_RIGHT | WALL_TOP | WALL_BOTTOM;

/// Collision groups a body can join. Group 0 collides with nothing.
pub const MAX_COLLISION_GROUP: u8 = 31;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicsData {
    /// One world per timeline that has physics.
    pub scenes: Vec<PhysicsScene>,
    pub bodies: Vec<PhysicsBody>,
}

impl PhysicsData {
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty() && self.bodies.is_empty()
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
    /// Simulation steps per frame. More keep fast bodies from tunnelling.
    pub substeps: u8,
    /// Speeds the world up or slows it down without changing the frame rate.
    pub time_scale: f32,
    /// Stage edges that act as walls, as `WALL_*` bits.
    pub walls: u8,
    pub wall_bounce: f32,
    pub wall_friction: f32,
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
}

impl ColliderShape {
    pub const ALL: [Self; 2] = [Self::Box, Self::Circle];

    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Box => 0,
            Self::Circle => 1,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Box),
            1 => Some(Self::Circle),
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
}

impl PhysicsBody {
    pub fn new(q0rg_id: u16, instance_id: u32) -> Self {
        Self {
            q0rg_id,
            instance_id,
            kind: BodyKind::Dynamic,
            shape: ColliderShape::Box,
            shape_scale: 1.0,
            mass: 1.0,
            bounce: 0.3,
            friction: 0.5,
            linear_damping: 0.0,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            fixed_rotation: false,
            group: 1,
            momentum: Momentum::FromMotion,
        }
    }

    fn sanitize(&mut self) {
        self.shape_scale = finite_clamp(self.shape_scale, 0.05, 20.0, 1.0);
        self.mass = finite_clamp(self.mass, 0.001, 100_000.0, 1.0);
        self.bounce = finite_clamp(self.bounce, 0.0, 1.0, 0.3);
        self.friction = finite_clamp(self.friction, 0.0, 4.0, 0.5);
        self.linear_damping = finite_clamp(self.linear_damping, 0.0, 100.0, 0.0);
        self.angular_damping = finite_clamp(self.angular_damping, 0.0, 100.0, 0.05);
        self.gravity_scale = finite_clamp(self.gravity_scale, -20.0, 20.0, 1.0);
        self.group = self.group.min(MAX_COLLISION_GROUP);
        if let Momentum::Manual { velocity, spin } = &mut self.momentum {
            for axis in velocity.iter_mut() {
                *axis = finite_clamp(*axis, -1_000_000.0, 1_000_000.0, 0.0);
            }
            *spin = finite_clamp(*spin, -100_000.0, 100_000.0, 0.0);
        }
    }
}

pub(crate) fn write(out: &mut Vec<u8>, data: &PhysicsData) -> Result<(), Error> {
    let count = u16::try_from(data.scenes.len())
        .map_err(|_| Error::Overflow("physics scene count exceeds u16"))?;
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
    }
    let count = u16::try_from(data.bodies.len())
        .map_err(|_| Error::Overflow("physics body count exceeds u16"))?;
    out.extend_from_slice(&count.to_le_bytes());
    for body in &data.bodies {
        out.extend_from_slice(&body.q0rg_id.to_le_bytes());
        out.extend_from_slice(&body.instance_id.to_le_bytes());
        out.push(body.kind.code());
        out.push(body.shape.code());
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
    }
    Ok(())
}

pub(crate) fn read(c: &mut Cursor<'_>) -> Result<PhysicsData, Error> {
    let mut data = PhysicsData::default();
    for _ in 0..c.read_u16()? {
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
        data.bodies.push(body);
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
    }
}
