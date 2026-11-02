//! Rigid bodies in 3D: rapier3d boxes and balls falling onto a floor and
//! knocking into each other. The editor bakes their falls into keys.
//!
//! Stage space: x right, y down, z away from the camera, in stage pixels.

use rapier3d::math::{Pose, Rotation};
use rapier3d::prelude::{ColliderBuilder, PhysicsWorld, RigidBodyBuilder, Vector};

/// Stage pixels in one rapier length unit, as in 2D.
const PIXELS_PER_UNIT: f32 = 100.0;
/// Shapes never shrink below this, so flat cards still collide.
const MIN_HALF: f32 = 1.0;
/// Rapier steps in every movie frame.
const SUBSTEPS: u32 = 4;
/// How thick the floor slab is under its top.
const FLOOR_DEPTH: f32 = 1000.0;
/// How far the floor reaches each way.
const FLOOR_REACH: f32 = 1.0e5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape3 {
    /// Half its size along its own x, y and z.
    Box([f32; 3]),
    Ball(f32),
}

/// One body at the start of a simulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body3 {
    pub dynamic: bool,
    pub shape: Shape3,
    /// Where the shape's middle sits in the body's own space.
    pub offset: [f32; 3],
    pub position: [f32; 3],
    /// A unit quaternion: x, y, z, w.
    pub rotation: [f32; 4],
    /// Stage pixels a second.
    pub velocity: [f32; 3],
    /// Radians a second about x, y and z.
    pub spin: [f32; 3],
    pub bounce: f32,
    pub friction: f32,
    pub mass: f32,
}

/// Where a body is on one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose3 {
    pub position: [f32; 3],
    /// A unit quaternion: x, y, z, w.
    pub rotation: [f32; 4],
}

fn vector(v: [f32; 3]) -> Vector {
    Vector::new(v[0], v[1], v[2])
}

/// Runs `bodies` for `frames` frames of a movie at `fps`, with `gravity`
/// pulling down the stage and, when given, a floor whose top is at that y.
/// Returns every body's pose on every frame, the starting one first.
pub fn simulate(
    bodies: &[Body3],
    gravity: f32,
    floor: Option<f32>,
    fps: f32,
    frames: usize,
) -> Vec<Vec<Pose3>> {
    let mut world = PhysicsWorld::new();
    world.gravity = Vector::new(0.0, gravity, 0.0);
    world.integration_parameters.length_unit = PIXELS_PER_UNIT;
    world.integration_parameters.dt = 1.0 / fps.max(1.0) / SUBSTEPS as f32;
    if let Some(top) = floor {
        let slab = ColliderBuilder::cuboid(FLOOR_REACH, FLOOR_DEPTH, FLOOR_REACH)
            .translation(Vector::new(0.0, top + FLOOR_DEPTH, 0.0))
            .friction(0.7)
            .restitution(0.3);
        world.colliders.insert(slab);
    }
    let handles: Vec<_> = bodies
        .iter()
        .map(|body| {
            let [x, y, z, w] = body.rotation;
            let pose = Pose::from_parts(
                vector(body.position),
                Rotation::from_xyzw(x, y, z, w).normalize(),
            );
            let builder = if body.dynamic {
                RigidBodyBuilder::dynamic()
                    .linvel(vector(body.velocity))
                    .angvel(vector(body.spin))
                    .ccd_enabled(true)
            } else {
                RigidBodyBuilder::fixed()
            }
            .pose(pose);
            let collider = match body.shape {
                Shape3::Box([hx, hy, hz]) => {
                    ColliderBuilder::cuboid(hx.max(MIN_HALF), hy.max(MIN_HALF), hz.max(MIN_HALF))
                }
                Shape3::Ball(radius) => ColliderBuilder::ball(radius.max(MIN_HALF)),
            }
            .translation(vector(body.offset))
            .restitution(body.bounce.clamp(0.0, 1.0))
            .friction(body.friction.max(0.0))
            .mass(body.mass.max(1.0e-3));
            world.insert(builder, collider).0
        })
        .collect();
    let read = |world: &PhysicsWorld| -> Vec<Pose3> {
        handles
            .iter()
            .map(|handle| {
                let body = &world.bodies[*handle];
                let p = body.translation();
                let q = *body.rotation();
                Pose3 {
                    position: [p.x, p.y, p.z],
                    rotation: [q.x, q.y, q.z, q.w],
                }
            })
            .collect()
    };
    let mut out: Vec<Vec<Pose3>> = vec![Vec::with_capacity(frames + 1); bodies.len()];
    let mut record = |poses: Vec<Pose3>| {
        for (track, pose) in out.iter_mut().zip(poses) {
            track.push(pose);
        }
    };
    record(read(&world));
    for _ in 0..frames {
        for _ in 0..SUBSTEPS {
            world.step();
        }
        record(read(&world));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(position: [f32; 3]) -> Body3 {
        Body3 {
            dynamic: true,
            shape: Shape3::Box([25.0, 25.0, 25.0]),
            offset: [0.0; 3],
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            velocity: [0.0; 3],
            spin: [0.0; 3],
            bounce: 0.0,
            friction: 0.8,
            mass: 1.0,
        }
    }

    #[test]
    fn a_box_falls_and_comes_to_rest_on_the_floor() {
        let poses = simulate(&[cube([100.0, 0.0, 0.0])], 980.0, Some(400.0), 24.0, 96);
        let track = &poses[0];
        assert_eq!(track.len(), 97);
        let last = track.last().unwrap();
        // Its middle rests half its size above the floor.
        assert!((last.position[1] - 375.0).abs() < 3.0, "{last:?}");
        assert!((last.position[0] - 100.0).abs() < 6.0, "{last:?}");
    }

    #[test]
    fn a_still_body_holds_its_place_and_stops_others() {
        let mut shelf = cube([0.0, 300.0, 0.0]);
        shelf.dynamic = false;
        shelf.shape = Shape3::Box([200.0, 10.0, 200.0]);
        let poses = simulate(
            &[shelf, cube([0.0, 0.0, 0.0])],
            980.0,
            Some(600.0),
            24.0,
            72,
        );
        assert_eq!(poses[0].last().unwrap().position, [0.0, 300.0, 0.0]);
        let resting = poses[1].last().unwrap().position[1];
        assert!((resting - 265.0).abs() < 3.0, "{resting}");
    }

    #[test]
    fn a_bouncy_ball_comes_back_up() {
        let mut ball = cube([0.0, 0.0, 0.0]);
        ball.shape = Shape3::Ball(20.0);
        ball.bounce = 0.9;
        let poses = simulate(&[ball], 980.0, Some(300.0), 24.0, 60);
        let heights: Vec<f32> = poses[0].iter().map(|p| p.position[1]).collect();
        let lowest = heights
            .iter()
            .position(|y| *y > 275.0)
            .expect("it reaches the floor");
        let after = heights[lowest..].iter().cloned().fold(f32::MAX, f32::min);
        assert!(after < 200.0, "it only got back to {after}");
    }
}
