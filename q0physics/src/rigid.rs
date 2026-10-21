//! The rigid-body world: rapier2d bodies on a timeline, the stage walls
//! around them, the links tying bodies together, and any liquids poured
//! into it.

use q0s_format::physics::{
    BodyKind, ColliderShape, LinkKind, PhysicsBody, PhysicsLink, PhysicsLiquid, PhysicsScene,
    MAX_COLLISION_GROUP, WALL_BOTTOM, WALL_LEFT, WALL_RIGHT, WALL_TOP,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{Transform2D, Vec2};
use rapier2d::prelude::{
    ColliderBuilder, ColliderHandle, FixedJointBuilder, Group, ImpulseJointHandle,
    InteractionGroups, InteractionTestMode, PhysicsWorld, Pose, RevoluteJointBuilder,
    RigidBodyBuilder, RigidBodyHandle, RopeJointBuilder, SharedShape, SpringCoefficients,
    SpringJointBuilder, Vector,
};

use crate::liquid::{Liquid, Obstacle, Walls};

/// Stage pixels in one rapier length unit, so its tolerances suit objects a
/// few dozen to a few hundred pixels across.
const PIXELS_PER_UNIT: f32 = 100.0;
/// Colliders never shrink below this, so lines and dots still collide.
pub const MIN_HALF_EXTENT: f32 = 0.5;

/// Where a collider sits and how far it is turned. Angles run on past a
/// half turn instead of wrapping, so keys spin the way the body did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyPose {
    pub center: [f32; 2],
    pub angle: f32,
}

/// An object's collider placement, fitted to its bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub pose: BodyPose,
    /// Half the width and height of its bounds as they show on the stage.
    pub half_extents: [f32; 2],
}

/// Fits a collider to `bounds` (min x, min y, max x, max y in the object's
/// own space) placed by `affine`.
pub fn fit_bounds(bounds: [f32; 4], affine: Affine) -> Fit {
    let [min_x, min_y, max_x, max_y] = bounds;
    let center = affine.apply(Vec2::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5));
    let x_axis = affine.a11.hypot(affine.a21);
    let y_axis = affine.a12.hypot(affine.a22);
    Fit {
        pose: BodyPose {
            center: [center.x, center.y],
            angle: affine.a21.atan2(affine.a11),
        },
        half_extents: [
            ((max_x - min_x) * 0.5 * x_axis).max(MIN_HALF_EXTENT),
            ((max_y - min_y) * 0.5 * y_axis).max(MIN_HALF_EXTENT),
        ],
    }
}

/// A collider's shape in its body's own frame: the origin is the body's
/// centre and x runs along the object's x axis.
#[derive(Debug, Clone, PartialEq)]
pub enum ColliderGeometry {
    /// Half its width and height, turned by `turn` radians.
    Box {
        half_extents: [f32; 2],
        turn: f32,
    },
    Circle {
        radius: f32,
    },
    /// A pill along the longer side of `half_extents`.
    Capsule {
        half_extents: [f32; 2],
        turn: f32,
    },
    /// Convex parts, each a loop of points.
    Parts(Vec<Vec<[f32; 2]>>),
}

impl ColliderGeometry {
    /// Half the width and height of the box around it.
    pub fn reach(&self) -> [f32; 2] {
        let mut reach = [MIN_HALF_EXTENT; 2];
        for point in self.outline().iter().flatten() {
            reach[0] = reach[0].max(point[0].abs());
            reach[1] = reach[1].max(point[1].abs());
        }
        reach
    }

    /// Loops that draw it, in the body's frame.
    pub fn outline(&self) -> Vec<Vec<[f32; 2]>> {
        const ROUND: usize = 32;
        let turned = |points: Vec<[f32; 2]>, turn: f32| {
            let (sin, cos) = turn.sin_cos();
            points
                .into_iter()
                .map(|[x, y]| [cos * x - sin * y, sin * x + cos * y])
                .collect::<Vec<_>>()
        };
        match self {
            Self::Box {
                half_extents: [hx, hy],
                turn,
            } => vec![turned(
                vec![[-hx, -hy], [*hx, -hy], [*hx, *hy], [-hx, *hy]],
                *turn,
            )],
            Self::Circle { radius } => vec![(0..ROUND)
                .map(|index| {
                    let angle = index as f32 * std::f32::consts::TAU / ROUND as f32;
                    [angle.cos() * radius, angle.sin() * radius]
                })
                .collect()],
            Self::Capsule { half_extents, turn } => {
                let (radius, reach, along_x) = capsule(*half_extents);
                let half_round = ROUND / 2;
                let mut points = Vec::with_capacity(ROUND + 2);
                for (end, start_angle) in [
                    (reach, -std::f32::consts::FRAC_PI_2),
                    (-reach, std::f32::consts::FRAC_PI_2),
                ] {
                    for index in 0..=half_round {
                        let angle =
                            start_angle + index as f32 * std::f32::consts::PI / half_round as f32;
                        points.push([end + angle.cos() * radius, angle.sin() * radius]);
                    }
                }
                if !along_x {
                    for point in &mut points {
                        *point = [point[1], point[0]];
                    }
                }
                vec![turned(points, *turn)]
            }
            Self::Parts(parts) => parts.clone(),
        }
    }

    fn builder(&self) -> ColliderBuilder {
        match self {
            Self::Box {
                half_extents: [hx, hy],
                turn,
            } => ColliderBuilder::cuboid(*hx, *hy).rotation(*turn),
            Self::Circle { radius } => ColliderBuilder::ball(*radius),
            Self::Capsule { half_extents, turn } => {
                let (radius, reach, along_x) = capsule(*half_extents);
                if along_x {
                    ColliderBuilder::capsule_x(reach, radius)
                } else {
                    ColliderBuilder::capsule_y(reach, radius)
                }
                .rotation(*turn)
            }
            Self::Parts(parts) => {
                let shapes = parts
                    .iter()
                    .filter_map(|part| {
                        let points = part
                            .iter()
                            .map(|[x, y]| Vector::new(*x, *y))
                            .collect::<Vec<_>>();
                        SharedShape::convex_hull(&points)
                    })
                    .map(|shape| (Pose::IDENTITY, shape))
                    .collect::<Vec<_>>();
                match shapes.len() {
                    0 => {
                        let [hx, hy] = self.reach();
                        ColliderBuilder::cuboid(hx, hy)
                    }
                    1 => ColliderBuilder::new(shapes.into_iter().next().expect("one").1),
                    _ => ColliderBuilder::compound(shapes),
                }
            }
        }
    }
}

/// A capsule fitting `half_extents`: its radius, how far its round ends
/// sit from the middle, and whether it lies along x.
fn capsule([hx, hy]: [f32; 2]) -> (f32, f32, bool) {
    let radius = hx.min(hy).max(MIN_HALF_EXTENT);
    (radius, (hx.max(hy) - radius).max(0.0), hx >= hy)
}

/// Where the collider of `body` sits for an object fitted with `fit`, drawn
/// through `affine`, and its shape. `parts` are the artwork's convex parts
/// in the object's own space, for hull and outline colliders.
pub fn body_collider(
    body: &PhysicsBody,
    fit: Fit,
    affine: Affine,
    parts: &[Vec<[f32; 2]>],
) -> (BodyPose, ColliderGeometry) {
    let angle = fit.pose.angle;
    let (sin, cos) = angle.sin_cos();
    let [ox, oy] = body.offset;
    let center = [
        fit.pose.center[0] + cos * ox - sin * oy,
        fit.pose.center[1] + sin * ox + cos * oy,
    ];
    let scale = [
        body.shape_scale * body.size[0],
        body.shape_scale * body.size[1],
    ];
    let turn = body.turn.to_radians();
    let half_extents = [
        (fit.half_extents[0] * scale[0]).max(MIN_HALF_EXTENT),
        (fit.half_extents[1] * scale[1]).max(MIN_HALF_EXTENT),
    ];
    let pose = BodyPose { center, angle };
    let boxed = ColliderGeometry::Box { half_extents, turn };
    let geometry = match body.shape {
        ColliderShape::Box => boxed,
        ColliderShape::Circle => ColliderGeometry::Circle {
            radius: half_extents[0].max(half_extents[1]),
        },
        ColliderShape::Capsule => ColliderGeometry::Capsule { half_extents, turn },
        ColliderShape::Hull | ColliderShape::Outline => {
            let (turn_sin, turn_cos) = turn.sin_cos();
            // Into the body's frame around the middle of the bounds, scaled
            // and turned there, so offset, size and turn act on these shapes
            // as they do on a box.
            let place = |point: &[f32; 2]| {
                let stage = affine.apply(Vec2::new(point[0], point[1]));
                let dx = stage.x - fit.pose.center[0];
                let dy = stage.y - fit.pose.center[1];
                let x = (cos * dx + sin * dy) * scale[0];
                let y = (-sin * dx + cos * dy) * scale[1];
                [turn_cos * x - turn_sin * y, turn_sin * x + turn_cos * y]
            };
            let placed = parts
                .iter()
                .filter(|part| part.len() >= 3)
                .map(|part| part.iter().map(place).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let placed = if body.shape == ColliderShape::Hull {
                let hull = convex_hull(placed.iter().flatten().copied());
                if hull.len() >= 3 {
                    vec![hull]
                } else {
                    Vec::new()
                }
            } else {
                placed
            };
            if placed.is_empty() {
                boxed
            } else {
                ColliderGeometry::Parts(placed)
            }
        }
    };
    (pose, geometry)
}

/// The smallest convex loop around `points`, counter-clockwise on a y-up
/// plane (clockwise on the stage).
pub fn convex_hull(points: impl IntoIterator<Item = [f32; 2]>) -> Vec<[f32; 2]> {
    let mut points = points
        .into_iter()
        .filter(|[x, y]| x.is_finite() && y.is_finite())
        .collect::<Vec<_>>();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f32; 2]> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &point in ordered {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    hull
}

/// The transform that shows an object, which had `transform` while its
/// collider sat at `start`, with the collider moved and turned to `pose`.
pub fn posed_transform(transform: Transform2D, start: BodyPose, pose: BodyPose) -> Transform2D {
    let turn = pose.angle - start.angle;
    let (sin, cos) = turn.sin_cos();
    let offset_x = transform.tx - start.center[0];
    let offset_y = transform.ty - start.center[1];
    Transform2D {
        tx: pose.center[0] + cos * offset_x - sin * offset_y,
        ty: pose.center[1] + sin * offset_x + cos * offset_y,
        rotation: transform.rotation + turn,
        ..transform
    }
}

/// Follow a body's attachment through the same affine transform as its artwork,
/// rather than through a separately sampled simulation pose. The reference
/// collider offset, artwork scale, skew and rotation all remain part of it.
pub fn attachment_point(
    start_transform: Transform2D,
    start_pose: BodyPose,
    drawn: Affine,
    anchor: [f32; 2],
) -> [f32; 2] {
    let point = pose_point(start_pose, anchor);
    let start = Affine::from_transform(start_transform);
    let local = start.inverse().map_or(Vec2::new(0.0, 0.0), |inverse| {
        inverse.apply(Vec2::new(point[0], point[1]))
    });
    let point = drawn.apply(local);
    [point.x, point.y]
}

/// A point of a body's own frame on the stage.
fn pose_point(pose: BodyPose, [x, y]: [f32; 2]) -> [f32; 2] {
    let (sin, cos) = pose.angle.sin_cos();
    [
        pose.center[0] + cos * x - sin * y,
        pose.center[1] + sin * x + cos * y,
    ]
}

/// A stage point in a body's own frame.
fn local_point(pose: BodyPose, [x, y]: [f32; 2]) -> [f32; 2] {
    let (sin, cos) = pose.angle.sin_cos();
    let (dx, dy) = (x - pose.center[0], y - pose.center[1]);
    [cos * dx + sin * dy, -sin * dx + cos * dy]
}

pub(crate) fn wrap_angle(angle: f32) -> f32 {
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
    wrapped - std::f32::consts::PI
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

/// A body as it enters the world.
pub struct BodyStart<'a> {
    pub body: &'a PhysicsBody,
    /// The collider, from [`body_collider`].
    pub geometry: ColliderGeometry,
    pub pose: BodyPose,
    /// Stage pixels per second.
    pub velocity: [f32; 2],
    /// Radians per second, clockwise.
    pub spin: f32,
}

struct Slot {
    instance_id: u32,
    handle: RigidBodyHandle,
    collider: ColliderHandle,
    kind: BodyKind,
    geometry: ColliderGeometry,
    group: u8,
    bounce: f32,
    friction: f32,
    mass: f32,
    /// Continuous angle as of the last finished frame.
    angle: f32,
    /// An animated body glides from here...
    drive_from: BodyPose,
    /// ...to here over the next frame.
    drive_to: BodyPose,
}

/// A link as it enters the world.
pub struct LinkStart<'a> {
    pub link: &'a PhysicsLink,
    /// The body index of the object it hangs from; `None` ties it to the
    /// stage.
    pub from: Option<usize>,
    /// The body index of the object tied on.
    pub to: usize,
}

/// A joint along a link, between two of its points.
struct Tie {
    handle: Option<ImpulseJointHandle>,
    /// The link points it holds together, by index into `LinkSlot::points`.
    between: (usize, usize),
}

struct LinkSlot {
    link_id: u16,
    kind: LinkKind,
    /// The bodies the link runs through and where on each, in their own
    /// frames: the `from` end, any rope pieces, then the `to` end.
    points: Vec<(RigidBodyHandle, [f32; 2])>,
    /// Joints that carry the pull, one per stretch of a rope.
    ties: Vec<Tie>,
    /// Joints across several stretches: springs that keep a stiff rope
    /// straight, and tethers that stop a long rope stretching under load.
    bends: Vec<Tie>,
    /// Pulled harder than this, a tie snaps; 0 never.
    break_force: f32,
}

/// One timeline's physics world.
pub struct World {
    rapier: PhysicsWorld,
    slots: Vec<Slot>,
    links: Vec<LinkSlot>,
    liquids: Vec<Liquid>,
    walls: Walls,
    gravity: [f32; 2],
    air_drag: f32,
    substeps: u32,
    substep_dt: f32,
}

impl World {
    /// A world for `scene` on a stage `stage_size` pixels big, stepping one
    /// timeline frame of a movie running at `fps` per [`World::step_frame`].
    pub fn new(scene: &PhysicsScene, stage_size: [f32; 2], fps: f32) -> Self {
        // Quality sets a rate in simulated seconds, not a number of steps in
        // each movie frame. With four steps/frame the old solver ran at 96 Hz
        // in a 24 fps movie and 960 Hz at 240 fps, changing integration error,
        // contacts, ropes and liquid pressure along with the movie's fps.
        // Split each frame evenly to reach the chosen rate (240 Hz by default).
        let frame_dt = f64::from(scene.time_scale) / f64::from(fps.max(1.0));
        let step_rate = 60.0 * f64::from(scene.substeps.clamp(1, 16));
        let substeps = (frame_dt * step_rate).ceil().max(1.0) as u32;
        let substep_dt = (frame_dt / f64::from(substeps)) as f32;
        let mut rapier = PhysicsWorld::new();
        rapier.gravity = Vector::new(scene.gravity[0], scene.gravity[1]);
        rapier.integration_parameters.length_unit = PIXELS_PER_UNIT;
        rapier.integration_parameters.dt = substep_dt;
        let walls = Walls {
            bits: scene.walls,
            width: stage_size[0].max(1.0),
            height: stage_size[1].max(1.0),
            bounce: scene.wall_bounce,
            friction: scene.wall_friction,
        };
        add_walls(&mut rapier, &walls);
        Self {
            rapier,
            slots: Vec::new(),
            links: Vec::new(),
            liquids: Vec::new(),
            walls,
            gravity: scene.gravity,
            air_drag: scene.air_drag,
            substeps,
            substep_dt,
        }
    }

    /// Adds a body; returns its index.
    pub fn add_body(&mut self, start: BodyStart<'_>) -> usize {
        let body = start.body;
        let [x, y] = start.pose.center;
        let builder = match body.kind {
            BodyKind::Dynamic => {
                let builder = RigidBodyBuilder::dynamic()
                    .linvel(Vector::new(start.velocity[0], start.velocity[1]))
                    .angvel(start.spin)
                    .linear_damping(body.linear_damping + self.air_drag)
                    .angular_damping(body.angular_damping + self.air_drag)
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
        .rotation(start.pose.angle);
        let collider = start
            .geometry
            .builder()
            .restitution(body.bounce)
            .friction(body.friction)
            .mass(body.mass)
            .collision_groups(body_groups(body.group));
        let (handle, collider) = self.rapier.insert(builder, collider);
        self.slots.push(Slot {
            instance_id: body.instance_id,
            handle,
            collider,
            kind: body.kind,
            geometry: start.geometry,
            group: body.group,
            bounce: body.bounce,
            friction: body.friction,
            mass: body.mass,
            angle: start.pose.angle,
            drive_from: start.pose,
            drive_to: start.pose,
        });
        self.slots.len() - 1
    }

    /// Ties two bodies together, or a body to the stage; returns the link's
    /// index. Hinges and welds hold the objects as they are now.
    pub fn add_link(&mut self, start: LinkStart<'_>) -> usize {
        let link = start.link;
        let to_handle = self.slots[start.to].handle;
        let to_pose = self.rapier_pose(to_handle);
        let to_point = pose_point(to_pose, link.to_anchor);
        let from_point = match (link.kind.spans(), start.from) {
            (true, Some(from)) => {
                pose_point(self.rapier_pose(self.slots[from].handle), link.from_anchor)
            }
            (true, None) => link.from_anchor,
            (false, _) => to_point,
        };
        let from_handle = match start.from {
            Some(from) => self.slots[from].handle,
            // A pin on the stage: a fixed body without a collider.
            None => self.rapier.bodies.insert(
                RigidBodyBuilder::fixed().translation(Vector::new(from_point[0], from_point[1])),
            ),
        };
        let from_pose = self.rapier_pose(from_handle);
        let from_local = local_point(from_pose, from_point);
        let to_local = local_point(to_pose, to_point);
        let damping_ratio = 0.02 + link.damping;
        let mut slot = LinkSlot {
            link_id: link.link_id,
            kind: link.kind,
            points: vec![(from_handle, from_local)],
            ties: Vec::new(),
            bends: Vec::new(),
            break_force: link.break_force,
        };
        match link.kind {
            LinkKind::Rope => {
                let pieces = usize::from(link.segments);
                let stretch = (pieces + 1) as f32;
                let piece_length = link.length / stretch;
                let radius = (link.width * 0.5).max(MIN_HALF_EXTENT);
                for piece in 1..=pieces {
                    let t = piece as f32 / stretch;
                    let at = [
                        from_point[0] + (to_point[0] - from_point[0]) * t,
                        from_point[1] + (to_point[1] - from_point[1]) * t,
                    ];
                    let body = RigidBodyBuilder::dynamic()
                        .translation(Vector::new(at[0], at[1]))
                        .lock_rotations()
                        .linear_damping(self.air_drag + link.damping * 2.0);
                    let collider = ColliderBuilder::ball(radius)
                        .mass(link.mass / pieces as f32)
                        .friction(0.5)
                        .restitution(0.0)
                        .collision_groups(body_groups(link.group));
                    let (handle, _) = self.rapier.insert(body, collider);
                    slot.points.push((handle, [0.0, 0.0]));
                }
                slot.points.push((to_handle, to_local));
                // Stretchy ropes give like a bungee; others hold their length.
                let softness = if link.stretch > 0.0 {
                    SpringCoefficients::new(40.0 * 0.015_f32.powf(link.stretch), damping_ratio)
                } else {
                    SpringCoefficients::joint_defaults()
                };
                for index in 0..slot.points.len() - 1 {
                    let (a, b) = (slot.points[index], slot.points[index + 1]);
                    let joint = RopeJointBuilder::new(piece_length)
                        .local_anchor1(Vector::new(a.1[0], a.1[1]))
                        .local_anchor2(Vector::new(b.1[0], b.1[1]))
                        .softness(softness)
                        .contacts_enabled(pieces == 0 && link.collide);
                    let handle = self.rapier.impulse_joints.insert(a.0, b.0, joint, true);
                    slot.ties.push(Tie {
                        handle: Some(handle),
                        between: (index, index + 1),
                    });
                }
                // Springs across every bend keep a stiff rope straight.
                if link.stiffness > 0.0 && pieces > 0 {
                    let omega = std::f32::consts::TAU * 0.4 * 25.0_f32.powf(link.stiffness);
                    for index in 0..slot.points.len() - 2 {
                        let (a, b) = (slot.points[index], slot.points[index + 2]);
                        let joint = SpringJointBuilder::new(
                            piece_length * 2.0,
                            omega * omega,
                            2.0 * damping_ratio * omega,
                        )
                        .local_anchor1(Vector::new(a.1[0], a.1[1]))
                        .local_anchor2(Vector::new(b.1[0], b.1[1]))
                        .contacts_enabled(false);
                        let handle = self.rapier.impulse_joints.insert(a.0, b.0, joint, true);
                        slot.bends.push(Tie {
                            handle: Some(handle),
                            between: (index, index + 2),
                        });
                    }
                }
                // A chain of light pieces under a heavy load gives way bit
                // by bit; tethers from the top hold every piece within its
                // share of the length, and the far end within all of it.
                if pieces > 0 {
                    let solver = &mut self.rapier.integration_parameters.num_solver_iterations;
                    *solver = (*solver).max(8);
                    let (top, top_local) = slot.points[0];
                    for index in 2..slot.points.len() {
                        let (b, b_local) = slot.points[index];
                        let last = index == slot.points.len() - 1;
                        let joint = RopeJointBuilder::new(piece_length * index as f32)
                            .local_anchor1(Vector::new(top_local[0], top_local[1]))
                            .local_anchor2(Vector::new(b_local[0], b_local[1]))
                            .softness(softness)
                            .contacts_enabled(last && link.collide);
                        let handle = self.rapier.impulse_joints.insert(top, b, joint, true);
                        slot.bends.push(Tie {
                            handle: Some(handle),
                            between: (0, index),
                        });
                    }
                }
            }
            LinkKind::Spring => {
                slot.points.push((to_handle, to_local));
                let omega = std::f32::consts::TAU * 0.3 * 40.0_f32.powf(link.stiffness);
                let joint = SpringJointBuilder::new(
                    link.length,
                    omega * omega,
                    2.0 * damping_ratio * omega,
                )
                .local_anchor1(Vector::new(from_local[0], from_local[1]))
                .local_anchor2(Vector::new(to_local[0], to_local[1]))
                .contacts_enabled(link.collide);
                let handle = self
                    .rapier
                    .impulse_joints
                    .insert(from_handle, to_handle, joint, true);
                slot.ties.push(Tie {
                    handle: Some(handle),
                    between: (0, 1),
                });
            }
            LinkKind::Hinge => {
                slot.points.push((to_handle, to_local));
                let turned = wrap_angle(to_pose.angle - from_pose.angle);
                // A hinge with stiffness swings back to how it started; one
                // without only slows down by its damping.
                let omega = std::f32::consts::TAU * 0.3 * 40.0_f32.powf(link.stiffness);
                let (stiffness, motor_damping) = if link.stiffness > 0.0 {
                    (omega * omega, 2.0 * damping_ratio * omega)
                } else {
                    (0.0, link.damping * 4.0)
                };
                let mut joint = RevoluteJointBuilder::new()
                    .local_anchor1(Vector::new(from_local[0], from_local[1]))
                    .local_anchor2(Vector::new(to_local[0], to_local[1]))
                    .contacts_enabled(link.collide);
                if stiffness > 0.0 || motor_damping > 0.0 {
                    joint = joint.motor(turned, 0.0, stiffness, motor_damping);
                }
                if link.swing_limit > 0.0 {
                    let reach = link.swing_limit.to_radians();
                    joint = joint.limits([turned - reach, turned + reach]);
                }
                let handle = self
                    .rapier
                    .impulse_joints
                    .insert(from_handle, to_handle, joint, true);
                slot.ties.push(Tie {
                    handle: Some(handle),
                    between: (0, 1),
                });
            }
            LinkKind::Weld => {
                slot.points.push((to_handle, to_local));
                let softness = if link.stiffness >= 0.999 {
                    SpringCoefficients::joint_defaults()
                } else {
                    SpringCoefficients::new(0.5 * 40.0_f32.powf(link.stiffness), damping_ratio)
                };
                let joint = FixedJointBuilder::new()
                    .local_frame1(Pose::new(
                        Vector::new(from_local[0], from_local[1]),
                        -from_pose.angle,
                    ))
                    .local_frame2(Pose::new(
                        Vector::new(to_local[0], to_local[1]),
                        -to_pose.angle,
                    ))
                    .softness(softness)
                    .contacts_enabled(link.collide);
                let handle = self
                    .rapier
                    .impulse_joints
                    .insert(from_handle, to_handle, joint, true);
                slot.ties.push(Tie {
                    handle: Some(handle),
                    between: (0, 1),
                });
            }
        }
        self.links.push(slot);
        self.links.len() - 1
    }

    fn rapier_pose(&self, handle: RigidBodyHandle) -> BodyPose {
        self.rapier.bodies.get(handle).map_or(
            BodyPose {
                center: [0.0, 0.0],
                angle: 0.0,
            },
            |body| {
                let center = body.translation();
                BodyPose {
                    center: [center.x, center.y],
                    angle: body.rotation().angle(),
                }
            },
        )
    }

    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    pub fn link_id(&self, index: usize) -> u16 {
        self.links[index].link_id
    }

    pub fn index_of_link(&self, link_id: u16) -> Option<usize> {
        self.links.iter().position(|link| link.link_id == link_id)
    }

    /// Where link `index` runs on the stage now, as lines from point to
    /// point: one line whole, more once it has snapped, none for hinges and
    /// welds.
    pub fn link_lines(&self, index: usize) -> Vec<Vec<[f32; 2]>> {
        let slot = &self.links[index];
        if !slot.kind.spans() {
            return Vec::new();
        }
        let points = slot
            .points
            .iter()
            .map(|(handle, local)| pose_point(self.rapier_pose(*handle), *local))
            .collect::<Vec<_>>();
        self.link_runs(index)
            .into_iter()
            .map(|(first, last)| points[first..=last].to_vec())
            .collect()
    }

    /// Inclusive point ranges that remain connected. Sharing this topology
    /// with the drawing code prevents a cut end from being reattached visually.
    fn link_runs(&self, index: usize) -> Vec<(usize, usize)> {
        let slot = &self.links[index];
        if !slot.kind.spans() {
            return Vec::new();
        }
        let mut runs = Vec::new();
        let mut first = 0;
        for tie in &slot.ties {
            if tie.handle.is_none() {
                if tie.between.0 > first {
                    runs.push((first, tie.between.0));
                }
                first = tie.between.1;
            }
        }
        let last = slot.points.len() - 1;
        if last > first {
            runs.push((first, last));
        }
        runs
    }

    /// Whether each drawn run still holds the link's original first/last end.
    pub fn link_attached_ends(&self, index: usize) -> Vec<[bool; 2]> {
        let last = self.links[index].points.len() - 1;
        self.link_runs(index)
            .into_iter()
            .map(|(first, end)| [first == 0, end == last])
            .collect()
    }

    /// The live shape, stretched to the endpoints of the displayed artwork.
    pub fn link_lines_to(
        &self,
        index: usize,
        from: Option<[f32; 2]>,
        to: Option<[f32; 2]>,
    ) -> Vec<Vec<[f32; 2]>> {
        crate::links::fit_link_lines(
            &self.link_lines(index),
            &self.link_attached_ends(index),
            from,
            to,
        )
    }

    /// Whether link `index` has snapped or been cut anywhere.
    pub fn link_broken(&self, index: usize) -> bool {
        self.links[index]
            .ties
            .iter()
            .any(|tie| tie.handle.is_none())
    }

    /// Cuts link `index`: a rope in its middle, anything else outright.
    pub fn cut_link(&mut self, index: usize) {
        let live = self.links[index]
            .ties
            .iter()
            .enumerate()
            .filter(|(_, tie)| tie.handle.is_some())
            .map(|(at, _)| at)
            .collect::<Vec<_>>();
        if let Some(&at) = live.get(live.len() / 2) {
            self.snap(index, at);
        }
    }

    /// Breaks tie `at` of link `index`, and the bends across it.
    fn snap(&mut self, index: usize, at: usize) {
        let slot = &mut self.links[index];
        let Some(handle) = slot.ties[at].handle.take() else {
            return;
        };
        self.rapier.impulse_joints.remove(handle, true);
        let (from, to) = slot.ties[at].between;
        for bend in &mut slot.bends {
            if bend.between.0 <= from && bend.between.1 >= to {
                if let Some(handle) = bend.handle.take() {
                    self.rapier.impulse_joints.remove(handle, true);
                }
            }
        }
    }

    /// Snaps the ties pulled harder than their link can take.
    fn break_overloaded(&mut self) {
        // The solver splits each step into as many small ones as it
        // iterates, and keeps the impulses of the last.
        let iterations = self
            .rapier
            .integration_parameters
            .num_solver_iterations
            .max(1);
        let dt = (self.substep_dt / iterations as f32).max(1.0e-9);
        let pull = |tie: &Tie| {
            let joint = tie
                .handle
                .and_then(|handle| self.rapier.impulse_joints.get(handle));
            joint.map_or(0.0, |joint| {
                let data = &joint.data;
                (joint.impulses.x.hypot(joint.impulses.y)
                    + data.limits[0].impulse.abs()
                    + data.limits[1].impulse.abs()
                    + data.motors[0].impulse.abs()
                    + data.motors[1].impulse.abs())
                    / dt
            })
        };
        let mut snapped = Vec::new();
        for (index, slot) in self.links.iter().enumerate() {
            if slot.break_force <= 0.0 {
                continue;
            }
            // A stretch carries its own pull and that of every tether and
            // spring across it.
            let across = slot.bends.iter().map(|bend| (bend.between, pull(bend)));
            let across = across.collect::<Vec<_>>();
            let loads = slot
                .ties
                .iter()
                .enumerate()
                .filter(|(_, tie)| tie.handle.is_some());
            let heaviest = loads
                .map(|(at, tie)| {
                    let (from, to) = tie.between;
                    let carried = across
                        .iter()
                        .filter(|((first, last), _)| *first <= from && *last >= to)
                        .map(|(_, pull)| pull)
                        .sum::<f32>();
                    (at, pull(tie) + carried)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((at, load)) = heaviest {
                if load > slot.break_force {
                    snapped.push((index, at));
                }
            }
        }
        for (index, at) in snapped {
            self.snap(index, at);
        }
    }

    /// Pours `liquid` in; returns its index.
    pub fn add_liquid(&mut self, liquid: &PhysicsLiquid) -> usize {
        self.liquids.push(Liquid::new(liquid, self.walls));
        self.liquids.len() - 1
    }

    pub fn body_count(&self) -> usize {
        self.slots.len()
    }

    pub fn index_of(&self, instance_id: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.instance_id == instance_id)
    }

    pub fn instance_id(&self, index: usize) -> u32 {
        self.slots[index].instance_id
    }

    pub fn kind(&self, index: usize) -> BodyKind {
        self.slots[index].kind
    }

    pub fn geometry(&self, index: usize) -> &ColliderGeometry {
        &self.slots[index].geometry
    }

    /// Gives body `index` a new collider shape, as when its artwork changes.
    pub fn set_geometry(&mut self, index: usize, geometry: ColliderGeometry) {
        let slot = &mut self.slots[index];
        if slot.geometry == geometry {
            return;
        }
        self.rapier.remove_collider(slot.collider);
        let collider = geometry
            .builder()
            .restitution(slot.bounce)
            .friction(slot.friction)
            .mass(slot.mass)
            .collision_groups(body_groups(slot.group));
        slot.collider = self.rapier.insert_collider(collider, Some(slot.handle));
        slot.geometry = geometry;
    }

    /// Where body `index` is now.
    pub fn pose(&self, index: usize) -> BodyPose {
        let slot = &self.slots[index];
        let Some(body) = self.rapier.bodies.get(slot.handle) else {
            return slot.drive_to;
        };
        let center = body.translation();
        BodyPose {
            center: [center.x, center.y],
            angle: slot.angle,
        }
    }

    pub fn liquid_count(&self) -> usize {
        self.liquids.len()
    }

    /// The particles of liquid `index`.
    pub fn liquid_particles(&self, index: usize) -> &[[f32; 2]] {
        self.liquids[index].positions()
    }

    /// Moves animated body `index` to `pose` over the next frame.
    pub fn drive(&mut self, index: usize, pose: BodyPose) {
        let slot = &mut self.slots[index];
        slot.drive_to = BodyPose {
            center: pose.center,
            angle: slot.drive_from.angle + wrap_angle(pose.angle - slot.drive_from.angle),
        };
    }

    /// Changes the velocity of body `index` by `delta`, in pixels per second.
    pub fn push(&mut self, index: usize, delta: [f32; 2]) {
        if let Some(body) = self.rapier.bodies.get_mut(self.slots[index].handle) {
            let velocity = body.linvel();
            body.set_linvel(
                Vector::new(velocity.x + delta[0], velocity.y + delta[1]),
                true,
            );
        }
    }

    pub fn set_velocity(&mut self, index: usize, velocity: [f32; 2]) {
        if let Some(body) = self.rapier.bodies.get_mut(self.slots[index].handle) {
            body.set_linvel(Vector::new(velocity[0], velocity[1]), true);
        }
    }

    /// Radians per second, clockwise.
    pub fn set_spin(&mut self, index: usize, spin: f32) {
        if let Some(body) = self.rapier.bodies.get_mut(self.slots[index].handle) {
            body.set_angvel(spin, true);
        }
    }

    /// Puts body `index` down at `center`, keeping its turn and velocity.
    pub fn teleport(&mut self, index: usize, center: [f32; 2]) {
        let slot = &mut self.slots[index];
        if let Some(body) = self.rapier.bodies.get_mut(slot.handle) {
            body.set_translation(Vector::new(center[0], center[1]), true);
        }
        slot.drive_from.center = center;
        slot.drive_to.center = center;
    }

    pub fn set_gravity(&mut self, gravity: [f32; 2]) {
        self.gravity = gravity;
        self.rapier.gravity = Vector::new(gravity[0], gravity[1]);
        for slot in &self.slots {
            if let Some(body) = self.rapier.bodies.get_mut(slot.handle) {
                body.wake_up(true);
            }
        }
    }

    /// Advances one timeline frame.
    pub fn step_frame(&mut self) -> Result<(), String> {
        for substep in 1..=self.substeps {
            let t = substep as f32 / self.substeps as f32;
            for slot in &self.slots {
                if slot.kind != BodyKind::Animated {
                    continue;
                }
                let from = slot.drive_from;
                let to = slot.drive_to;
                let center = Vector::new(
                    from.center[0] + (to.center[0] - from.center[0]) * t,
                    from.center[1] + (to.center[1] - from.center[1]) * t,
                );
                let angle = from.angle + (to.angle - from.angle) * t;
                if let Some(body) = self.rapier.bodies.get_mut(slot.handle) {
                    body.set_next_kinematic_position(Pose::new(center, angle));
                }
            }
            self.rapier.step();
            if self.links.iter().any(|link| link.break_force > 0.0) {
                self.break_overloaded();
            }
            if !self.liquids.is_empty() {
                self.step_liquids();
            }
        }
        for slot in &mut self.slots {
            slot.drive_from = slot.drive_to;
            let Some(body) = self.rapier.bodies.get(slot.handle) else {
                continue;
            };
            let center = body.translation();
            let raw = body.rotation().angle();
            slot.angle += wrap_angle(raw - slot.angle);
            if slot.kind == BodyKind::Animated {
                slot.angle = slot.drive_to.angle;
            }
            if !center.x.is_finite() || !center.y.is_finite() || !slot.angle.is_finite() {
                return Err("the simulation blew up; try a higher simulation rate".to_string());
            }
        }
        Ok(())
    }

    fn step_liquids(&mut self) {
        let obstacles = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let body = self.rapier.bodies.get(slot.handle)?;
                let center = body.translation();
                let linvel = body.linvel();
                Some(Obstacle {
                    geometry: slot.geometry.clone(),
                    center: [center.x, center.y],
                    angle: body.rotation().angle(),
                    velocity: [linvel.x, linvel.y],
                    spin: body.angvel(),
                    group: slot.group,
                    body: (slot.kind == BodyKind::Dynamic).then_some(index),
                })
            })
            .collect::<Vec<_>>();
        let mut pushes = vec![[0.0_f32; 2]; self.slots.len()];
        for liquid in &mut self.liquids {
            liquid.step(self.substep_dt, self.gravity, &obstacles, &mut pushes);
        }
        for (slot, push) in self.slots.iter().zip(pushes) {
            if push == [0.0, 0.0] || slot.mass <= 0.0 {
                continue;
            }
            if let Some(body) = self.rapier.bodies.get_mut(slot.handle) {
                body.apply_impulse(Vector::new(push[0], push[1]), true);
            }
        }
    }
}

fn add_walls(rapier: &mut PhysicsWorld, walls: &Walls) {
    let Walls { width, height, .. } = *walls;
    // Thick walls that run well past the corners, so nothing slips out
    // between two of them or punches through one.
    let thick = width.max(height);
    let sides = [
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
    for (bit, [x, y], [hx, hy]) in sides {
        if walls.bits & bit == 0 {
            continue;
        }
        rapier.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(x, y)),
            ColliderBuilder::cuboid(hx, hy)
                .restitution(walls.bounce)
                .friction(walls.friction)
                .collision_groups(wall_groups()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> PhysicsScene {
        PhysicsScene {
            end_frame: 47,
            ..PhysicsScene::new(1)
        }
    }

    fn start(body: &PhysicsBody, center: [f32; 2]) -> BodyStart<'_> {
        BodyStart {
            body,
            geometry: ColliderGeometry::Box {
                half_extents: [10.0, 10.0],
                turn: 0.0,
            },
            pose: BodyPose { center, angle: 0.0 },
            velocity: [0.0, 0.0],
            spin: 0.0,
        }
    }

    fn fitted(center: [f32; 2]) -> (Fit, Affine) {
        let affine = Affine::from_transform(Transform2D {
            tx: center[0],
            ty: center[1],
            ..Transform2D::IDENTITY
        });
        (fit_bounds([-10.0, -10.0, 10.0, 10.0], affine), affine)
    }

    #[test]
    fn offset_size_and_turn_reshape_a_box_collider() {
        let mut body = PhysicsBody::new(1, 7);
        body.offset = [5.0, 0.0];
        body.size = [2.0, 1.0];
        body.turn = 90.0;
        let (fit, affine) = fitted([100.0, 100.0]);
        let (pose, geometry) = body_collider(&body, fit, affine, &[]);
        assert_eq!(pose.center, [105.0, 100.0]);
        let ColliderGeometry::Box { half_extents, turn } = geometry else {
            panic!("a box stays a box");
        };
        assert_eq!(half_extents, [20.0, 10.0]);
        assert!((turn - std::f32::consts::FRAC_PI_2).abs() < 1.0e-6);
    }

    #[test]
    fn a_hull_wraps_the_drawn_points() {
        let mut body = PhysicsBody::new(1, 7);
        body.shape = ColliderShape::Hull;
        let (fit, affine) = fitted([0.0, 0.0]);
        // A triangle with a point inside it.
        let art = vec![vec![[-10.0, 10.0], [10.0, 10.0], [0.0, -10.0], [0.0, 5.0]]];
        let (_, geometry) = body_collider(&body, fit, affine, &art);
        let ColliderGeometry::Parts(parts) = geometry else {
            panic!("a hull is a part");
        };
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 3, "the inner point is no corner");
        // No artwork falls back to the box.
        let (_, fallback) = body_collider(&body, fit, affine, &[]);
        assert!(matches!(fallback, ColliderGeometry::Box { .. }));
    }

    /// A U-shaped cup 100 px wide whose walls stand 60 px tall.
    fn cup(world: &mut World, shape: ColliderShape) -> f32 {
        let mut body = PhysicsBody::new(1, 1);
        body.kind = BodyKind::Static;
        body.shape = shape;
        let affine = Affine::from_transform(Transform2D {
            tx: 200.0,
            ty: 240.0,
            ..Transform2D::IDENTITY
        });
        let fit = fit_bounds([-50.0, -30.0, 50.0, 30.0], affine);
        let parts = vec![
            vec![[-50.0, -30.0], [-40.0, -30.0], [-40.0, 30.0], [-50.0, 30.0]],
            vec![[-40.0, 20.0], [40.0, 20.0], [40.0, 30.0], [-40.0, 30.0]],
            vec![[40.0, -30.0], [50.0, -30.0], [50.0, 30.0], [40.0, 30.0]],
        ];
        let (pose, geometry) = body_collider(&body, fit, affine, &parts);
        world.add_body(BodyStart {
            body: &body,
            geometry,
            pose,
            velocity: [0.0, 0.0],
            spin: 0.0,
        });
        // The top of the walls.
        210.0
    }

    fn dropped_ball(world: &mut World) -> usize {
        let mut ball = PhysicsBody::new(1, 2);
        ball.bounce = 0.0;
        world.add_body(BodyStart {
            body: &ball,
            geometry: ColliderGeometry::Circle { radius: 8.0 },
            pose: BodyPose {
                center: [200.0, 100.0],
                angle: 0.0,
            },
            velocity: [0.0, 0.0],
            spin: 0.0,
        })
    }

    #[test]
    fn an_outline_catches_a_ball_inside_where_a_box_keeps_it_on_top() {
        let mut world = World::new(&scene(), [400.0, 300.0], 24.0);
        let top = cup(&mut world, ColliderShape::Outline);
        let ball = dropped_ball(&mut world);
        for _ in 0..48 {
            world.step_frame().unwrap();
        }
        let inside = world.pose(ball).center[1];
        assert!(
            inside > top + 20.0,
            "falls into the cup and rests on its floor: {inside}"
        );

        let mut world = World::new(&scene(), [400.0, 300.0], 24.0);
        let top = cup(&mut world, ColliderShape::Box);
        let ball = dropped_ball(&mut world);
        for _ in 0..48 {
            world.step_frame().unwrap();
        }
        let on_top = world.pose(ball).center[1];
        assert!(on_top < top, "rests on the box: {on_top}");
    }

    #[test]
    fn a_capsule_lies_along_the_longer_side() {
        let geometry = ColliderGeometry::Capsule {
            half_extents: [30.0, 10.0],
            turn: 0.0,
        };
        let [rx, ry] = geometry.reach();
        assert!(
            (rx - 30.0).abs() < 0.5 && (ry - 10.0).abs() < 0.5,
            "{rx} {ry}"
        );
    }

    #[test]
    fn a_new_shape_takes_over_mid_simulation() {
        let mut world = World::new(&scene(), [400.0, 300.0], 24.0);
        let mut body = PhysicsBody::new(1, 7);
        body.bounce = 0.0;
        let index = world.add_body(start(&body, [200.0, 50.0]));
        world.set_geometry(
            index,
            ColliderGeometry::Box {
                half_extents: [10.0, 40.0],
                turn: 0.0,
            },
        );
        for _ in 0..47 {
            world.step_frame().unwrap();
        }
        let rest = world.pose(index).center[1];
        assert!((rest - 260.0).abs() < 2.0, "stands 40 px tall: {rest}");
    }

    #[test]
    fn a_dropped_box_lands_on_the_floor() {
        let mut world = World::new(&scene(), [400.0, 300.0], 24.0);
        let mut body = PhysicsBody::new(1, 7);
        body.bounce = 0.0;
        let index = world.add_body(start(&body, [200.0, 50.0]));
        for _ in 0..47 {
            world.step_frame().unwrap();
        }
        let rest = world.pose(index).center[1];
        assert!((rest - 290.0).abs() < 2.0, "rests on the floor, got {rest}");
    }

    #[test]
    fn pushes_and_gravity_change_a_live_world() {
        let mut floating = scene();
        floating.gravity = [0.0, 0.0];
        floating.walls = 0;
        let mut world = World::new(&floating, [400.0, 300.0], 24.0);
        let mut body = PhysicsBody::new(1, 7);
        body.angular_damping = 0.0;
        let index = world.add_body(start(&body, [100.0, 100.0]));
        world.push(index, [240.0, 0.0]);
        for _ in 0..24 {
            world.step_frame().unwrap();
        }
        let x = world.pose(index).center[0];
        assert!((x - 340.0).abs() < 1.0, "a second at 240 px/s, got {x}");
        world.set_gravity([0.0, 100.0]);
        world.set_velocity(index, [0.0, 0.0]);
        for _ in 0..24 {
            world.step_frame().unwrap();
        }
        assert!(world.pose(index).center[1] > 140.0);
    }

    #[test]
    fn light_bodies_float_higher_in_liquid_than_heavy_ones() {
        let settle_box = |mass: f32| {
            let mut pool = scene();
            pool.walls = WALL_LEFT | WALL_RIGHT | WALL_BOTTOM;
            let mut world = World::new(&pool, [200.0, 200.0], 24.0);
            let mut liquid = PhysicsLiquid::new(1, 1, [0.0, 110.0, 200.0, 88.0]);
            liquid.buoyancy = 2.0;
            world.add_liquid(&liquid);
            let mut body = PhysicsBody::new(1, 7);
            body.mass = mass;
            body.fixed_rotation = true;
            let index = world.add_body(start(&body, [100.0, 60.0]));
            for _ in 0..24 * 4 {
                world.step_frame().unwrap();
            }
            world.pose(index).center[1]
        };
        let light = settle_box(0.02);
        let heavy = settle_box(20.0);
        assert!(heavy > 180.0, "the heavy box sinks to the floor: {heavy}");
        assert!(
            light < heavy - 20.0,
            "the light box floats: {light} vs {heavy}"
        );
    }

    #[test]
    fn turning_keeps_the_object_around_its_collider() {
        let turned = posed_transform(
            Transform2D {
                tx: 10.0,
                ty: 0.0,
                ..Transform2D::IDENTITY
            },
            BodyPose {
                center: [20.0, 0.0],
                angle: 0.0,
            },
            BodyPose {
                center: [20.0, 0.0],
                angle: std::f32::consts::FRAC_PI_2,
            },
        );
        assert!((turned.tx - 20.0).abs() < 1.0e-4);
        assert!((turned.ty + 10.0).abs() < 1.0e-4);
        assert!((turned.rotation - std::f32::consts::FRAC_PI_2).abs() < 1.0e-5);
    }

    fn floating() -> PhysicsScene {
        PhysicsScene {
            walls: 0,
            ..scene()
        }
    }

    fn pinned_box(world: &mut World, center: [f32; 2]) -> usize {
        let mut anchor = PhysicsBody::new(1, 1);
        anchor.kind = BodyKind::Static;
        world.add_body(start(&anchor, center))
    }

    fn hanging_ball(world: &mut World, center: [f32; 2]) -> usize {
        let mut ball = PhysicsBody::new(1, 2);
        ball.angular_damping = 0.0;
        world.add_body(BodyStart {
            body: &ball,
            geometry: ColliderGeometry::Circle { radius: 8.0 },
            pose: BodyPose { center, angle: 0.0 },
            velocity: [0.0, 0.0],
            spin: 0.0,
        })
    }

    fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    #[test]
    fn a_rope_holds_a_ball_at_its_length_and_sags_when_slack() {
        let mut world = World::new(&floating(), [400.0, 400.0], 24.0);
        let top = pinned_box(&mut world, [200.0, 50.0]);
        let ball = hanging_ball(&mut world, [200.0, 120.0]);
        let mut rope = PhysicsLink::new(1, 1, LinkKind::Rope, 1, 2);
        rope.length = 150.0;
        let link = world.add_link(LinkStart {
            link: &rope,
            from: Some(top),
            to: ball,
        });
        for _ in 0..72 {
            world.step_frame().unwrap();
        }
        let hang = world.pose(ball).center;
        assert!(
            (hang[1] - 200.0).abs() < 6.0 && (hang[0] - 200.0).abs() < 3.0,
            "falls until the rope is taut, 150 px below: {hang:?}"
        );
        let lines = world.link_lines(link);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 12, "two ends and ten pieces");
        assert!(distance(lines[0][0], [200.0, 50.0]) < 0.5);
        assert!(distance(*lines[0].last().unwrap(), hang) < 0.5);
    }

    #[test]
    fn a_tied_ball_swings_along_when_its_anchor_moves() {
        let mut world = World::new(&floating(), [800.0, 400.0], 24.0);
        let mut carrier = PhysicsBody::new(1, 1);
        carrier.kind = BodyKind::Animated;
        let top = world.add_body(start(&carrier, [100.0, 50.0]));
        let ball = hanging_ball(&mut world, [100.0, 150.0]);
        let mut rope = PhysicsLink::new(1, 1, LinkKind::Rope, 1, 2);
        rope.length = 100.0;
        rope.damping = 0.0;
        world.add_link(LinkStart {
            link: &rope,
            from: Some(top),
            to: ball,
        });
        // The carrier slides right, then stops.
        for frame in 1..=24 {
            world.drive(
                top,
                BodyPose {
                    center: [100.0 + frame as f32 * 10.0, 50.0],
                    angle: 0.0,
                },
            );
            world.step_frame().unwrap();
        }
        let mut swing = 0.0_f32;
        for _ in 0..24 {
            world.step_frame().unwrap();
            swing = swing.max(world.pose(ball).center[0] - 340.0);
        }
        assert!(
            world.pose(ball).center[0] > 250.0,
            "dragged along: {:?}",
            world.pose(ball)
        );
        assert!(swing > 10.0, "and swings on past the stop: {swing}");
    }

    #[test]
    fn a_spring_bobs_about_its_rest_length() {
        let mut world = World::new(&floating(), [400.0, 600.0], 24.0);
        let top = pinned_box(&mut world, [200.0, 50.0]);
        let ball = hanging_ball(&mut world, [200.0, 150.0]);
        let mut spring = PhysicsLink::new(1, 1, LinkKind::Spring, 1, 2);
        spring.length = 100.0;
        spring.damping = 0.0;
        world.add_link(LinkStart {
            link: &spring,
            from: Some(top),
            to: ball,
        });
        let mut lowest = 0.0_f32;
        let mut highest_after = f32::MAX;
        for frame in 0..48 {
            world.step_frame().unwrap();
            let y = world.pose(ball).center[1];
            lowest = lowest.max(y);
            if frame > 4 {
                highest_after = highest_after.min(y);
            }
        }
        assert!(lowest > 160.0, "gravity stretches it: {lowest}");
        assert!(highest_after < lowest - 10.0, "and it bobs back up");
    }

    #[test]
    fn a_weld_carries_an_object_rigidly() {
        let mut world = World::new(&floating(), [800.0, 400.0], 24.0);
        let mut carrier = PhysicsBody::new(1, 1);
        carrier.kind = BodyKind::Animated;
        let top = world.add_body(start(&carrier, [100.0, 100.0]));
        let ball = hanging_ball(&mut world, [100.0, 130.0]);
        let mut weld = PhysicsLink::new(1, 1, LinkKind::Weld, 1, 2);
        weld.stiffness = 1.0;
        world.add_link(LinkStart {
            link: &weld,
            from: Some(top),
            to: ball,
        });
        for frame in 1..=24 {
            world.drive(
                top,
                BodyPose {
                    center: [100.0 + frame as f32 * 5.0, 100.0],
                    angle: 0.0,
                },
            );
            world.step_frame().unwrap();
        }
        let at = world.pose(ball).center;
        assert!(
            distance(at, [220.0, 130.0]) < 3.0,
            "stays 30 px below: {at:?}"
        );
    }

    #[test]
    fn a_hinge_lets_it_swing_around_the_pin() {
        let mut world = World::new(&floating(), [400.0, 400.0], 24.0);
        let ball = hanging_ball(&mut world, [260.0, 100.0]);
        let mut pin = PhysicsLink::new(1, 1, LinkKind::Hinge, 0, 2);
        // Pinned 60 px left of the ball's middle.
        pin.to_anchor = [-60.0, 0.0];
        world.add_link(LinkStart {
            link: &pin,
            from: None,
            to: ball,
        });
        for _ in 0..12 {
            world.step_frame().unwrap();
        }
        let at = world.pose(ball).center;
        assert!(
            (distance(at, [200.0, 100.0]) - 60.0).abs() < 2.0,
            "stays 60 px from the pin: {at:?}"
        );
        assert!(at[1] > 130.0, "and swings down: {at:?}");
    }

    #[test]
    fn a_rope_snaps_under_too_much_weight_and_cuts_on_demand() {
        let mut world = World::new(&floating(), [400.0, 800.0], 24.0);
        let top = pinned_box(&mut world, [200.0, 50.0]);
        let ball = hanging_ball(&mut world, [200.0, 100.0]);
        let mut weak = PhysicsLink::new(1, 1, LinkKind::Rope, 1, 2);
        weak.length = 60.0;
        weak.segments = 4;
        weak.break_force = 300.0;
        let link = world.add_link(LinkStart {
            link: &weak,
            from: Some(top),
            to: ball,
        });
        for _ in 0..48 {
            world.step_frame().unwrap();
        }
        assert!(world.link_broken(link));
        assert!(world.pose(ball).center[1] > 300.0, "the ball falls away");

        let mut world = World::new(&floating(), [400.0, 800.0], 24.0);
        let top = pinned_box(&mut world, [200.0, 50.0]);
        let ball = hanging_ball(&mut world, [200.0, 100.0]);
        let mut strong = weak.clone();
        strong.break_force = 0.0;
        let link = world.add_link(LinkStart {
            link: &strong,
            from: Some(top),
            to: ball,
        });
        for _ in 0..24 {
            world.step_frame().unwrap();
        }
        assert!(!world.link_broken(link));
        world.cut_link(link);
        for _ in 0..24 {
            world.step_frame().unwrap();
        }
        assert!(world.link_broken(link));
        assert_eq!(
            world.link_lines(link).len(),
            2,
            "cut in the middle: two ends"
        );
        assert!(world.pose(ball).center[1] > 200.0);
    }
}
