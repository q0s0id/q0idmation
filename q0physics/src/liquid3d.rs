//! Liquid in 3D: the double density relaxation of the 2D liquid (Clavet,
//! Beaudoin and Poulin, 2005) in three dimensions. Drops push apart when
//! crowded and pull together when sparse, slide over a floor and still
//! bodies, and may be held in a tank.
//!
//! Stage space: x right, y down, z away from the camera, in stage pixels.

use std::collections::HashMap;

/// Interaction radius in drop spacings.
const REACH: f32 = 2.0;
/// Steps a second the stiffness was tuned at, as in 2D.
const REFERENCE_RATE: f32 = 96.0;
/// Steps in every movie frame.
const SUBSTEPS: u32 = 4;
/// Drops slide over surfaces with this much grip.
const GRIP: f32 = 0.08;
/// The most drops one liquid may hold.
pub const MAX_DROPS: usize = 6000;
/// Drops further than this from the stage are let go.
const FAR: f32 = 20_000.0;

/// How a liquid behaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings3 {
    /// Stage pixels between drops.
    pub spacing: f32,
    /// 0 runs like water, 1 oozes like honey.
    pub thickness: f32,
    /// How much it clings together.
    pub cling: f32,
}

/// Something the liquid runs into, standing still.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Obstacle3 {
    /// A box: its middle, half its size along its own axes, and its turn
    /// as a unit quaternion (x, y, z, w).
    Box {
        centre: [f32; 3],
        half: [f32; 3],
        rotation: [f32; 4],
    },
    Ball {
        centre: [f32; 3],
        radius: f32,
    },
}

/// What holds the liquid.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tank3 {
    /// The floor's height, down the stage.
    pub floor: Option<f32>,
    /// Walls at these x and z: left, right, front, back.
    pub walls: Option<[f32; 4]>,
}

type P = [f32; 3];

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: P, k: f32) -> P {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(a: P) -> f32 {
    dot(a, a).sqrt()
}

/// Turns `v` by the unit quaternion `q`.
fn rotate(q: [f32; 4], v: P) -> P {
    let [x, y, z, w] = q;
    let u = [x, y, z];
    let uv = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let uuv = [
        u[1] * uv[2] - u[2] * uv[1],
        u[2] * uv[0] - u[0] * uv[2],
        u[0] * uv[1] - u[1] * uv[0],
    ];
    add(v, add(scale(uv, 2.0 * w), scale(uuv, 2.0)))
}

fn inverse(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// How far `point` is inside `obstacle` grown by `radius`, and which way out.
fn penetration(obstacle: &Obstacle3, point: P, radius: f32) -> Option<(P, f32)> {
    match *obstacle {
        Obstacle3::Ball { centre, radius: r } => {
            let d = sub(point, centre);
            let distance = length(d);
            let reach = r + radius;
            if distance >= reach {
                return None;
            }
            let normal = if distance > 1.0e-6 {
                scale(d, 1.0 / distance)
            } else {
                [0.0, -1.0, 0.0]
            };
            Some((normal, reach - distance))
        }
        Obstacle3::Box {
            centre,
            half,
            rotation,
        } => {
            let local = rotate(inverse(rotation), sub(point, centre));
            let grown = [half[0] + radius, half[1] + radius, half[2] + radius];
            if (0..3).any(|a| local[a].abs() >= grown[a]) {
                return None;
            }
            // Out through the nearest face.
            let (axis, depth) = (0..3)
                .map(|a| (a, grown[a] - local[a].abs()))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            let mut normal = [0.0; 3];
            normal[axis] = local[axis].signum();
            Some((rotate(rotation, normal), depth))
        }
    }
}

/// Drops bucketed in cubes a reach wide.
#[derive(Default)]
struct Grid {
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl Grid {
    fn key(&self, p: P) -> (i32, i32, i32) {
        (
            (p[0] / self.cell).floor() as i32,
            (p[1] / self.cell).floor() as i32,
            (p[2] / self.cell).floor() as i32,
        )
    }

    fn rebuild(&mut self, points: &[P], cell: f32) {
        self.cell = cell;
        for list in self.cells.values_mut() {
            list.clear();
        }
        for (i, p) in points.iter().enumerate() {
            let key = self.key(*p);
            self.cells.entry(key).or_default().push(i as u32);
        }
    }

    fn near(&self, p: P, out: &mut Vec<u32>) {
        out.clear();
        let (x, y, z) = self.key(p);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = self.cells.get(&(x + dx, y + dy, z + dz)) {
                        out.extend_from_slice(list);
                    }
                }
            }
        }
    }
}

/// The crowding a drop feels inside a still block of liquid.
fn lattice_density(spacing: f32, reach: f32) -> f32 {
    let mut density = 0.0;
    for dx in -3_i32..=3 {
        for dy in -3_i32..=3 {
            for dz in -3_i32..=3 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let distance = spacing * ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                if distance < reach {
                    let q = 1.0 - distance / reach;
                    density += q * q;
                }
            }
        }
    }
    density
}

/// The crowding a drop feels inside a still sheet of the 2D liquid.
fn lattice_density_2d(spacing: f32, reach: f32) -> f32 {
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

/// The drops of a block between corners `from` and `to`, `spacing` apart,
/// at most [`MAX_DROPS`].
pub fn fill(from: P, to: P, spacing: f32) -> Vec<P> {
    let spacing = spacing.max(1.0);
    let lo = [from[0].min(to[0]), from[1].min(to[1]), from[2].min(to[2])];
    let hi = [from[0].max(to[0]), from[1].max(to[1]), from[2].max(to[2])];
    let count = |a: usize| (((hi[a] - lo[a]) / spacing).floor() as usize + 1).max(1);
    let (nx, ny, nz) = (count(0), count(1), count(2));
    let mut drops = Vec::new();
    'fill: for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                if drops.len() >= MAX_DROPS {
                    break 'fill;
                }
                // A slight offset on every other row keeps the block from
                // standing as a perfect stack.
                let jitter = if (i + j + k) % 2 == 0 { 0.05 } else { -0.05 } * spacing;
                drops.push([
                    lo[0] + i as f32 * spacing + jitter,
                    lo[1] + j as f32 * spacing,
                    lo[2] + k as f32 * spacing - jitter,
                ]);
            }
        }
    }
    drops
}

/// Lets `drops` go with `splash` speed and runs them for `frames` frames
/// of a movie at `fps`. Returns the drops of every frame, the first one
/// first, rounded to whole stage pixels.
#[allow(clippy::too_many_arguments)]
pub fn simulate(
    drops: Vec<P>,
    splash: P,
    settings: Settings3,
    gravity: f32,
    tank: Tank3,
    obstacles: &[Obstacle3],
    fps: f32,
    frames: usize,
) -> Vec<Vec<[i16; 3]>> {
    let spacing = settings.spacing.max(1.0);
    let reach = spacing * REACH;
    let rest = lattice_density(spacing, reach);
    let radius = spacing * 0.5;
    let dt = 1.0 / fps.max(1.0) / SUBSTEPS as f32;
    let stiffness = 1.0;
    let cling = settings.cling.clamp(0.0, 1.0);
    let sigma = (settings.thickness.clamp(0.0, 1.0) * 5.0).min(0.5 / dt);
    let beta = settings.thickness.clamp(0.0, 1.0) * 0.01;
    // A drop in 3D has about twice the neighbours it has in 2D: the push
    // the 2D liquid was tuned with, shared out over them.
    let relax = 0.25
        * spacing
        * (dt * REFERENCE_RATE).powi(2)
        * (lattice_density_2d(spacing, reach) / rest);
    let grip = 1.0 - (1.0 - GRIP).powf(dt * REFERENCE_RATE);

    let mut positions = drops;
    let mut previous = positions.clone();
    let mut velocities = vec![splash; positions.len()];
    let mut grid = Grid::default();
    let mut near = Vec::new();
    let mut neighbours: Vec<(usize, P, f32, f32)> = Vec::new();

    let snapshot = |positions: &[P]| -> Vec<[i16; 3]> {
        positions
            .iter()
            .map(|p| p.map(|v| v.round().clamp(-32_000.0, 32_000.0) as i16))
            .collect()
    };
    let mut out = Vec::with_capacity(frames + 1);
    out.push(snapshot(&positions));
    for _ in 0..frames {
        for _ in 0..SUBSTEPS {
            for v in &mut velocities {
                v[1] += gravity * dt;
            }
            // Thickness: drops closing in on each other slow down together.
            if sigma > 0.0 || beta > 0.0 {
                grid.rebuild(&positions, reach);
                for i in 0..positions.len() {
                    grid.near(positions[i], &mut near);
                    for &j in &near {
                        let j = j as usize;
                        if j <= i {
                            continue;
                        }
                        let d = sub(positions[j], positions[i]);
                        let distance = length(d);
                        if distance <= 1.0e-6 || distance >= reach {
                            continue;
                        }
                        let normal = scale(d, 1.0 / distance);
                        let inward = dot(sub(velocities[i], velocities[j]), normal);
                        if inward <= 0.0 {
                            continue;
                        }
                        let q = distance / reach;
                        let impulse = dt * (1.0 - q) * (sigma * inward + beta * inward * inward);
                        let half = scale(normal, impulse * 0.5);
                        velocities[i] = sub(velocities[i], half);
                        velocities[j] = add(velocities[j], half);
                    }
                }
            }
            for ((p, before), v) in positions.iter_mut().zip(&mut previous).zip(&velocities) {
                *before = *p;
                *p = add(*p, scale(*v, dt));
            }
            // Density relaxation.
            grid.rebuild(&positions, reach);
            for i in 0..positions.len() {
                let pi = positions[i];
                grid.near(pi, &mut near);
                neighbours.clear();
                let mut density = 0.0;
                let mut near_density = 0.0;
                for &j in &near {
                    let j = j as usize;
                    if j == i {
                        continue;
                    }
                    let d = sub(positions[j], pi);
                    let distance = length(d);
                    if distance >= reach {
                        continue;
                    }
                    let q = 1.0 - distance / reach;
                    density += q * q;
                    near_density += q * q * q;
                    neighbours.push((j, d, distance, q));
                }
                let mut pressure = stiffness * (density - rest);
                if pressure < 0.0 {
                    pressure *= cling * 0.5;
                }
                // Half the 2D liquid's: there are more close neighbours to share it.
                let near_pressure = stiffness * near_density;
                let mut moved = [0.0f32; 3];
                for &(j, d, distance, q) in &neighbours {
                    let normal = if distance > 1.0e-6 {
                        scale(d, 1.0 / distance)
                    } else {
                        let a = i as f32 * 2.399_963;
                        [a.sin(), a.cos(), (a * 0.5).sin()]
                    };
                    let push = relax * (pressure * q + near_pressure * q * q);
                    let half = scale(normal, push * 0.5);
                    positions[j] = add(positions[j], half);
                    moved = sub(moved, half);
                }
                positions[i] = add(positions[i], moved);
            }
            // Bodies, the floor and the tank.
            for i in 0..positions.len() {
                for obstacle in obstacles {
                    let Some((normal, depth)) = penetration(obstacle, positions[i], radius) else {
                        continue;
                    };
                    positions[i] = add(positions[i], scale(normal, depth));
                    let relative = scale(sub(positions[i], previous[i]), 1.0 / dt);
                    let into = dot(relative, normal);
                    if into < 0.0 {
                        let slide = sub(relative, scale(normal, into));
                        let change = sub(scale(normal, -into), scale(slide, grip));
                        positions[i] = add(positions[i], scale(change, dt));
                    }
                }
                let p = &mut positions[i];
                let before = &mut previous[i];
                let mut hold = |axis: usize, limit: f32, below: bool| {
                    let outside = if below {
                        p[axis] < limit
                    } else {
                        p[axis] > limit
                    };
                    if outside {
                        p[axis] = limit;
                        before[axis] = limit;
                        for other in (0..3).filter(|a| *a != axis) {
                            let slide = p[other] - before[other];
                            before[other] = p[other] - slide * (1.0 - grip);
                        }
                    }
                };
                if let Some(floor) = tank.floor {
                    hold(1, floor - radius, false);
                }
                if let Some([left, right, front, back]) = tank.walls {
                    hold(0, left + radius, true);
                    hold(0, right - radius, false);
                    hold(2, front + radius, true);
                    hold(2, back - radius, false);
                }
            }
            // Speeds follow from where the drops got to; none jumps more
            // than a spacing and a half in a step.
            let limit = spacing * 1.5;
            for ((p, before), v) in positions.iter_mut().zip(&previous).zip(&mut velocities) {
                let mut moved = sub(*p, *before);
                let distance = length(moved);
                if distance > limit {
                    moved = scale(moved, limit / distance);
                    *p = add(*before, moved);
                }
                *v = scale(moved, 1.0 / dt);
            }
            // Drops that fell off the world are gone.
            let mut i = 0;
            while i < positions.len() {
                if positions[i].iter().any(|v| v.abs() > FAR || !v.is_finite()) {
                    positions.swap_remove(i);
                    previous.swap_remove(i);
                    velocities.swap_remove(i);
                } else {
                    i += 1;
                }
            }
        }
        out.push(snapshot(&positions));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(spacing: f32) -> Settings3 {
        Settings3 {
            spacing,
            thickness: 0.1,
            cling: 0.3,
        }
    }

    #[test]
    fn a_block_of_liquid_spreads_into_a_pool_on_the_floor() {
        let drops = fill([0.0, 0.0, 0.0], [60.0, 60.0, 60.0], 15.0);
        assert_eq!(drops.len(), 125);
        let tank = Tank3 {
            floor: Some(200.0),
            walls: None,
        };
        let frames = simulate(drops, [0.0; 3], water(15.0), 980.0, tank, &[], 24.0, 48);
        let last = frames.last().unwrap();
        assert_eq!(last.len(), 125, "no drop is lost");
        let lowest = last.iter().map(|d| d[1]).max().unwrap();
        let highest = last.iter().map(|d| d[1]).min().unwrap();
        assert!(lowest <= 200, "nothing goes through the floor: {lowest}");
        assert!(highest > 120, "it has fallen and spread low: {highest}");
        let width =
            last.iter().map(|d| d[0]).max().unwrap() - last.iter().map(|d| d[0]).min().unwrap();
        assert!(width > 60, "it spread out: {width}");
    }

    #[test]
    fn liquid_runs_off_a_box_it_lands_on() {
        let drops = fill([-20.0, 0.0, -20.0], [20.0, 30.0, 20.0], 10.0);
        let shelf = Obstacle3::Box {
            centre: [0.0, 150.0, 0.0],
            half: [40.0, 10.0, 40.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let frames = simulate(
            drops,
            [0.0; 3],
            water(10.0),
            980.0,
            Tank3::default(),
            &[shelf],
            24.0,
            24,
        );
        // Right after landing, nothing sits inside the shelf.
        for d in frames.last().unwrap() {
            let inside = d[0].abs() < 38 && (d[1] - 150).abs() < 8 && d[2].abs() < 38;
            assert!(!inside, "{d:?} is inside the shelf");
        }
    }
}

#[cfg(test)]
mod stability {
    use super::*;

    #[test]
    fn a_big_block_does_not_blow_up() {
        let drops = fill([200.0, -80.0, -60.0], [330.0, 20.0, 60.0], 13.0);
        let count = drops.len();
        let top = -80.0;
        let tank = Tank3 {
            floor: Some(480.0),
            walls: None,
        };
        let settings = Settings3 {
            spacing: 13.0,
            thickness: 0.1,
            cling: 0.3,
        };
        let frames = simulate(
            drops,
            [40.0, 0.0, 0.0],
            settings,
            980.0,
            tank,
            &[],
            24.0,
            40,
        );
        for (index, frame) in frames.iter().enumerate() {
            let highest = frame.iter().map(|d| d[1]).min().unwrap_or(0);
            println!("frame {index}: {} drops, highest {highest}", frame.len());
        }
        let last = frames.last().unwrap();
        assert_eq!(last.len(), count);
        let highest = last.iter().map(|d| d[1]).min().unwrap();
        assert!(f32::from(highest) > top, "drops flew up to {highest}");
        let pooled = last.iter().filter(|d| d[1] > 400).count();
        assert!(
            pooled * 2 > count,
            "only {pooled} of {count} reached the floor"
        );
    }
}
