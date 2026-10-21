//! Particle emitters: what the editor bakes into keyframes and q0player runs
//! live. A seeded generator makes the same emitter give the same particles
//! everywhere.
//!
//! The model follows the classic cocos2d particle system that Geometry
//! Dash's particle editor is built on: a gravity mode where particles fly
//! and get pulled around, a radius mode where they circle the emitter, and
//! start and end values for size, spin and colour with a variance each.

use q0s_format::particles::{
    EmitterShape, ParticleEmitter, ParticleLook, ParticleMotion, ParticleSpace,
};
use q0s_format::raster::RuntimeBoundsCache;
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::v2::{ProjectV2, Rgba};

use crate::rng::Rng;

/// At most this many steps run ahead for a quick start.
const QUICK_START_STEPS: u32 = 600;
/// Streaks never grow longer than this many times their width.
const MAX_STRETCH: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// From the point it moves around: where the emitter was when it was
    /// born for free particles, the emitter itself for relative ones.
    pub offset: [f32; 2],
    /// Stage pixels per second.
    pub velocity: [f32; 2],
    /// Seconds since it left the emitter.
    pub age: f32,
    /// Seconds it lives.
    pub life: f32,
    /// Where the emitter was when it was born.
    origin: [f32; 2],
    /// Radius mode: the centre it circles, from `origin`.
    hub: [f32; 2],
    size: [f32; 2],
    /// Radians at birth and at death.
    spin: [f32; 2],
    /// 0..=255 per channel at birth and at death.
    color: [[f32; 4]; 2],
    /// Seconds to fade in and to fade out.
    fade: [f32; 2],
    radial: f32,
    tangential: f32,
    drag: f32,
    /// Radius mode: where on the circle it is, how fast it goes round
    /// (radians a second) and the radius at birth and at death.
    angle: f32,
    orbit: f32,
    radius: [f32; 2],
}

impl Particle {
    /// 0 when it is born, 1 when it dies.
    pub fn progress(&self) -> f32 {
        (self.age / self.life.max(1.0e-4)).clamp(0.0, 1.0)
    }
}

/// How one particle looks right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleStyle {
    /// Pixels across for drawn shapes; a scale factor for library items.
    pub size: f32,
    pub color: Rgba,
    /// Radians, clockwise.
    pub rotation: f32,
    /// Drawn this many times longer along its rotation than across.
    pub stretch: f32,
    /// Adds its light to what is behind instead of covering it.
    pub additive: bool,
}

/// A particle as one frame draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleDraw {
    pub position: [f32; 2],
    pub style: ParticleStyle,
}

/// Where an emitter following object `instance_id` sits on `frame`: the
/// middle of the object as drawn, or `None` while it is not on the stage.
pub fn follow_point(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    instance_id: u32,
    scene: &RuntimeSceneState,
    cache: Option<&RuntimeBoundsCache>,
) -> Option<[f32; 2]> {
    let metrics = match cache {
        Some(cache) => q0s_format::raster::runtime_instance_metrics_with_cache(
            project,
            q0rg_id,
            frame,
            instance_id,
            scene,
            cache,
        ),
        None => q0s_format::raster::runtime_instance_metrics(
            project,
            q0rg_id,
            frame,
            instance_id,
            scene,
        ),
    }?;
    Some([
        (metrics.left + metrics.right) * 0.5,
        (metrics.top + metrics.bottom) * 0.5,
    ])
}

pub struct ParticleSystem {
    emitter: ParticleEmitter,
    stage_size: [f32; 2],
    particles: Vec<Particle>,
    rng: Rng,
    carry: f32,
    emitting: bool,
    /// Seconds since it started emitting.
    elapsed: f32,
    /// Where the object it follows is; the emitter's position is an offset
    /// from it.
    anchor: Option<[f32; 2]>,
}

impl ParticleSystem {
    /// An emitter that starts emitting: run ahead first for a quick start,
    /// then its burst.
    pub fn new(emitter: &ParticleEmitter, stage_size: [f32; 2]) -> Self {
        Self::new_at(emitter, stage_size, None)
    }

    /// Like [`ParticleSystem::new`], following an object that starts at
    /// `anchor`.
    pub fn new_at(
        emitter: &ParticleEmitter,
        stage_size: [f32; 2],
        anchor: Option<[f32; 2]>,
    ) -> Self {
        let mut system = Self {
            emitter: emitter.clone(),
            stage_size,
            particles: Vec::new(),
            rng: Rng::new(u64::from(emitter.seed)),
            carry: 0.0,
            emitting: true,
            elapsed: 0.0,
            anchor: anchor.filter(|_| emitter.follow.is_some()),
        };
        if emitter.quick_start {
            let ahead = (emitter.lifetime.value + emitter.lifetime.variance).max(0.0);
            let steps = ((ahead * 30.0).ceil() as u32).clamp(1, QUICK_START_STEPS);
            let dt = ahead / steps as f32;
            for _ in 0..steps {
                system.advance(dt, false);
            }
            system.elapsed = 0.0;
        }
        system.burst(usize::from(emitter.burst));
        system
    }

    pub fn emitter(&self) -> &ParticleEmitter {
        &self.emitter
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn is_emitting(&self) -> bool {
        self.emitting && !self.emission_over()
    }

    pub fn set_emitting(&mut self, emitting: bool) {
        self.emitting = emitting;
        if emitting {
            self.elapsed = 0.0;
        } else {
            self.carry = 0.0;
        }
    }

    /// Moves the emitter to a stage position; it stops following anything.
    pub fn set_position(&mut self, position: [f32; 2]) {
        if position.iter().all(|value| value.is_finite()) {
            self.emitter.position = position;
            self.emitter.follow = None;
            self.anchor = None;
        }
    }

    /// Where the object the emitter follows is now.
    pub fn set_anchor(&mut self, anchor: [f32; 2]) {
        if self.emitter.follow.is_some() && anchor.iter().all(|value| value.is_finite()) {
            self.anchor = Some(anchor);
        }
    }

    /// The emitter's centre on the stage.
    pub fn centre(&self) -> [f32; 2] {
        let [x, y] = self.emitter.position;
        match self.anchor {
            Some([ax, ay]) => [ax + x, ay + y],
            None => [x, y],
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.carry = 0.0;
    }

    /// Releases `count` particles at once.
    pub fn burst(&mut self, count: usize) {
        let room = usize::from(self.emitter.max_particles).saturating_sub(self.particles.len());
        for _ in 0..count.min(room) {
            let particle = self.spawn();
            self.particles.push(particle);
        }
    }

    /// Where `particle` is on the stage.
    pub fn position_of(&self, particle: &Particle) -> [f32; 2] {
        let [x, y] = match self.emitter.space {
            ParticleSpace::Free => particle.origin,
            ParticleSpace::Relative => self.centre(),
        };
        [x + particle.offset[0], y + particle.offset[1]]
    }

    /// Advances `dt` seconds: ages and moves every particle, retires the
    /// spent ones and emits new ones at the emitter's rate.
    pub fn step(&mut self, dt: f32) {
        self.advance(dt, true);
    }

    fn emission_over(&self) -> bool {
        self.emitter.emit_seconds > 0.0 && self.elapsed > self.emitter.emit_seconds
    }

    fn advance(&mut self, dt: f32, timed: bool) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let gravity = self.emitter.gravity;
        let bounce = self.emitter.edge_bounce;
        let motion = self.emitter.motion;
        let reference_now = self.centre();
        let space = self.emitter.space;
        let [width, height] = self.stage_size;
        for particle in &mut self.particles {
            particle.age += dt;
            match motion {
                ParticleMotion::Gravity => {
                    let [ox, oy] = particle.offset;
                    let distance = ox.hypot(oy);
                    let (rx, ry) = if distance > 1.0e-4 {
                        (ox / distance, oy / distance)
                    } else {
                        (0.0, 0.0)
                    };
                    // Clockwise on a stage whose y runs down.
                    let ax = gravity[0] + rx * particle.radial - ry * particle.tangential;
                    let ay = gravity[1] + ry * particle.radial + rx * particle.tangential;
                    let slowing = (-particle.drag * dt).exp();
                    particle.velocity[0] = (particle.velocity[0] + ax * dt) * slowing;
                    particle.velocity[1] = (particle.velocity[1] + ay * dt) * slowing;
                    particle.offset[0] += particle.velocity[0] * dt;
                    particle.offset[1] += particle.velocity[1] * dt;
                    if bounce > 0.0 {
                        let reference = match space {
                            ParticleSpace::Free => particle.origin,
                            ParticleSpace::Relative => reference_now,
                        };
                        bounce_inside(particle, reference, width, height, bounce);
                    }
                }
                ParticleMotion::Radius => {
                    particle.angle += particle.orbit * dt;
                    let t = particle.progress();
                    let radius = particle.radius[0] + (particle.radius[1] - particle.radius[0]) * t;
                    let next = [
                        particle.hub[0] + particle.angle.cos() * radius,
                        particle.hub[1] + particle.angle.sin() * radius,
                    ];
                    particle.velocity = [
                        (next[0] - particle.offset[0]) / dt,
                        (next[1] - particle.offset[1]) / dt,
                    ];
                    particle.offset = next;
                }
            }
        }
        self.particles
            .retain(|particle| particle.age < particle.life);
        if timed {
            self.elapsed += dt;
        }
        if self.emitting && !(timed && self.emission_over()) {
            self.carry += self.emitter.rate * dt;
            let whole = self.carry.floor();
            self.carry -= whole;
            let room = usize::from(self.emitter.max_particles).saturating_sub(self.particles.len());
            for index in 0..(whole as usize).min(room) {
                let mut particle = self.spawn();
                // Spread the newcomers over the step instead of bunching
                // them on its start.
                let head_start = dt * (index as f32 + 0.5) / whole.max(1.0);
                particle.age = head_start;
                if motion == ParticleMotion::Gravity {
                    particle.offset[0] += particle.velocity[0] * head_start;
                    particle.offset[1] += particle.velocity[1] * head_start;
                } else {
                    particle.angle += particle.orbit * head_start;
                }
                self.particles.push(particle);
            }
        }
    }

    fn spawn(&mut self) -> Particle {
        let centre = self.centre();
        let emitter = &self.emitter;
        let rng = &mut self.rng;
        let local = match emitter.shape {
            EmitterShape::Point => [0.0, 0.0],
            EmitterShape::Line => [rng.signed() * emitter.size[0] * 0.5, 0.0],
            EmitterShape::Box => [
                rng.signed() * emitter.size[0] * 0.5,
                rng.signed() * emitter.size[1] * 0.5,
            ],
            EmitterShape::Circle => {
                let angle = rng.unit() * std::f32::consts::TAU;
                let distance = rng.unit().sqrt() * emitter.size[0];
                [angle.cos() * distance, angle.sin() * distance]
            }
            EmitterShape::Ring => {
                let angle = rng.unit() * std::f32::consts::TAU;
                [angle.cos() * emitter.size[0], angle.sin() * emitter.size[0]]
            }
        };
        let (sin, cos) = emitter.angle.to_radians().sin_cos();
        let place = [
            cos * local[0] - sin * local[1],
            sin * local[0] + cos * local[1],
        ];
        let heading = emitter.direction.at(rng.signed()).to_radians();
        let speed = emitter.speed.at(rng.signed());
        let life = emitter.lifetime.at(rng.signed()).max(0.02);
        let size_start = emitter.size_start.at(rng.signed()).max(0.0);
        let size_end = emitter.size_end.at(rng.signed()).max(0.0);
        let spin_start = emitter.spin_start.at(rng.signed());
        let spin_end = emitter.spin_end.at(rng.signed());
        let color_start = varied_color(
            emitter.color_start,
            emitter.color_start_variance,
            emitter.color_sync,
            rng,
        );
        let color_end = varied_color(
            emitter.color_end,
            emitter.color_end_variance,
            emitter.color_sync,
            rng,
        );
        let fade = [
            emitter.fade_in.at(rng.signed()).max(0.0),
            emitter.fade_out.at(rng.signed()).max(0.0),
        ];
        let radial = emitter.radial_accel.at(rng.signed());
        let tangential = emitter.tangential_accel.at(rng.signed());
        let drag = emitter.drag.at(rng.signed()).max(0.0);
        let orbit = emitter.orbit.at(rng.signed()).to_radians();
        let radius = [
            emitter.start_radius.at(rng.signed()).max(0.0),
            emitter.end_radius.at(rng.signed()).max(0.0),
        ];
        let (offset, velocity) = match emitter.motion {
            ParticleMotion::Gravity => (place, [heading.cos() * speed, heading.sin() * speed]),
            ParticleMotion::Radius => (
                [
                    place[0] + heading.cos() * radius[0],
                    place[1] + heading.sin() * radius[0],
                ],
                [0.0, 0.0],
            ),
        };
        Particle {
            offset,
            velocity,
            age: 0.0,
            life,
            origin: centre,
            hub: place,
            size: [
                size_start,
                if emitter.size_end_same {
                    size_start
                } else {
                    size_end
                },
            ],
            spin: [
                spin_start.to_radians(),
                if emitter.spin_end_same {
                    spin_start
                } else {
                    spin_end
                }
                .to_radians(),
            ],
            color: [color_start, color_end],
            fade,
            radial,
            tangential,
            drag,
            angle: heading,
            orbit,
            // Equal start and end radii keep each particle on one circle
            // instead of drawing a second random radius.
            radius: if emitter.start_radius == emitter.end_radius {
                [radius[0], radius[0]]
            } else {
                radius
            },
        }
    }

    /// How `particle` looks now: size, spin and colour run from their start
    /// values to their end values over the particle's life, and it fades in
    /// and out over its first and last moments.
    pub fn style(&self, particle: &Particle) -> ParticleStyle {
        let t = particle.progress();
        let emitter = &self.emitter;
        let lerp = |pair: [f32; 2]| pair[0] + (pair[1] - pair[0]) * t;
        let size = lerp(particle.size);
        let channel = |index: usize| {
            lerp([particle.color[0][index], particle.color[1][index]])
                .round()
                .clamp(0.0, 255.0)
        };
        let mut alpha = channel(3) / 255.0;
        if particle.fade[0] > 1.0e-4 {
            alpha *= (particle.age / particle.fade[0]).clamp(0.0, 1.0);
        }
        if particle.fade[1] > 1.0e-4 {
            alpha *= ((particle.life - particle.age) / particle.fade[1]).clamp(0.0, 1.0);
        }
        let speed = particle.velocity[0].hypot(particle.velocity[1]);
        let spin = lerp(particle.spin);
        let facing = emitter.face_motion || matches!(emitter.look, ParticleLook::Spark);
        let rotation = if facing && speed > 1.0e-3 {
            particle.velocity[1].atan2(particle.velocity[0]) + spin
        } else {
            spin
        };
        ParticleStyle {
            size: size.max(0.0),
            color: Rgba {
                r: channel(0) as u8,
                g: channel(1) as u8,
                b: channel(2) as u8,
                a: (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
            },
            rotation,
            stretch: (1.0 + speed * emitter.stretch / 100.0).min(MAX_STRETCH),
            additive: emitter.additive,
        }
    }

    /// Every particle as it is drawn now, back to front.
    pub fn draws(&self) -> Vec<ParticleDraw> {
        let draw = |particle: &Particle| ParticleDraw {
            position: self.position_of(particle),
            style: self.style(particle),
        };
        if self.emitter.newest_on_top {
            self.particles.iter().map(draw).collect()
        } else {
            self.particles.iter().rev().map(draw).collect()
        }
    }
}

fn varied_color(base: Rgba, variance: Rgba, sync: bool, rng: &mut Rng) -> [f32; 4] {
    let shared = rng.signed();
    let mut channel = |value: u8, spread: u8, synced: bool| {
        let amount = if synced { shared } else { rng.signed() };
        (f32::from(value) + f32::from(spread) * amount).clamp(0.0, 255.0)
    };
    [
        channel(base.r, variance.r, sync),
        channel(base.g, variance.g, sync),
        channel(base.b, variance.b, sync),
        channel(base.a, variance.a, false),
    ]
}

fn bounce_inside(
    particle: &mut Particle,
    reference: [f32; 2],
    width: f32,
    height: f32,
    bounce: f32,
) {
    for (axis, limit) in [width, height].into_iter().enumerate() {
        let position = reference[axis] + particle.offset[axis];
        if position < 0.0 {
            particle.offset[axis] = -position * bounce - reference[axis];
            particle.velocity[axis] = particle.velocity[axis].abs() * bounce;
        } else if position > limit {
            particle.offset[axis] = limit - (position - limit) * bounce - reference[axis];
            particle.velocity[axis] = -particle.velocity[axis].abs() * bounce;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::particles::Varied;

    fn emitter() -> ParticleEmitter {
        ParticleEmitter::new(1, 1, [100.0, 100.0])
    }

    /// No randomness, no forces: particles go straight at one speed.
    fn plain() -> ParticleEmitter {
        let mut settings = emitter();
        settings.direction = Varied::fixed(0.0);
        settings.speed = Varied::fixed(20.0);
        settings.gravity = [0.0, 0.0];
        settings.drag = Varied::fixed(0.0);
        settings.rate = 0.0;
        settings.burst = 1;
        settings.lifetime = Varied::fixed(2.0);
        settings.size_start = Varied::fixed(10.0);
        settings.size_end = Varied::fixed(2.0);
        settings.color_start_variance = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        settings.fade_in = Varied::fixed(0.0);
        settings.fade_out = Varied::fixed(0.0);
        settings
    }

    #[test]
    fn the_same_seed_gives_the_same_particles() {
        let mut a = ParticleSystem::new(&emitter(), [400.0, 300.0]);
        let mut b = ParticleSystem::new(&emitter(), [400.0, 300.0]);
        for _ in 0..30 {
            a.step(1.0 / 24.0);
            b.step(1.0 / 24.0);
        }
        assert!(!a.particles().is_empty());
        assert_eq!(a.particles(), b.particles());
        assert_eq!(a.draws(), b.draws());
    }

    #[test]
    fn rate_lifetime_and_cap_shape_the_crowd() {
        let mut settings = emitter();
        settings.rate = 48.0;
        settings.lifetime = Varied::fixed(0.5);
        settings.max_particles = 1000;
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        for _ in 0..48 {
            system.step(1.0 / 24.0);
        }
        // Half a second of life at 48 a second keeps about 24 alive.
        let alive = system.particles().len();
        assert!((20..=28).contains(&alive), "{alive} alive");
        settings.max_particles = 5;
        let mut capped = ParticleSystem::new(&settings, [400.0, 300.0]);
        for _ in 0..48 {
            capped.step(1.0 / 24.0);
        }
        assert!(capped.particles().len() <= 5);
    }

    #[test]
    fn particles_fly_their_direction_and_change_size_and_colour() {
        let mut system = ParticleSystem::new(&plain(), [400.0, 300.0]);
        system.step(1.0);
        let particle = system.particles()[0];
        let [x, y] = system.position_of(&particle);
        assert!(
            (x - 120.0).abs() < 0.01 && (y - 100.0).abs() < 0.01,
            "{x} {y}"
        );
        let style = system.style(&particle);
        assert!((style.size - 6.0).abs() < 0.01, "halfway between 10 and 2");
        assert!(
            style.color.a < 255 && style.color.a > 0,
            "fading to the end colour"
        );
        system.step(2.0);
        assert!(system.particles().is_empty(), "dies after its life");
    }

    #[test]
    fn fades_soften_the_first_and_last_moments() {
        let mut settings = plain();
        settings.color_end = settings.color_start;
        settings.fade_in = Varied::fixed(0.5);
        settings.fade_out = Varied::fixed(0.5);
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        system.step(0.25);
        let early = system.style(&system.particles()[0]).color.a;
        system.step(0.75);
        let middle = system.style(&system.particles()[0]).color.a;
        system.step(0.75);
        let late = system.style(&system.particles()[0]).color.a;
        assert!((120..=135).contains(&early), "{early}");
        assert_eq!(middle, 255);
        assert!((120..=135).contains(&late), "{late}");
    }

    #[test]
    fn radius_mode_circles_the_emitter() {
        let mut settings = plain();
        settings.motion = ParticleMotion::Radius;
        settings.start_radius = Varied::fixed(50.0);
        settings.end_radius = Varied::fixed(50.0);
        settings.orbit = Varied::fixed(90.0);
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        let born = system.position_of(&system.particles()[0]);
        assert!((born[0] - 150.0).abs() < 0.01 && (born[1] - 100.0).abs() < 0.01);
        system.step(1.0);
        // A quarter turn clockwise on a y-down stage ends straight below.
        let [x, y] = system.position_of(&system.particles()[0]);
        assert!(
            (x - 100.0).abs() < 0.05 && (y - 150.0).abs() < 0.05,
            "{x} {y}"
        );
    }

    #[test]
    fn tangential_pull_curls_and_radial_pull_draws_in() {
        let mut settings = plain();
        settings.tangential_accel = Varied::fixed(400.0);
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        system.step(0.05);
        system.step(0.5);
        let [_, y] = system.position_of(&system.particles()[0]);
        assert!(y > 101.0, "curled clockwise, down the stage: {y}");

        let mut settings = plain();
        settings.speed = Varied::fixed(0.0);
        settings.burst = 0;
        settings.shape = EmitterShape::Ring;
        settings.size = [80.0, 0.0];
        settings.radial_accel = Varied::fixed(-200.0);
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        system.burst(1);
        let before = system.particles()[0].offset;
        system.step(0.3);
        let after = system.particles()[0].offset;
        assert!(after[0].hypot(after[1]) < before[0].hypot(before[1]));
    }

    #[test]
    fn relative_particles_travel_with_a_followed_object() {
        let mut settings = plain();
        settings.follow = Some(5);
        settings.position = [0.0, -10.0];
        settings.speed = Varied::fixed(0.0);
        settings.space = ParticleSpace::Relative;
        let mut system = ParticleSystem::new_at(&settings, [400.0, 300.0], Some([50.0, 50.0]));
        assert_eq!(system.position_of(&system.particles()[0]), [50.0, 40.0]);
        system.set_anchor([200.0, 50.0]);
        system.step(0.1);
        assert_eq!(system.position_of(&system.particles()[0]), [200.0, 40.0]);

        settings.space = ParticleSpace::Free;
        let mut system = ParticleSystem::new_at(&settings, [400.0, 300.0], Some([50.0, 50.0]));
        system.set_anchor([200.0, 50.0]);
        system.step(0.1);
        assert_eq!(system.position_of(&system.particles()[0]), [50.0, 40.0]);
    }

    #[test]
    fn quick_start_begins_full_and_emit_seconds_stop_it() {
        let mut settings = emitter();
        settings.rate = 40.0;
        settings.lifetime = Varied::fixed(1.0);
        settings.quick_start = true;
        let system = ParticleSystem::new(&settings, [400.0, 300.0]);
        assert!(
            system.particles().len() >= 30,
            "{}",
            system.particles().len()
        );

        settings.quick_start = false;
        settings.emit_seconds = 0.5;
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        for _ in 0..12 {
            system.step(1.0 / 24.0);
        }
        let made = system.particles().len();
        assert!(system.is_emitting());
        for _ in 0..12 {
            system.step(1.0 / 24.0);
        }
        assert!(!system.is_emitting());
        assert!(system.particles().len() <= made + 1);
    }

    #[test]
    fn stopping_holds_new_particles_and_bursts_add_them() {
        let mut system = ParticleSystem::new(&emitter(), [400.0, 300.0]);
        system.set_emitting(false);
        system.step(1.0);
        assert!(system.particles().is_empty());
        system.burst(12);
        assert_eq!(system.particles().len(), 12);
    }

    #[test]
    fn draw_order_follows_the_setting() {
        let mut settings = plain();
        settings.burst = 0;
        settings.rate = 10.0;
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        system.step(0.5);
        let newest_last = system.draws();
        settings.newest_on_top = false;
        let mut system = ParticleSystem::new(&settings, [400.0, 300.0]);
        system.step(0.5);
        let mut oldest_last = system.draws();
        oldest_last.reverse();
        assert!(newest_last.len() > 2);
        assert_eq!(newest_last, oldest_last);
    }
}
