//! Lights authored in the editor.
//!
//! Nothing is baked: every renderer lights the frame itself. What is drawn
//! up to the light layer is multiplied by the light that reaches it, so a
//! lamp in a dark room shows the colours under it instead of washing them
//! white, and layers above the light layer (a HUD, say) stay as drawn.
//!
//! The light at a point is the darkness's shade plus every lamp's colour
//! times its brightness and falloff; [`shade`] is the one formula, and the
//! GPU shaders follow it.

use crate::error::Error;
use crate::io::{write_string_u16, Cursor};
use crate::v2::Rgba;

/// The GPU lights at most this many lamps a timeline.
pub const MAX_LIGHTS: usize = 32;
/// Light can at most double the colour under it.
pub const MAX_GAIN: f32 = 2.0;
const LAYOUT_VERSION: u8 = 1;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightingData {
    pub setups: Vec<LightSetup>,
}

impl LightingData {
    pub fn is_empty(&self) -> bool {
        self.setups.is_empty()
    }

    pub fn setup(&self, q0rg_id: u16) -> Option<&LightSetup> {
        self.setups.iter().find(|setup| setup.q0rg_id == q0rg_id)
    }

    /// The setup of `q0rg_id`, made with defaults when it has none yet.
    pub fn setup_mut(&mut self, q0rg_id: u16) -> &mut LightSetup {
        let index = match self
            .setups
            .iter()
            .position(|setup| setup.q0rg_id == q0rg_id)
        {
            Some(index) => index,
            None => {
                self.setups.push(LightSetup::new(q0rg_id));
                self.setups.len() - 1
            }
        };
        &mut self.setups[index]
    }

    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.setups.retain(|setup| seen.insert(setup.q0rg_id));
        for setup in &mut self.setups {
            setup.sanitize();
        }
    }
}

/// How one timeline is lit.
#[derive(Debug, Clone, PartialEq)]
pub struct LightSetup {
    pub q0rg_id: u16,
    /// Off shows the timeline as drawn, lights and all kept.
    pub enabled: bool,
    /// Layers up to and including this one are lit; the ones above it are
    /// not. `None` lights every layer.
    pub layer_id: Option<u16>,
    /// 0 leaves unlit places as drawn, 1 takes them down to `ambient`.
    pub darkness: f32,
    /// The colour of the dark: deep blue for night, black for a cave.
    pub ambient: Rgba,
    pub lights: Vec<Light>,
}

impl LightSetup {
    pub fn new(q0rg_id: u16) -> Self {
        Self {
            q0rg_id,
            enabled: true,
            layer_id: None,
            darkness: 0.75,
            ambient: Rgba {
                r: 20,
                g: 26,
                b: 52,
                a: 255,
            },
            lights: Vec::new(),
        }
    }

    pub fn light(&self, light_id: u16) -> Option<&Light> {
        self.lights.iter().find(|light| light.light_id == light_id)
    }

    pub fn light_mut(&mut self, light_id: u16) -> Option<&mut Light> {
        self.lights
            .iter_mut()
            .find(|light| light.light_id == light_id)
    }

    /// The lowest light id not used yet.
    pub fn next_light_id(&self) -> u16 {
        (1..=u16::MAX)
            .find(|id| self.light(*id).is_none())
            .unwrap_or(u16::MAX)
    }

    /// How the setup lights `frame` of a movie at `fps`. `anchor` says where
    /// a followed object is; lights following one that is gone stay dark.
    pub fn frame(
        &self,
        frame: u16,
        fps: u16,
        mut anchor: impl FnMut(u32) -> Option<[f32; 2]>,
    ) -> Option<FrameLighting> {
        if !self.enabled {
            return None;
        }
        let shade = mix_rgb([1.0; 3], unit_rgb(self.ambient), self.darkness);
        let time = f32::from(frame) / f32::from(fps.max(1));
        let lights = self
            .lights
            .iter()
            .filter(|light| light.enabled)
            .take(MAX_LIGHTS)
            .filter_map(|light| {
                let origin = match light.follow {
                    Some(instance_id) => anchor(instance_id)?,
                    None => [0.0, 0.0],
                };
                Some(light.lit(frame, time, origin))
            })
            .filter(|lit| lit.radius > 0.0 && lit.color.iter().any(|channel| *channel > 0.0))
            .collect();
        Some(FrameLighting {
            layer_id: self.layer_id,
            shade,
            lights,
        })
    }

    fn sanitize(&mut self) {
        self.darkness = finite_clamp(self.darkness, 0.0, 1.0, 0.75);
        let mut seen = std::collections::HashSet::new();
        self.lights.retain(|light| seen.insert(light.light_id));
        self.lights.truncate(MAX_LIGHTS);
        for light in &mut self.lights {
            light.sanitize();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightKind {
    /// Shines all around.
    #[default]
    Point,
    /// Shines one way, in a cone.
    Spot,
}

/// A lamp.
#[derive(Debug, Clone, PartialEq)]
pub struct Light {
    pub light_id: u16,
    /// How q0lang calls it.
    pub name: String,
    pub kind: LightKind,
    pub enabled: bool,
    /// Stage position, or the offset from the middle of the object it
    /// follows.
    pub position: [f32; 2],
    /// A placed object (instance id on the same timeline) it rides along
    /// with.
    pub follow: Option<u32>,
    pub color: Rgba,
    /// 1 lifts the dark to the light's colour; more brightens past it.
    pub intensity: f32,
    /// Pixels it reaches.
    pub radius: f32,
    /// 0 is a hard edge, 1 fades from the middle out.
    pub softness: f32,
    /// Spot: degrees, 0 to the right and 90 straight down.
    pub direction: f32,
    /// Spot: degrees across the cone.
    pub cone: f32,
    /// Spot: 0 is a hard cone edge, 1 fades across the whole cone.
    pub cone_softness: f32,
    /// A halo added on top around the light itself.
    pub glow: f32,
    /// How much the brightness wavers, 0 to 1.
    pub flicker: f32,
    /// How many times a second it wavers.
    pub flicker_speed: f32,
    /// Animation: values on frames, eased in between. Empty holds still.
    pub keys: Vec<LightKey>,
}

/// A light's animated values on one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightKey {
    pub frame: u16,
    pub position: [f32; 2],
    pub color: Rgba,
    pub intensity: f32,
    pub radius: f32,
    pub direction: f32,
}

impl Light {
    pub fn new(light_id: u16, position: [f32; 2]) -> Self {
        Self {
            light_id,
            name: format!("light{light_id}"),
            kind: LightKind::Point,
            enabled: true,
            position,
            follow: None,
            color: Rgba {
                r: 255,
                g: 214,
                b: 150,
                a: 255,
            },
            intensity: 1.0,
            radius: 180.0,
            softness: 0.8,
            direction: 90.0,
            cone: 50.0,
            cone_softness: 0.4,
            glow: 0.15,
            flicker: 0.0,
            flicker_speed: 8.0,
            keys: Vec::new(),
        }
    }

    /// The key on `frame`, if any.
    pub fn key_at(&self, frame: u16) -> Option<&LightKey> {
        self.keys.iter().find(|key| key.frame == frame)
    }

    /// Keys this light's current values on `frame`, replacing a key there.
    pub fn set_key(&mut self, frame: u16) {
        let key = LightKey {
            frame,
            position: self.position,
            color: self.color,
            intensity: self.intensity,
            radius: self.radius,
            direction: self.direction,
        };
        match self.keys.binary_search_by_key(&frame, |key| key.frame) {
            Ok(index) => self.keys[index] = key,
            Err(index) => self.keys.insert(index, key),
        }
    }

    /// The animated values on `frame`: eased between the keys around it,
    /// held before the first and after the last.
    pub fn animated(&self, frame: u16) -> LightKey {
        let own = LightKey {
            frame,
            position: self.position,
            color: self.color,
            intensity: self.intensity,
            radius: self.radius,
            direction: self.direction,
        };
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return own;
        };
        if frame <= first.frame {
            return *first;
        }
        if frame >= last.frame {
            return *last;
        }
        let next = self.keys.partition_point(|key| key.frame <= frame);
        let (a, b) = (self.keys[next - 1], self.keys[next]);
        let t = f32::from(frame - a.frame) / f32::from((b.frame - a.frame).max(1));
        let t = t * t * (3.0 - 2.0 * t);
        let mix = |x: f32, y: f32| x + (y - x) * t;
        let channel =
            |x: u8, y: u8| mix(f32::from(x), f32::from(y)).round().clamp(0.0, 255.0) as u8;
        LightKey {
            frame,
            position: [
                mix(a.position[0], b.position[0]),
                mix(a.position[1], b.position[1]),
            ],
            color: Rgba {
                r: channel(a.color.r, b.color.r),
                g: channel(a.color.g, b.color.g),
                b: channel(a.color.b, b.color.b),
                a: 255,
            },
            intensity: mix(a.intensity, b.intensity),
            radius: mix(a.radius, b.radius),
            direction: mix(a.direction, b.direction),
        }
    }

    /// How it shines on `frame`, `time` seconds into the movie, around a
    /// followed object at `origin`.
    fn lit(&self, frame: u16, time: f32, origin: [f32; 2]) -> LitLight {
        let values = self.animated(frame);
        let waver = if self.flicker > 0.0 {
            1.0 - self.flicker * flicker_noise(self.light_id, time * self.flicker_speed)
        } else {
            1.0
        };
        let strength = values.intensity * waver;
        let color = unit_rgb(values.color).map(|channel| channel * strength);
        LitLight {
            position: [
                origin[0] + values.position[0],
                origin[1] + values.position[1],
            ],
            color,
            radius: values.radius,
            softness: self.softness,
            spot: (self.kind == LightKind::Spot).then(|| Spot {
                direction: values.direction.to_radians(),
                half_cone: (self.cone * 0.5).to_radians(),
                softness: self.cone_softness,
            }),
            glow: self.glow,
        }
    }

    fn sanitize(&mut self) {
        const FAR: f32 = 100_000.0;
        for value in &mut self.position {
            *value = finite_clamp(*value, -FAR, FAR, 0.0);
        }
        self.intensity = finite_clamp(self.intensity, 0.0, 8.0, 1.0);
        self.radius = finite_clamp(self.radius, 0.0, FAR, 180.0);
        self.softness = finite_clamp(self.softness, 0.0, 1.0, 0.8);
        self.direction = finite_clamp(self.direction, -36_000.0, 36_000.0, 90.0);
        self.cone = finite_clamp(self.cone, 1.0, 360.0, 50.0);
        self.cone_softness = finite_clamp(self.cone_softness, 0.0, 1.0, 0.4);
        self.glow = finite_clamp(self.glow, 0.0, 4.0, 0.15);
        self.flicker = finite_clamp(self.flicker, 0.0, 1.0, 0.0);
        self.flicker_speed = finite_clamp(self.flicker_speed, 0.0, 60.0, 8.0);
        if self.name.chars().count() > 64 {
            self.name = self.name.chars().take(64).collect();
        }
        self.keys.sort_by_key(|key| key.frame);
        self.keys.dedup_by_key(|key| key.frame);
        for key in &mut self.keys {
            for value in &mut key.position {
                *value = finite_clamp(*value, -FAR, FAR, 0.0);
            }
            key.intensity = finite_clamp(key.intensity, 0.0, 8.0, 1.0);
            key.radius = finite_clamp(key.radius, 0.0, FAR, 180.0);
            key.direction = finite_clamp(key.direction, -36_000.0, 36_000.0, 90.0);
        }
    }
}

/// A smooth wobble between 0 and 1, the same for a light everywhere.
fn flicker_noise(light_id: u16, t: f32) -> f32 {
    let hash = |step: f32| {
        let mut x = (step as i64 as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(u64::from(light_id).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        x ^= x >> 31;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 29;
        (x >> 40) as f32 / (1u64 << 24) as f32
    };
    let step = t.floor();
    let f = t - step;
    let f = f * f * (3.0 - 2.0 * f);
    hash(step) + (hash(step + 1.0) - hash(step)) * f
}

/// A spot light's cone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    /// Radians.
    pub direction: f32,
    /// Radians either side of the direction.
    pub half_cone: f32,
    pub softness: f32,
}

/// One light as it shines on one frame, in the timeline's own space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LitLight {
    pub position: [f32; 2],
    /// Colour times brightness, 0..=1 per unit of brightness.
    pub color: [f32; 3],
    pub radius: f32,
    pub softness: f32,
    pub spot: Option<Spot>,
    pub glow: f32,
}

impl LitLight {
    /// How much of the light reaches `point`, 0 to 1.
    pub fn reach(&self, point: [f32; 2]) -> f32 {
        let dx = point[0] - self.position[0];
        let dy = point[1] - self.position[1];
        let distance = dx.hypot(dy);
        if distance >= self.radius {
            return 0.0;
        }
        let soft = self.softness.max(0.02);
        let falloff = 1.0 - smoothstep(self.radius * (1.0 - soft), self.radius, distance);
        match self.spot {
            None => falloff,
            Some(spot) => {
                let angle = dy.atan2(dx);
                let off = wrap_angle(angle - spot.direction).abs();
                let soft = spot.softness.max(0.02);
                let cone = 1.0 - smoothstep(spot.half_cone * (1.0 - soft), spot.half_cone, off);
                falloff * cone
            }
        }
    }

    /// How strong its halo is at `point`, 0 to 1.
    pub fn halo(&self, point: [f32; 2]) -> f32 {
        if self.glow <= 0.0 {
            return 0.0;
        }
        let reach = self.radius * 0.45;
        let distance = (point[0] - self.position[0]).hypot(point[1] - self.position[1]);
        if distance >= reach {
            return 0.0;
        }
        let t = 1.0 - distance / reach;
        self.glow * t * t
    }
}

/// How one frame is lit.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLighting {
    /// Layers up to and including this one are lit.
    pub layer_id: Option<u16>,
    /// What unlit places are multiplied by.
    pub shade: [f32; 3],
    pub lights: Vec<LitLight>,
}

/// What the light does at `point`: the multiplier for the colour under it
/// and the light added on top of it, per channel and 0..=1.
pub fn shade(lighting: &FrameLighting, point: [f32; 2]) -> ([f32; 3], [f32; 3]) {
    let mut gain = lighting.shade;
    let mut added = [0.0; 3];
    for light in &lighting.lights {
        let reach = light.reach(point);
        let halo = light.halo(point);
        for channel in 0..3 {
            gain[channel] += light.color[channel] * reach;
            added[channel] += light.color[channel] * halo;
        }
    }
    (
        gain.map(|channel| channel.clamp(0.0, MAX_GAIN)),
        added.map(|channel| channel.clamp(0.0, 1.0)),
    )
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wrap_angle(angle: f32) -> f32 {
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
    wrapped - std::f32::consts::PI
}

fn unit_rgb(color: Rgba) -> [f32; 3] {
    [color.r, color.g, color.b].map(|channel| f32::from(channel) / 255.0)
}

fn mix_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub(crate) fn write(out: &mut Vec<u8>, data: &LightingData) -> Result<(), Error> {
    out.push(LAYOUT_VERSION);
    let count = u16::try_from(data.setups.len())
        .map_err(|_| Error::Overflow("lighting setup count exceeds u16"))?;
    out.extend_from_slice(&count.to_le_bytes());
    let f32s = |out: &mut Vec<u8>, values: &[f32]| {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    };
    let rgba = |out: &mut Vec<u8>, color: Rgba| {
        out.extend_from_slice(&[color.r, color.g, color.b, color.a])
    };
    for setup in &data.setups {
        out.extend_from_slice(&setup.q0rg_id.to_le_bytes());
        out.push(u8::from(setup.enabled));
        match setup.layer_id {
            None => out.push(0),
            Some(layer_id) => {
                out.push(1);
                out.extend_from_slice(&layer_id.to_le_bytes());
            }
        }
        f32s(out, &[setup.darkness]);
        rgba(out, setup.ambient);
        let lights = u16::try_from(setup.lights.len())
            .map_err(|_| Error::Overflow("light count exceeds u16"))?;
        out.extend_from_slice(&lights.to_le_bytes());
        for light in &setup.lights {
            out.extend_from_slice(&light.light_id.to_le_bytes());
            write_string_u16(out, &light.name)?;
            out.push(match light.kind {
                LightKind::Point => 0,
                LightKind::Spot => 1,
            });
            out.push(u8::from(light.enabled));
            f32s(out, &light.position);
            match light.follow {
                None => out.push(0),
                Some(instance_id) => {
                    out.push(1);
                    out.extend_from_slice(&instance_id.to_le_bytes());
                }
            }
            rgba(out, light.color);
            f32s(
                out,
                &[
                    light.intensity,
                    light.radius,
                    light.softness,
                    light.direction,
                    light.cone,
                    light.cone_softness,
                    light.glow,
                    light.flicker,
                    light.flicker_speed,
                ],
            );
            let keys = u16::try_from(light.keys.len())
                .map_err(|_| Error::Overflow("light key count exceeds u16"))?;
            out.extend_from_slice(&keys.to_le_bytes());
            for key in &light.keys {
                out.extend_from_slice(&key.frame.to_le_bytes());
                f32s(out, &key.position);
                rgba(out, key.color);
                f32s(out, &[key.intensity, key.radius, key.direction]);
            }
        }
    }
    Ok(())
}

pub(crate) fn read(c: &mut Cursor<'_>) -> Result<LightingData, Error> {
    if c.read_u8()? != LAYOUT_VERSION {
        return Err(Error::Validation("unknown lighting layout"));
    }
    let rgba = |c: &mut Cursor<'_>| -> Result<Rgba, Error> {
        Ok(Rgba {
            r: c.read_u8()?,
            g: c.read_u8()?,
            b: c.read_u8()?,
            a: c.read_u8()?,
        })
    };
    let mut data = LightingData::default();
    for _ in 0..c.read_u16()? {
        let mut setup = LightSetup::new(c.read_u16()?);
        setup.enabled = c.read_u8()? != 0;
        setup.layer_id = match c.read_u8()? {
            0 => None,
            _ => Some(c.read_u16()?),
        };
        setup.darkness = c.read_f32()?;
        setup.ambient = rgba(c)?;
        for _ in 0..c.read_u16()? {
            let mut light = Light::new(c.read_u16()?, [0.0, 0.0]);
            light.name = c.read_string_u16()?;
            light.kind = match c.read_u8()? {
                0 => LightKind::Point,
                1 => LightKind::Spot,
                _ => return Err(Error::Validation("unknown light kind")),
            };
            light.enabled = c.read_u8()? != 0;
            light.position = [c.read_f32()?, c.read_f32()?];
            light.follow = match c.read_u8()? {
                0 => None,
                _ => Some(c.read_u32()?),
            };
            light.color = rgba(c)?;
            light.intensity = c.read_f32()?;
            light.radius = c.read_f32()?;
            light.softness = c.read_f32()?;
            light.direction = c.read_f32()?;
            light.cone = c.read_f32()?;
            light.cone_softness = c.read_f32()?;
            light.glow = c.read_f32()?;
            light.flicker = c.read_f32()?;
            light.flicker_speed = c.read_f32()?;
            for _ in 0..c.read_u16()? {
                light.keys.push(LightKey {
                    frame: c.read_u16()?,
                    position: [c.read_f32()?, c.read_f32()?],
                    color: rgba(c)?,
                    intensity: c.read_f32()?,
                    radius: c.read_f32()?,
                    direction: c.read_f32()?,
                });
            }
            setup.lights.push(light);
        }
        data.setups.push(setup);
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

    fn lamp(position: [f32; 2]) -> Light {
        Light {
            softness: 0.5,
            glow: 0.0,
            ..Light::new(1, position)
        }
    }

    fn lit(setup: &LightSetup) -> FrameLighting {
        setup.frame(0, 24, |_| None).unwrap()
    }

    #[test]
    fn darkness_shades_unlit_places_and_a_lamp_lifts_them() {
        let mut setup = LightSetup::new(1);
        setup.darkness = 1.0;
        setup.ambient = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let mut light = lamp([100.0, 100.0]);
        light.color = Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        setup.lights.push(light);
        let lighting = lit(&setup);
        let (far, _) = shade(&lighting, [400.0, 400.0]);
        assert_eq!(far, [0.0; 3], "pitch dark away from the lamp");
        let (near, _) = shade(&lighting, [100.0, 100.0]);
        assert_eq!(near, [1.0; 3], "under the lamp things show as drawn");
        let (edge, _) = shade(&lighting, [100.0 + 170.0, 100.0]);
        assert!(edge[0] > 0.0 && edge[0] < 0.5, "fades out: {edge:?}");
    }

    #[test]
    fn coloured_light_tints_without_whitening() {
        let mut setup = LightSetup::new(1);
        setup.darkness = 0.0;
        let mut light = lamp([0.0, 0.0]);
        light.color = Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        setup.lights.push(light);
        let (gain, added) = shade(&lit(&setup), [0.0, 0.0]);
        // Daylight plus a red lamp: red doubles, green and blue stay.
        assert_eq!(gain, [2.0, 1.0, 1.0]);
        assert_eq!(added, [0.0; 3]);
    }

    #[test]
    fn a_spot_only_lights_its_cone() {
        let mut setup = LightSetup::new(1);
        setup.darkness = 1.0;
        let mut light = lamp([0.0, 0.0]);
        light.kind = LightKind::Spot;
        light.direction = 0.0;
        light.cone = 40.0;
        light.cone_softness = 0.1;
        setup.lights.push(light);
        let lighting = lit(&setup);
        let ahead = shade(&lighting, [50.0, 0.0]).0[0];
        let aside = shade(&lighting, [0.0, 50.0]).0[0];
        let behind = shade(&lighting, [-50.0, 0.0]).0[0];
        assert!(ahead > 0.5, "{ahead}");
        assert!(aside < 0.2 && behind < 0.2, "{aside} {behind}");
    }

    #[test]
    fn keys_animate_and_followers_ride_along() {
        let mut light = lamp([0.0, 0.0]);
        light.set_key(0);
        light.position = [100.0, 0.0];
        light.intensity = 0.0;
        light.set_key(10);
        let halfway = light.animated(5);
        assert!((halfway.position[0] - 50.0).abs() < 0.01);
        assert!((halfway.intensity - 0.5).abs() < 0.01);
        assert_eq!(light.animated(40).position, [100.0, 0.0]);

        let mut setup = LightSetup::new(1);
        let mut follower = lamp([0.0, -10.0]);
        follower.follow = Some(7);
        setup.lights.push(follower);
        let lighting = setup
            .frame(0, 24, |id| (id == 7).then_some([30.0, 40.0]))
            .unwrap();
        assert_eq!(lighting.lights[0].position, [30.0, 30.0]);
        let gone = setup.frame(0, 24, |_| None).unwrap();
        assert!(gone.lights.is_empty(), "no object, no light");
    }

    #[test]
    fn flicker_wavers_the_same_way_every_time() {
        let mut setup = LightSetup::new(1);
        let mut light = lamp([0.0, 0.0]);
        light.flicker = 0.8;
        setup.lights.push(light);
        let a = (0..48)
            .map(|frame| setup.frame(frame, 24, |_| None).unwrap().lights[0].color[0])
            .collect::<Vec<_>>();
        let b = (0..48)
            .map(|frame| setup.frame(frame, 24, |_| None).unwrap().lights[0].color[0])
            .collect::<Vec<_>>();
        assert_eq!(a, b);
        let low = a.iter().copied().fold(f32::MAX, f32::min);
        let high = a.iter().copied().fold(f32::MIN, f32::max);
        assert!(high - low > 0.2, "it wavers: {low}..{high}");
    }

    #[test]
    fn every_setting_survives_a_round_trip() {
        let mut data = LightingData::default();
        let setup = data.setup_mut(3);
        setup.layer_id = Some(9);
        setup.darkness = 0.4;
        let mut light = Light::new(2, [5.0, 6.0]);
        light.kind = LightKind::Spot;
        light.follow = Some(11);
        light.flicker = 0.3;
        light.set_key(4);
        setup.lights.push(light);
        let mut bytes = Vec::new();
        write(&mut bytes, &data).unwrap();
        let mut cursor = Cursor::new(&bytes);
        assert_eq!(read(&mut cursor).unwrap(), data);
        assert_eq!(cursor.remaining(), 0);
    }
}
