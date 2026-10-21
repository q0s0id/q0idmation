//! Liquid as particles that push apart when crowded and pull together when
//! sparse: the double density relaxation of Clavet, Beaudoin and Poulin,
//! "Particle-based Viscoelastic Fluid Simulation" (2005). It is stable at
//! animation frame rates, keeps a surface that holds together in drops, and
//! needs no pressure solve.

use std::collections::HashMap;

use q0s_format::physics::{
    LiquidSource, PhysicsLiquid, WALL_BOTTOM, WALL_LEFT, WALL_RIGHT, WALL_TOP,
};

use crate::rigid::ColliderGeometry;
use crate::rng::Rng;

/// Interaction radius in particle spacings.
const REACH: f32 = 2.0;
/// Liquid slides along walls and bodies with this much grip.
const SURFACE_FRICTION: f32 = 0.08;
/// The old default, 24 fps with four steps/frame, at which liquid stiffness
/// and surface grip were tuned. Convert those coefficients to seconds so
/// raising movie fps or solver quality cannot overpower gravity.
const REFERENCE_RATE: f32 = 96.0;
/// Particles this far off a side with no wall are dropped.
const OFF_STAGE: f32 = 2000.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Walls {
    pub bits: u8,
    pub width: f32,
    pub height: f32,
    pub bounce: f32,
    pub friction: f32,
}

/// A body the liquid runs into, as it is during one step.
pub(crate) struct Obstacle {
    pub geometry: ColliderGeometry,
    pub center: [f32; 2],
    pub angle: f32,
    pub velocity: [f32; 2],
    pub spin: f32,
    pub group: u8,
    /// Index of the dynamic body the liquid pushes back on.
    pub body: Option<usize>,
}

pub(crate) struct Liquid {
    params: PhysicsLiquid,
    walls: Walls,
    positions: Vec<[f32; 2]>,
    previous: Vec<[f32; 2]>,
    velocities: Vec<[f32; 2]>,
    reach: f32,
    rest_density: f32,
    radius: f32,
    particle_mass: f32,
    carry: f32,
    rng: Rng,
    grid: Grid,
}

impl Liquid {
    pub(crate) fn new(params: &PhysicsLiquid, walls: Walls) -> Self {
        let spacing = params.spacing.max(1.0);
        let reach = spacing * REACH;
        let mut liquid = Self {
            params: params.clone(),
            walls,
            positions: Vec::new(),
            previous: Vec::new(),
            velocities: Vec::new(),
            reach,
            rest_density: lattice_density(spacing, reach),
            radius: spacing * 0.5,
            // Rapier's length unit is 100 px: a particle weighs its area.
            particle_mass: (spacing / 100.0).powi(2),
            carry: 0.0,
            rng: Rng::new(u64::from(params.liquid_id) << 16 | u64::from(params.q0rg_id)),
            grid: Grid::default(),
        };
        if params.source == LiquidSource::Block {
            liquid.fill_region();
        }
        liquid
    }

    pub(crate) fn positions(&self) -> &[[f32; 2]] {
        &self.positions
    }

    fn fill_region(&mut self) {
        let [x, y, width, height] = self.params.region;
        let spacing = self.params.spacing;
        let columns = (width / spacing).floor().max(1.0) as usize;
        let rows = (height / spacing).floor().max(1.0) as usize;
        let max = usize::from(self.params.max_particles);
        // From the bottom up, so a capped block still sits where it was drawn.
        'rows: for row in (0..rows).rev() {
            for column in 0..columns {
                if self.positions.len() >= max {
                    break 'rows;
                }
                let jitter = spacing * 0.05;
                let position = [
                    x + (column as f32 + 0.5) * spacing + self.rng.signed() * jitter,
                    y + (row as f32 + 0.5) * spacing + self.rng.signed() * jitter,
                ];
                self.spawn(position, [0.0, 0.0]);
            }
        }
    }

    fn spawn(&mut self, position: [f32; 2], velocity: [f32; 2]) {
        self.positions.push(position);
        self.previous.push(position);
        self.velocities.push(velocity);
    }

    fn pour(&mut self, dt: f32) {
        if self.params.source != LiquidSource::Stream {
            return;
        }
        let [x, y, width, height] = self.params.region;
        self.carry += self.params.stream_rate * dt;
        while self.carry >= 1.0 && self.positions.len() < usize::from(self.params.max_particles) {
            self.carry -= 1.0;
            let position = [
                x + width * 0.5 + self.rng.signed() * width * 0.5,
                y + height * 0.5 + self.rng.signed() * height * 0.5,
            ];
            let wobble = self.params.spacing * 2.0;
            let velocity = [
                self.params.stream_velocity[0] + self.rng.signed() * wobble,
                self.params.stream_velocity[1] + self.rng.signed() * wobble,
            ];
            self.spawn(position, velocity);
        }
        self.carry = self.carry.min(1.0);
    }

    /// One substep. Reactions on dynamic bodies are added to `pushes` as
    /// impulses, by body index.
    pub(crate) fn step(
        &mut self,
        dt: f32,
        gravity: [f32; 2],
        obstacles: &[Obstacle],
        pushes: &mut [[f32; 2]],
    ) {
        self.pour(dt);
        if self.positions.is_empty() {
            return;
        }
        let gravity = [
            gravity[0] * self.params.gravity_scale,
            gravity[1] * self.params.gravity_scale,
        ];
        for velocity in &mut self.velocities {
            velocity[0] += gravity[0] * dt;
            velocity[1] += gravity[1] * dt;
        }
        self.grid.rebuild(&self.positions, self.reach);
        self.apply_viscosity(dt);
        for ((position, previous), velocity) in self
            .positions
            .iter_mut()
            .zip(&mut self.previous)
            .zip(&self.velocities)
        {
            *previous = *position;
            position[0] += velocity[0] * dt;
            position[1] += velocity[1] * dt;
        }
        self.grid.rebuild(&self.positions, self.reach);
        self.relax_density(dt);
        self.collide(dt, obstacles, pushes);
        // Velocities follow from where the particles ended up; a step never
        // moves one further than a spacing and a half.
        let limit = self.params.spacing * 1.5;
        for ((position, previous), velocity) in self
            .positions
            .iter_mut()
            .zip(&self.previous)
            .zip(&mut self.velocities)
        {
            let mut moved = [position[0] - previous[0], position[1] - previous[1]];
            let distance = moved[0].hypot(moved[1]);
            if distance > limit {
                let scale = limit / distance;
                moved = [moved[0] * scale, moved[1] * scale];
                *position = [previous[0] + moved[0], previous[1] + moved[1]];
            }
            *velocity = [moved[0] / dt, moved[1] / dt];
        }
        self.drop_strays();
    }

    fn apply_viscosity(&mut self, dt: f32) {
        let sigma = (self.params.viscosity * 5.0).min(0.5 / dt);
        let beta = self.params.viscosity * 0.01;
        if sigma <= 0.0 && beta <= 0.0 {
            return;
        }
        let reach = self.reach;
        for i in 0..self.positions.len() {
            let pi = self.positions[i];
            let neighbors = self.grid.neighbors(pi, reach);
            for j in neighbors {
                if j <= i {
                    continue;
                }
                let pj = self.positions[j];
                let delta = [pj[0] - pi[0], pj[1] - pi[1]];
                let distance = delta[0].hypot(delta[1]);
                if distance <= 1.0e-6 || distance >= reach {
                    continue;
                }
                let q = distance / reach;
                let normal = [delta[0] / distance, delta[1] / distance];
                let vi = self.velocities[i];
                let vj = self.velocities[j];
                let inward = (vi[0] - vj[0]) * normal[0] + (vi[1] - vj[1]) * normal[1];
                if inward <= 0.0 {
                    continue;
                }
                let impulse = dt * (1.0 - q) * (sigma * inward + beta * inward * inward);
                let half = [normal[0] * impulse * 0.5, normal[1] * impulse * 0.5];
                self.velocities[i][0] -= half[0];
                self.velocities[i][1] -= half[1];
                self.velocities[j][0] += half[0];
                self.velocities[j][1] += half[1];
            }
        }
    }

    fn relax_density(&mut self, dt: f32) {
        let reach = self.reach;
        let stiffness = self.params.stiffness;
        let cohesion = self.params.cohesion;
        // Pressure is an acceleration; its positional correction scales with
        // dt². A constant offset each step made its force grow quadratically
        // with the step rate while gravity still used seconds.
        let scale = 0.25 * self.params.spacing * (dt * REFERENCE_RATE).powi(2);
        let mut neighbors = Vec::new();
        for i in 0..self.positions.len() {
            let pi = self.positions[i];
            neighbors.clear();
            let mut density = 0.0;
            let mut near_density = 0.0;
            for j in self.grid.neighbors(pi, reach) {
                if j == i {
                    continue;
                }
                let pj = self.positions[j];
                let delta = [pj[0] - pi[0], pj[1] - pi[1]];
                let distance = delta[0].hypot(delta[1]);
                if distance >= reach {
                    continue;
                }
                let q = 1.0 - distance / reach;
                density += q * q;
                near_density += q * q * q;
                neighbors.push((j, delta, distance, q));
            }
            let mut pressure = stiffness * (density - self.rest_density);
            if pressure < 0.0 {
                // Too sparse: pull together, as much as the liquid clings.
                pressure *= cohesion * 0.5;
            }
            let near_pressure = stiffness * 2.0 * near_density;
            let mut moved = [0.0_f32, 0.0];
            for &(j, delta, distance, q) in &neighbors {
                let normal = if distance > 1.0e-6 {
                    [delta[0] / distance, delta[1] / distance]
                } else {
                    let angle = (i as f32 * 2.399_963).sin_cos();
                    [angle.0, angle.1]
                };
                let push = scale * (pressure * q + near_pressure * q * q);
                let half = [normal[0] * push * 0.5, normal[1] * push * 0.5];
                self.positions[j][0] += half[0];
                self.positions[j][1] += half[1];
                moved[0] -= half[0];
                moved[1] -= half[1];
            }
            self.positions[i][0] += moved[0];
            self.positions[i][1] += moved[1];
        }
    }

    fn collide(&mut self, dt: f32, obstacles: &[Obstacle], pushes: &mut [[f32; 2]]) {
        let radius = self.radius;
        let group = self.params.group;
        let mass = self.particle_mass * self.params.buoyancy;
        let surface_grip = 1.0 - (1.0 - SURFACE_FRICTION).powf(dt * REFERENCE_RATE);
        let wall_grip =
            1.0 - (1.0 - (self.walls.friction * 0.1).clamp(0.0, 1.0)).powf(dt * REFERENCE_RATE);
        for i in 0..self.positions.len() {
            for obstacle in obstacles {
                if group == 0 || obstacle.group != group {
                    continue;
                }
                let Some((normal, depth)) = penetration(obstacle, self.positions[i], radius) else {
                    continue;
                };
                let position = &mut self.positions[i];
                position[0] += normal[0] * depth;
                position[1] += normal[1] * depth;
                // Velocity against the surface, which may itself be moving.
                let previous = self.previous[i];
                let arm = [
                    position[0] - obstacle.center[0],
                    position[1] - obstacle.center[1],
                ];
                let surface = [
                    obstacle.velocity[0] - obstacle.spin * arm[1],
                    obstacle.velocity[1] + obstacle.spin * arm[0],
                ];
                let relative = [
                    (position[0] - previous[0]) / dt - surface[0],
                    (position[1] - previous[1]) / dt - surface[1],
                ];
                let into = relative[0] * normal[0] + relative[1] * normal[1];
                if into >= 0.0 {
                    continue;
                }
                let normal_part = [normal[0] * into, normal[1] * into];
                let slide = [relative[0] - normal_part[0], relative[1] - normal_part[1]];
                let change = [
                    -normal_part[0] - surface_grip * slide[0],
                    -normal_part[1] - surface_grip * slide[1],
                ];
                position[0] += change[0] * dt;
                position[1] += change[1] * dt;
                if let Some(body) = obstacle.body {
                    if let Some(push) = pushes.get_mut(body) {
                        push[0] -= change[0] * mass;
                        push[1] -= change[1] * mass;
                    }
                }
            }
            self.keep_inside_walls(i, wall_grip);
        }
    }

    fn keep_inside_walls(&mut self, i: usize, grip: f32) {
        let Walls {
            bits,
            width,
            height,
            ..
        } = self.walls;
        let radius = self.radius;
        let position = &mut self.positions[i];
        let previous = &mut self.previous[i];
        let mut hit = |axis: usize, limit: f32, below: bool| {
            let outside = if below {
                position[axis] < limit
            } else {
                position[axis] > limit
            };
            if !outside {
                return;
            }
            position[axis] = limit;
            // Liquid does not bounce off walls; it slides along them.
            let other = 1 - axis;
            let slide = position[other] - previous[other];
            previous[axis] = position[axis];
            previous[other] = position[other] - slide * (1.0 - grip);
        };
        if bits & WALL_LEFT != 0 {
            hit(0, radius, true);
        }
        if bits & WALL_RIGHT != 0 {
            hit(0, width - radius, false);
        }
        if bits & WALL_TOP != 0 {
            hit(1, radius, true);
        }
        if bits & WALL_BOTTOM != 0 {
            hit(1, height - radius, false);
        }
    }

    fn drop_strays(&mut self) {
        let Walls { width, height, .. } = self.walls;
        let mut i = 0;
        while i < self.positions.len() {
            let [x, y] = self.positions[i];
            let stray = !x.is_finite()
                || !y.is_finite()
                || x < -OFF_STAGE
                || y < -OFF_STAGE
                || x > width + OFF_STAGE
                || y > height + OFF_STAGE;
            if stray {
                self.positions.swap_remove(i);
                self.previous.swap_remove(i);
                self.velocities.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// How far a particle of `radius` at `point` sits inside `obstacle`, and the
/// way out.
fn penetration(obstacle: &Obstacle, point: [f32; 2], radius: f32) -> Option<([f32; 2], f32)> {
    let delta = [point[0] - obstacle.center[0], point[1] - obstacle.center[1]];
    // Into the obstacle's frame, and the way out back to the stage's.
    let into = |delta: [f32; 2], angle: f32| {
        let (sin, cos) = angle.sin_cos();
        [
            cos * delta[0] + sin * delta[1],
            -sin * delta[0] + cos * delta[1],
        ]
    };
    let out = |normal: [f32; 2], angle: f32| {
        let (sin, cos) = angle.sin_cos();
        [
            cos * normal[0] - sin * normal[1],
            sin * normal[0] + cos * normal[1],
        ]
    };
    match &obstacle.geometry {
        ColliderGeometry::Circle { radius: reach } => {
            let reach = reach + radius;
            let distance = delta[0].hypot(delta[1]);
            if distance >= reach {
                return None;
            }
            let normal = if distance > 1.0e-6 {
                [delta[0] / distance, delta[1] / distance]
            } else {
                [0.0, -1.0]
            };
            Some((normal, reach - distance))
        }
        ColliderGeometry::Box { half_extents, turn } => {
            let angle = obstacle.angle + turn;
            let local = into(delta, angle);
            let hx = half_extents[0] + radius;
            let hy = half_extents[1] + radius;
            let over_x = hx - local[0].abs();
            let over_y = hy - local[1].abs();
            if over_x <= 0.0 || over_y <= 0.0 {
                return None;
            }
            let (local_normal, depth) = if over_x < over_y {
                ([local[0].signum(), 0.0], over_x)
            } else {
                ([0.0, local[1].signum()], over_y)
            };
            Some((out(local_normal, angle), depth))
        }
        ColliderGeometry::Capsule { half_extents, turn } => {
            let angle = obstacle.angle + turn;
            let local = into(delta, angle);
            let (round, reach) = (
                half_extents[0].min(half_extents[1]),
                (half_extents[0].max(half_extents[1]) - half_extents[0].min(half_extents[1]))
                    .max(0.0),
            );
            // The nearest point of the capsule's spine.
            let nearest = if half_extents[0] >= half_extents[1] {
                [local[0].clamp(-reach, reach), 0.0]
            } else {
                [0.0, local[1].clamp(-reach, reach)]
            };
            let away = [local[0] - nearest[0], local[1] - nearest[1]];
            let distance = away[0].hypot(away[1]);
            let total = round + radius;
            if distance >= total {
                return None;
            }
            let local_normal = if distance > 1.0e-6 {
                [away[0] / distance, away[1] / distance]
            } else {
                [0.0, -1.0]
            };
            Some((out(local_normal, angle), total - distance))
        }
        ColliderGeometry::Parts(parts) => {
            let local = into(delta, obstacle.angle);
            parts
                .iter()
                .filter_map(|part| convex_penetration(part, local, radius))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(normal, depth)| (out(normal, obstacle.angle), depth))
        }
    }
}

/// How far a particle of `radius` at `point` sits inside the convex loop
/// `part`, and the way out through its nearest edge.
fn convex_penetration(part: &[[f32; 2]], point: [f32; 2], radius: f32) -> Option<([f32; 2], f32)> {
    if part.len() < 3 {
        return None;
    }
    // Outward normals depend on which way the loop runs.
    let area = part
        .iter()
        .zip(part.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f32>();
    let outward = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut best: Option<([f32; 2], f32)> = None;
    for (a, b) in part.iter().zip(part.iter().cycle().skip(1)) {
        let edge = [b[0] - a[0], b[1] - a[1]];
        let length = edge[0].hypot(edge[1]);
        if length <= 1.0e-6 {
            continue;
        }
        let normal = [outward * edge[1] / length, -outward * edge[0] / length];
        let distance = (point[0] - a[0]) * normal[0] + (point[1] - a[1]) * normal[1];
        if distance >= radius {
            return None;
        }
        if best.is_none_or(|(_, depth)| radius - distance < depth) {
            best = Some((normal, radius - distance));
        }
    }
    best
}

/// The density a particle has inside a square lattice of `spacing`: what the
/// liquid relaxes towards.
fn lattice_density(spacing: f32, reach: f32) -> f32 {
    let mut density = 0.0;
    for dx in -3_i32..=3 {
        for dy in -3_i32..=3 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let distance = spacing * ((dx * dx + dy * dy) as f32).sqrt();
            if distance < reach {
                let q = 1.0 - distance / reach;
                density += q * q;
            }
        }
    }
    density
}

/// Particles bucketed by cell, a reach wide, so neighbours are found in the
/// nine cells around one.
#[derive(Default)]
struct Grid {
    cell: f32,
    order: Vec<(u64, u32)>,
    spans: HashMap<u64, (u32, u32)>,
}

impl Grid {
    fn key(cx: i32, cy: i32) -> u64 {
        (u64::from(cx as u32) << 32) | u64::from(cy as u32)
    }

    fn cell_of(&self, point: [f32; 2]) -> (i32, i32) {
        (
            (point[0] / self.cell).floor() as i32,
            (point[1] / self.cell).floor() as i32,
        )
    }

    fn rebuild(&mut self, points: &[[f32; 2]], cell: f32) {
        self.cell = cell;
        self.order.clear();
        self.spans.clear();
        for (index, point) in points.iter().enumerate() {
            let (cx, cy) = self.cell_of(*point);
            self.order.push((Self::key(cx, cy), index as u32));
        }
        self.order.sort_unstable();
        let mut start = 0;
        while start < self.order.len() {
            let key = self.order[start].0;
            let mut end = start + 1;
            while end < self.order.len() && self.order[end].0 == key {
                end += 1;
            }
            self.spans.insert(key, (start as u32, end as u32));
            start = end;
        }
    }

    /// Indices of the particles in the cells around `point`.
    fn neighbors(&self, point: [f32; 2], _reach: f32) -> Vec<usize> {
        let (cx, cy) = self.cell_of(point);
        let mut found = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(&(start, end)) = self.spans.get(&Self::key(cx + dx, cy + dy)) {
                    found.extend(
                        self.order[start as usize..end as usize]
                            .iter()
                            .map(|(_, index)| *index as usize),
                    );
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walls() -> Walls {
        Walls {
            bits: WALL_LEFT | WALL_RIGHT | WALL_BOTTOM,
            width: 200.0,
            height: 200.0,
            bounce: 0.0,
            friction: 0.5,
        }
    }

    fn settle(liquid: &mut Liquid, obstacles: &[Obstacle], frames: usize) {
        let mut pushes = vec![[0.0; 2]; 4];
        for _ in 0..frames * 4 {
            liquid.step(1.0 / 96.0, [0.0, 980.0], obstacles, &mut pushes);
        }
    }

    #[test]
    fn a_block_of_liquid_settles_into_a_pool() {
        let params = PhysicsLiquid::new(1, 1, [40.0, 20.0, 80.0, 80.0]);
        let mut liquid = Liquid::new(&params, walls());
        let count = liquid.positions().len();
        assert_eq!(count, 100);
        settle(&mut liquid, &[], 72);
        assert_eq!(liquid.positions().len(), count, "none lost");
        let top = liquid
            .positions()
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        let left = liquid
            .positions()
            .iter()
            .map(|p| p[0])
            .fold(f32::INFINITY, f32::min);
        let right = liquid
            .positions()
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(liquid.positions().iter().all(|p| p[1] <= 196.0 + 0.01));
        assert!(
            right - left > 150.0,
            "spreads across the floor: {left}..{right}"
        );
        // 100 particles over a 200 px floor stand about four rows deep.
        assert!(top > 150.0, "lies flat, top at {top}");
    }

    #[test]
    fn liquid_pours_around_a_body_and_pushes_back() {
        let params = PhysicsLiquid::new(1, 1, [20.0, 20.0, 160.0, 60.0]);
        let mut liquid = Liquid::new(&params, walls());
        let block = Obstacle {
            geometry: ColliderGeometry::Box {
                half_extents: [20.0, 20.0],
                turn: 0.0,
            },
            center: [100.0, 170.0],
            angle: 0.0,
            velocity: [0.0, 0.0],
            spin: 0.0,
            group: 1,
            body: Some(0),
        };
        let mut pushes = vec![[0.0; 2]; 1];
        for _ in 0..96 * 2 {
            liquid.step(
                1.0 / 96.0,
                [0.0, 980.0],
                std::slice::from_ref(&block),
                &mut pushes,
            );
        }
        assert!(
            liquid
                .positions()
                .iter()
                .all(|p| penetration(&block, *p, 0.0).is_none_or(|(_, depth)| depth < 4.0)),
            "nothing stays inside the block"
        );
        assert!(
            pushes[0] != [0.0, 0.0],
            "the liquid pushes back on the block"
        );
    }

    #[test]
    fn a_stream_keeps_pouring_up_to_its_cap() {
        let mut params = PhysicsLiquid::new(1, 1, [90.0, 10.0, 20.0, 4.0]);
        params.source = LiquidSource::Stream;
        params.stream_rate = 240.0;
        params.max_particles = 50;
        let mut liquid = Liquid::new(&params, walls());
        assert!(liquid.positions().is_empty());
        settle(&mut liquid, &[], 6);
        assert_eq!(liquid.positions().len(), 50);
    }
}
