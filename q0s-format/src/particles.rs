//! Particle emitters authored in the editor, built like Geometry Dash's
//! particle editor: nearly every number has a ± variance, and size, spin
//! and colour run from a start value to an end value over each particle's
//! life.
//!
//! Like physics, a simulation is baked into ordinary keyframes on a layer
//! of its own. Emitters marked `live` also travel to q0player, where q0lang
//! starts, stops and bursts them at runtime (`q0particles.*!`).

use crate::error::Error;
use crate::io::{write_string_u16, Cursor};
use crate::v2::{Rgba, Target};

/// Opens a particle block written with a layout version; the first
/// development layout started straight with the emitter count.
const LAYOUT_MARK: u16 = u16::MAX;
const LAYOUT_VERSION: u8 = 2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticleData {
    pub emitters: Vec<ParticleEmitter>,
}

impl ParticleData {
    pub fn is_empty(&self) -> bool {
        self.emitters.is_empty()
    }

    pub fn emitter(&self, q0rg_id: u16, emitter_id: u16) -> Option<&ParticleEmitter> {
        self.emitters
            .iter()
            .find(|emitter| emitter.q0rg_id == q0rg_id && emitter.emitter_id == emitter_id)
    }

    pub fn emitter_mut(&mut self, q0rg_id: u16, emitter_id: u16) -> Option<&mut ParticleEmitter> {
        self.emitters
            .iter_mut()
            .find(|emitter| emitter.q0rg_id == q0rg_id && emitter.emitter_id == emitter_id)
    }

    /// The lowest emitter id not used in `q0rg_id` yet.
    pub fn next_emitter_id(&self, q0rg_id: u16) -> u16 {
        (1..=u16::MAX)
            .find(|id| self.emitter(q0rg_id, *id).is_none())
            .unwrap_or(u16::MAX)
    }

    /// Keeps only the emitters q0player runs live.
    pub fn retain_live(&mut self) {
        self.emitters.retain(|emitter| emitter.live);
    }

    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.emitters
            .retain(|emitter| seen.insert((emitter.q0rg_id, emitter.emitter_id)));
        for emitter in &mut self.emitters {
            emitter.sanitize();
        }
    }
}

/// A value, and how far either side of it each particle may land.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Varied {
    pub value: f32,
    pub variance: f32,
}

impl Varied {
    pub const fn new(value: f32, variance: f32) -> Self {
        Self { value, variance }
    }

    pub const fn fixed(value: f32) -> Self {
        Self::new(value, 0.0)
    }

    /// The value for a particle, given `signed` in -1..=1.
    pub fn at(self, signed: f32) -> f32 {
        self.value + self.variance * signed
    }

    fn sanitize(&mut self, min: f32, max: f32, fallback: f32) {
        self.value = finite_clamp(self.value, min, max, fallback);
        self.variance = finite_clamp(self.variance, 0.0, max - min, 0.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmitterShape {
    /// Everything leaves one point.
    #[default]
    Point,
    /// Along a line `size[0]` long.
    Line,
    /// Anywhere in a `size[0]` by `size[1]` box.
    Box,
    /// Anywhere in a circle of radius `size[0]`.
    Circle,
    /// On the edge of a circle of radius `size[0]`.
    Ring,
}

impl EmitterShape {
    pub const ALL: [Self; 5] = [Self::Point, Self::Line, Self::Box, Self::Circle, Self::Ring];

    const fn code(self) -> u8 {
        match self {
            Self::Point => 0,
            Self::Line => 1,
            Self::Box => 2,
            Self::Circle => 3,
            Self::Ring => 4,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Point),
            1 => Some(Self::Line),
            2 => Some(Self::Box),
            3 => Some(Self::Circle),
            4 => Some(Self::Ring),
            _ => None,
        }
    }
}

/// What particles do once they are out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleMotion {
    /// They fly off and gravity, pulls and drag steer them.
    #[default]
    Gravity,
    /// They circle the emitter, from a start radius to an end radius.
    Radius,
}

/// Whether particles stay where they were let go or travel with the
/// emitter when it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleSpace {
    /// Left behind where they were born, like smoke from a moving torch.
    #[default]
    Free,
    /// Carried along with the emitter, like a shield around a player.
    Relative,
}

/// What one particle looks like.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ParticleLook {
    #[default]
    Circle,
    /// A soft dot that fades to nothing at its edge.
    Glow,
    Square,
    Star,
    /// A streak stretched along the way it flies.
    Spark,
    /// A thin circle outline.
    Ring,
    /// A drawing or symbol from the library.
    Item(Target),
}

impl ParticleLook {
    /// The looks drawn from built-in white shapes, tinted per particle.
    pub const BUILT_IN: [Self; 6] = [
        Self::Circle,
        Self::Glow,
        Self::Square,
        Self::Star,
        Self::Spark,
        Self::Ring,
    ];

    fn write(self, out: &mut Vec<u8>) {
        match self {
            Self::Circle => out.push(0),
            Self::Square => out.push(1),
            Self::Star => out.push(2),
            Self::Spark => out.push(3),
            Self::Item(Target::Asset(id)) => {
                out.push(4);
                out.extend_from_slice(&id.to_le_bytes());
            }
            Self::Item(Target::Q0rg(id)) => {
                out.push(5);
                out.extend_from_slice(&id.to_le_bytes());
            }
            Self::Glow => out.push(6),
            Self::Ring => out.push(7),
        }
    }

    fn read(c: &mut Cursor<'_>) -> Result<Self, Error> {
        Ok(match c.read_u8()? {
            0 => Self::Circle,
            1 => Self::Square,
            2 => Self::Star,
            3 => Self::Spark,
            4 => Self::Item(Target::Asset(c.read_u16()?)),
            5 => Self::Item(Target::Q0rg(c.read_u16()?)),
            6 => Self::Glow,
            7 => Self::Ring,
            _ => return Err(Error::Validation("unknown particle look")),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParticleEmitter {
    pub q0rg_id: u16,
    pub emitter_id: u16,
    /// How q0lang calls it.
    pub name: String,
    /// It emits from this frame...
    pub start_frame: u16,
    /// ...and the bake ends on this one.
    pub end_frame: u16,

    // Where particles come from.
    pub shape: EmitterShape,
    /// Stage position of the emitter's centre, or its offset from the
    /// object it follows.
    pub position: [f32; 2],
    /// Line length, box width and height, or circle radius in `size[0]`.
    pub size: [f32; 2],
    /// Degrees the shape is turned.
    pub angle: f32,
    /// A placed object (instance id on the same timeline) the emitter
    /// rides along with.
    pub follow: Option<u32>,
    pub space: ParticleSpace,

    // How many.
    /// Particles a second.
    pub rate: f32,
    /// Particles all at once when it starts.
    pub burst: u16,
    pub max_particles: u16,
    /// Seconds it keeps emitting; 0 keeps it going to the end.
    pub emit_seconds: f32,
    /// Starts already full, as if it had been running for a while.
    pub quick_start: bool,
    /// Seconds a particle lives.
    pub lifetime: Varied,

    // How they move.
    pub motion: ParticleMotion,
    /// Degrees, 0 to the right and 90 straight down; the variance is the
    /// spread either side.
    pub direction: Varied,
    /// Pixels per second it leaves at.
    pub speed: Varied,
    /// Pixels per second squared.
    pub gravity: [f32; 2],
    /// Pull away from the emitter (negative pulls in), px/s².
    pub radial_accel: Varied,
    /// Push around the emitter, clockwise, px/s².
    pub tangential_accel: Varied,
    /// How fast the air slows it, per second.
    pub drag: Varied,
    /// Radius mode: distance from the emitter when born...
    pub start_radius: Varied,
    /// ...and when it dies.
    pub end_radius: Varied,
    /// Radius mode: degrees a second it circles, clockwise.
    pub orbit: Varied,
    /// Turns to face the way it flies.
    pub face_motion: bool,

    // How they look.
    pub look: ParticleLook,
    /// Adds light instead of covering what is behind: glows and fire.
    pub additive: bool,
    /// Pixels across for drawn shapes, a scale for library items.
    pub size_start: Varied,
    pub size_end: Varied,
    /// Each particle ends as big as it started.
    pub size_end_same: bool,
    /// Degrees turned when born...
    pub spin_start: Varied,
    /// ...and when it dies.
    pub spin_end: Varied,
    pub spin_end_same: bool,
    pub color_start: Rgba,
    /// How far each channel may stray, either way.
    pub color_start_variance: Rgba,
    pub color_end: Rgba,
    pub color_end_variance: Rgba,
    /// One random amount for red, green and blue, so the variance makes a
    /// particle lighter or darker instead of another hue.
    pub color_sync: bool,
    /// Seconds it takes to fade in after birth.
    pub fade_in: Varied,
    /// Seconds it takes to fade out before death.
    pub fade_out: Varied,
    /// Stretches particles along their flight, more the faster they go.
    pub stretch: f32,
    /// New particles cover old ones; off puts them underneath.
    pub newest_on_top: bool,
    /// Particles bounce off the stage edges this much; 0 lets them leave.
    pub edge_bounce: f32,
    /// The same seed makes the same particles every time.
    pub seed: u32,
    /// q0player can run it live, driven by q0lang.
    pub live: bool,
    /// The layer the last bake drew into, reused by the next bake.
    pub layer_id: Option<u16>,
}

impl ParticleEmitter {
    pub fn new(q0rg_id: u16, emitter_id: u16, position: [f32; 2]) -> Self {
        Self {
            q0rg_id,
            emitter_id,
            name: format!("emitter{emitter_id}"),
            start_frame: 0,
            end_frame: 47,
            shape: EmitterShape::Point,
            position,
            size: [80.0, 40.0],
            angle: 0.0,
            follow: None,
            space: ParticleSpace::Free,
            rate: 60.0,
            burst: 0,
            max_particles: 400,
            emit_seconds: 0.0,
            quick_start: false,
            lifetime: Varied::new(1.0, 0.3),
            motion: ParticleMotion::Gravity,
            direction: Varied::new(270.0, 30.0),
            speed: Varied::new(160.0, 50.0),
            gravity: [0.0, -60.0],
            radial_accel: Varied::fixed(0.0),
            tangential_accel: Varied::fixed(0.0),
            drag: Varied::fixed(0.4),
            start_radius: Varied::new(100.0, 0.0),
            end_radius: Varied::new(0.0, 0.0),
            orbit: Varied::new(180.0, 30.0),
            face_motion: false,
            look: ParticleLook::Glow,
            additive: true,
            size_start: Varied::new(26.0, 8.0),
            size_end: Varied::new(6.0, 2.0),
            size_end_same: false,
            spin_start: Varied::fixed(0.0),
            spin_end: Varied::fixed(0.0),
            spin_end_same: false,
            color_start: Rgba {
                r: 255,
                g: 190,
                b: 70,
                a: 255,
            },
            color_start_variance: Rgba {
                r: 0,
                g: 40,
                b: 30,
                a: 0,
            },
            color_end: Rgba {
                r: 230,
                g: 50,
                b: 30,
                a: 0,
            },
            color_end_variance: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            color_sync: false,
            fade_in: Varied::fixed(0.05),
            fade_out: Varied::fixed(0.2),
            stretch: 0.0,
            newest_on_top: true,
            edge_bounce: 0.0,
            seed: u32::from(emitter_id).wrapping_mul(2_654_435_761),
            live: false,
            layer_id: None,
        }
    }

    pub fn sanitize(&mut self) {
        const FAR: f32 = 100_000.0;
        if self.end_frame < self.start_frame {
            std::mem::swap(&mut self.start_frame, &mut self.end_frame);
        }
        for value in self.position.iter_mut().chain(&mut self.gravity) {
            *value = finite_clamp(*value, -FAR, FAR, 0.0);
        }
        for value in &mut self.size {
            *value = finite_clamp(*value, 0.0, FAR, 0.0);
        }
        self.angle = finite_clamp(self.angle, -36_000.0, 36_000.0, 0.0);
        self.rate = finite_clamp(self.rate, 0.0, 5_000.0, 40.0);
        self.max_particles = self.max_particles.clamp(1, 20_000);
        self.emit_seconds = finite_clamp(self.emit_seconds, 0.0, 3_600.0, 0.0);
        self.lifetime.sanitize(0.02, 600.0, 1.0);
        self.direction.sanitize(-36_000.0, 36_000.0, 270.0);
        self.direction.variance = self.direction.variance.min(180.0);
        self.speed.sanitize(-FAR, FAR, 0.0);
        self.radial_accel.sanitize(-FAR, FAR, 0.0);
        self.tangential_accel.sanitize(-FAR, FAR, 0.0);
        self.drag.sanitize(0.0, 100.0, 0.0);
        self.start_radius.sanitize(0.0, FAR, 100.0);
        self.end_radius.sanitize(0.0, FAR, 0.0);
        self.orbit.sanitize(-FAR, FAR, 180.0);
        self.size_start.sanitize(0.0, 10_000.0, 10.0);
        self.size_end.sanitize(0.0, 10_000.0, 2.0);
        self.spin_start.sanitize(-FAR, FAR, 0.0);
        self.spin_end.sanitize(-FAR, FAR, 0.0);
        self.fade_in.sanitize(0.0, 600.0, 0.0);
        self.fade_out.sanitize(0.0, 600.0, 0.0);
        self.stretch = finite_clamp(self.stretch, 0.0, 20.0, 0.0);
        self.edge_bounce = finite_clamp(self.edge_bounce, 0.0, 1.0, 0.0);
        if self.name.chars().count() > 64 {
            self.name = self.name.chars().take(64).collect();
        }
    }
}

pub(crate) fn write(out: &mut Vec<u8>, data: &ParticleData) -> Result<(), Error> {
    let count = u16::try_from(data.emitters.len())
        .ok()
        .filter(|count| *count != LAYOUT_MARK)
        .ok_or(Error::Overflow("particle emitter count exceeds u16"))?;
    out.extend_from_slice(&LAYOUT_MARK.to_le_bytes());
    out.push(LAYOUT_VERSION);
    out.extend_from_slice(&count.to_le_bytes());
    for emitter in &data.emitters {
        write_emitter(out, emitter)?;
    }
    Ok(())
}

fn write_emitter(out: &mut Vec<u8>, emitter: &ParticleEmitter) -> Result<(), Error> {
    let f32s = |out: &mut Vec<u8>, values: &[f32]| {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    };
    let varied = |out: &mut Vec<u8>, values: &[Varied]| {
        for value in values {
            out.extend_from_slice(&value.value.to_le_bytes());
            out.extend_from_slice(&value.variance.to_le_bytes());
        }
    };
    out.extend_from_slice(&emitter.q0rg_id.to_le_bytes());
    out.extend_from_slice(&emitter.emitter_id.to_le_bytes());
    write_string_u16(out, &emitter.name)?;
    out.extend_from_slice(&emitter.start_frame.to_le_bytes());
    out.extend_from_slice(&emitter.end_frame.to_le_bytes());

    out.push(emitter.shape.code());
    f32s(
        out,
        &[
            emitter.position[0],
            emitter.position[1],
            emitter.size[0],
            emitter.size[1],
            emitter.angle,
        ],
    );
    match emitter.follow {
        None => out.push(0),
        Some(instance_id) => {
            out.push(1);
            out.extend_from_slice(&instance_id.to_le_bytes());
        }
    }
    out.push(match emitter.space {
        ParticleSpace::Free => 0,
        ParticleSpace::Relative => 1,
    });

    f32s(out, &[emitter.rate]);
    out.extend_from_slice(&emitter.burst.to_le_bytes());
    out.extend_from_slice(&emitter.max_particles.to_le_bytes());
    f32s(out, &[emitter.emit_seconds]);
    out.push(u8::from(emitter.quick_start));
    varied(out, &[emitter.lifetime]);

    out.push(match emitter.motion {
        ParticleMotion::Gravity => 0,
        ParticleMotion::Radius => 1,
    });
    varied(out, &[emitter.direction, emitter.speed]);
    f32s(out, &emitter.gravity);
    varied(
        out,
        &[
            emitter.radial_accel,
            emitter.tangential_accel,
            emitter.drag,
            emitter.start_radius,
            emitter.end_radius,
            emitter.orbit,
        ],
    );
    out.push(u8::from(emitter.face_motion));

    emitter.look.write(out);
    out.push(u8::from(emitter.additive));
    varied(out, &[emitter.size_start, emitter.size_end]);
    out.push(u8::from(emitter.size_end_same));
    varied(out, &[emitter.spin_start, emitter.spin_end]);
    out.push(u8::from(emitter.spin_end_same));
    for color in [
        emitter.color_start,
        emitter.color_start_variance,
        emitter.color_end,
        emitter.color_end_variance,
    ] {
        out.extend_from_slice(&[color.r, color.g, color.b, color.a]);
    }
    out.push(u8::from(emitter.color_sync));
    varied(out, &[emitter.fade_in, emitter.fade_out]);
    f32s(out, &[emitter.stretch]);
    out.push(u8::from(emitter.newest_on_top));
    f32s(out, &[emitter.edge_bounce]);
    out.extend_from_slice(&emitter.seed.to_le_bytes());
    out.push(u8::from(emitter.live));
    match emitter.layer_id {
        None => out.push(0),
        Some(layer_id) => {
            out.push(1);
            out.extend_from_slice(&layer_id.to_le_bytes());
        }
    }
    Ok(())
}

pub(crate) fn read(c: &mut Cursor<'_>) -> Result<ParticleData, Error> {
    let mut data = ParticleData::default();
    let first = c.read_u16()?;
    if first == LAYOUT_MARK {
        if c.read_u8()? != LAYOUT_VERSION {
            return Err(Error::Validation("unknown particle layout"));
        }
        for _ in 0..c.read_u16()? {
            data.emitters.push(read_emitter(c)?);
        }
    } else {
        for _ in 0..first {
            data.emitters.push(read_first_layout(c)?);
        }
    }
    data.sanitize();
    Ok(data)
}

fn read_emitter(c: &mut Cursor<'_>) -> Result<ParticleEmitter, Error> {
    let varied = |c: &mut Cursor<'_>| -> Result<Varied, Error> {
        Ok(Varied::new(c.read_f32()?, c.read_f32()?))
    };
    let flag = |c: &mut Cursor<'_>| -> Result<bool, Error> { Ok(c.read_u8()? != 0) };
    let q0rg_id = c.read_u16()?;
    let emitter_id = c.read_u16()?;
    let mut emitter = ParticleEmitter::new(q0rg_id, emitter_id, [0.0, 0.0]);
    emitter.name = c.read_string_u16()?;
    emitter.start_frame = c.read_u16()?;
    emitter.end_frame = c.read_u16()?;

    emitter.shape = EmitterShape::from_code(c.read_u8()?)
        .ok_or(Error::Validation("unknown particle emitter shape"))?;
    emitter.position = [c.read_f32()?, c.read_f32()?];
    emitter.size = [c.read_f32()?, c.read_f32()?];
    emitter.angle = c.read_f32()?;
    emitter.follow = match c.read_u8()? {
        0 => None,
        _ => Some(c.read_u32()?),
    };
    emitter.space = match c.read_u8()? {
        0 => ParticleSpace::Free,
        1 => ParticleSpace::Relative,
        _ => return Err(Error::Validation("unknown particle space")),
    };

    emitter.rate = c.read_f32()?;
    emitter.burst = c.read_u16()?;
    emitter.max_particles = c.read_u16()?;
    emitter.emit_seconds = c.read_f32()?;
    emitter.quick_start = flag(c)?;
    emitter.lifetime = varied(c)?;

    emitter.motion = match c.read_u8()? {
        0 => ParticleMotion::Gravity,
        1 => ParticleMotion::Radius,
        _ => return Err(Error::Validation("unknown particle motion")),
    };
    emitter.direction = varied(c)?;
    emitter.speed = varied(c)?;
    emitter.gravity = [c.read_f32()?, c.read_f32()?];
    emitter.radial_accel = varied(c)?;
    emitter.tangential_accel = varied(c)?;
    emitter.drag = varied(c)?;
    emitter.start_radius = varied(c)?;
    emitter.end_radius = varied(c)?;
    emitter.orbit = varied(c)?;
    emitter.face_motion = flag(c)?;

    emitter.look = ParticleLook::read(c)?;
    emitter.additive = flag(c)?;
    emitter.size_start = varied(c)?;
    emitter.size_end = varied(c)?;
    emitter.size_end_same = flag(c)?;
    emitter.spin_start = varied(c)?;
    emitter.spin_end = varied(c)?;
    emitter.spin_end_same = flag(c)?;
    emitter.color_start = read_rgba(c)?;
    emitter.color_start_variance = read_rgba(c)?;
    emitter.color_end = read_rgba(c)?;
    emitter.color_end_variance = read_rgba(c)?;
    emitter.color_sync = flag(c)?;
    emitter.fade_in = varied(c)?;
    emitter.fade_out = varied(c)?;
    emitter.stretch = c.read_f32()?;
    emitter.newest_on_top = flag(c)?;
    emitter.edge_bounce = c.read_f32()?;
    emitter.seed = c.read_u32()?;
    emitter.live = flag(c)?;
    emitter.layer_id = match c.read_u8()? {
        0 => None,
        _ => Some(c.read_u16()?),
    };
    Ok(emitter)
}

/// The first development layout: fewer settings, variances as fractions of
/// their value, and spin in degrees per second.
fn read_first_layout(c: &mut Cursor<'_>) -> Result<ParticleEmitter, Error> {
    let q0rg_id = c.read_u16()?;
    let emitter_id = c.read_u16()?;
    let mut emitter = ParticleEmitter::new(q0rg_id, emitter_id, [0.0, 0.0]);
    emitter.name = c.read_string_u16()?;
    emitter.start_frame = c.read_u16()?;
    emitter.end_frame = c.read_u16()?;
    emitter.shape = EmitterShape::from_code(c.read_u8()?)
        .filter(|shape| *shape != EmitterShape::Ring)
        .ok_or(Error::Validation("unknown particle emitter shape"))?;
    emitter.position = [c.read_f32()?, c.read_f32()?];
    emitter.size = [c.read_f32()?, c.read_f32()?];
    emitter.angle = c.read_f32()?;
    emitter.rate = c.read_f32()?;
    emitter.burst = c.read_u16()?;
    let lifetime = c.read_f32()?;
    let lifetime_variance = c.read_f32()?;
    emitter.lifetime = Varied::new(lifetime, lifetime * lifetime_variance);
    let speed = c.read_f32()?;
    let speed_variance = c.read_f32()?;
    emitter.speed = Varied::new(speed, speed.abs() * speed_variance);
    let direction = c.read_f32()?;
    let spread = c.read_f32()?;
    emitter.direction = Varied::new(direction, spread);
    emitter.gravity = [c.read_f32()?, c.read_f32()?];
    emitter.drag = Varied::fixed(c.read_f32()?);
    emitter.size_start = Varied::fixed(c.read_f32()?);
    emitter.size_end = Varied::fixed(c.read_f32()?);
    emitter.color_start = read_rgba(c)?;
    emitter.color_end = read_rgba(c)?;
    emitter.color_start_variance = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    let spin = c.read_f32()?;
    let spin_variance = c.read_f32()?;
    let turn = spin * lifetime;
    // Spinning particles used to start at a random angle.
    let start = Varied::new(0.0, if spin == 0.0 { 0.0 } else { 180.0 });
    emitter.spin_start = start;
    emitter.spin_end = Varied::new(turn, turn.abs() * spin_variance);
    emitter.look = match c.read_u8()? {
        0 => ParticleLook::Circle,
        1 => ParticleLook::Square,
        2 => ParticleLook::Star,
        3 => ParticleLook::Spark,
        4 => ParticleLook::Item(Target::Asset(c.read_u16()?)),
        5 => ParticleLook::Item(Target::Q0rg(c.read_u16()?)),
        _ => return Err(Error::Validation("unknown particle look")),
    };
    if emitter.look == ParticleLook::Spark {
        emitter.face_motion = true;
        emitter.stretch = 0.8;
    }
    emitter.additive = false;
    emitter.fade_in = Varied::fixed(0.0);
    emitter.fade_out = Varied::fixed(0.0);
    emitter.edge_bounce = c.read_f32()?;
    emitter.max_particles = c.read_u16()?;
    emitter.seed = c.read_u32()?;
    emitter.live = c.read_u8()? != 0;
    emitter.layer_id = match c.read_u8()? {
        0 => None,
        _ => Some(c.read_u16()?),
    };
    Ok(emitter)
}

fn read_rgba(c: &mut Cursor<'_>) -> Result<Rgba, Error> {
    Ok(Rgba {
        r: c.read_u8()?,
        g: c.read_u8()?,
        b: c.read_u8()?,
        a: c.read_u8()?,
    })
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
    fn every_setting_survives_a_round_trip() {
        let mut emitter = ParticleEmitter::new(3, 7, [12.0, 34.0]);
        emitter.shape = EmitterShape::Ring;
        emitter.follow = Some(42);
        emitter.space = ParticleSpace::Relative;
        emitter.motion = ParticleMotion::Radius;
        emitter.quick_start = true;
        emitter.emit_seconds = 1.5;
        emitter.look = ParticleLook::Item(Target::Q0rg(9));
        emitter.tangential_accel = Varied::new(-40.0, 12.0);
        emitter.spin_end = Varied::new(720.0, 90.0);
        emitter.spin_end_same = true;
        emitter.color_sync = true;
        emitter.newest_on_top = false;
        emitter.layer_id = Some(4);
        let data = ParticleData {
            emitters: vec![emitter],
        };
        let mut bytes = Vec::new();
        write(&mut bytes, &data).unwrap();
        let mut cursor = Cursor::new(&bytes);
        assert_eq!(read(&mut cursor).unwrap(), data);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn the_first_layout_still_loads() {
        // One emitter written the way the first particle build wrote it.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        write_string_u16(&mut bytes, "torch").unwrap();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&47u16.to_le_bytes());
        bytes.push(1);
        let f32s = |bytes: &mut Vec<u8>, values: &[f32]| {
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        };
        f32s(&mut bytes, &[100.0, 200.0, 40.0, 0.0, 0.0, 60.0]);
        bytes.extend_from_slice(&5u16.to_le_bytes());
        f32s(
            &mut bytes,
            &[
                2.0, 0.5, 100.0, 0.2, 270.0, 25.0, 0.0, 300.0, 0.2, 10.0, 2.0,
            ],
        );
        bytes.extend_from_slice(&[255, 200, 60, 255, 230, 60, 30, 0]);
        f32s(&mut bytes, &[90.0, 0.0]);
        bytes.push(3);
        f32s(&mut bytes, &[0.5]);
        bytes.extend_from_slice(&400u16.to_le_bytes());
        bytes.extend_from_slice(&77u32.to_le_bytes());
        bytes.push(1);
        bytes.push(1);
        bytes.extend_from_slice(&6u16.to_le_bytes());

        let mut cursor = Cursor::new(&bytes);
        let data = read(&mut cursor).unwrap();
        assert_eq!(cursor.remaining(), 0);
        let emitter = &data.emitters[0];
        assert_eq!(emitter.name, "torch");
        assert_eq!(emitter.shape, EmitterShape::Line);
        assert_eq!(emitter.lifetime, Varied::new(2.0, 1.0));
        assert_eq!(emitter.speed, Varied::new(100.0, 20.0));
        assert_eq!(emitter.direction, Varied::new(270.0, 25.0));
        assert_eq!(emitter.spin_end.value, 180.0);
        assert_eq!(emitter.look, ParticleLook::Spark);
        assert!(emitter.face_motion);
        assert_eq!(emitter.edge_bounce, 0.5);
        assert_eq!(emitter.seed, 77);
        assert!(emitter.live);
        assert_eq!(emitter.layer_id, Some(6));
    }
}
