//! How 3D lives on a timeline: where each placed object sits in depth, the
//! camera that looks at the stage, the 3D lights, and bone poses keyed on
//! the timeline.
//!
//! The stage camera looks straight at the stage from in front of it, so
//! anything at depth 0 that is not turned shows exactly as it would in 2D.

use super::math::{Mat4, Quat, V3};
use super::model::Trs;
use crate::transform::Affine;
use crate::v2::{Rgba, Vec2};

/// No clip: the model holds its rest pose.
pub const NO_CLIP: u16 = u16::MAX;

/// A placed object's place in 3D. Kept on every keyframe and eased by
/// tweens like the rest of the placement.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Space3d {
    /// Depth: more is further away.
    pub z: f32,
    /// Degrees about the object's own horizontal axis (leaning back).
    pub tilt_x: f32,
    /// Degrees about the object's own vertical axis (turning).
    pub tilt_y: f32,
    /// The model's animation clip, or [`NO_CLIP`].
    pub clip: u16,
    /// Seconds into the clip on the placement's first frame.
    pub clip_time: f32,
    /// 1 plays the clip at its own speed, 0 holds `clip_time`.
    pub clip_speed: f32,
    pub clip_loop: bool,
    /// Lit by the 3D lights.
    pub lit: bool,
}

impl Default for Space3d {
    fn default() -> Self {
        Self {
            z: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            clip: NO_CLIP,
            clip_time: 0.0,
            clip_speed: 1.0,
            clip_loop: true,
            lit: true,
        }
    }
}

impl Space3d {
    pub fn lerp_to(self, target: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        let same_clip = self.clip == target.clip;
        Self {
            z: mix(self.z, target.z),
            tilt_x: mix(self.tilt_x, target.tilt_x),
            tilt_y: mix(self.tilt_y, target.tilt_y),
            clip: if t >= 1.0 { target.clip } else { self.clip },
            // Easing a clip's time scrubs through it; across different
            // clips the time stays with the clip that is showing.
            clip_time: if same_clip {
                mix(self.clip_time, target.clip_time)
            } else if t >= 1.0 {
                target.clip_time
            } else {
                self.clip_time
            },
            clip_speed: if same_clip {
                mix(self.clip_speed, target.clip_speed)
            } else if t >= 1.0 {
                target.clip_speed
            } else {
                self.clip_speed
            },
            clip_loop: if t >= 1.0 {
                target.clip_loop
            } else {
                self.clip_loop
            },
            lit: if t >= 1.0 { target.lit } else { self.lit },
        }
    }

    pub fn sanitize(&mut self) {
        for value in [
            &mut self.z,
            &mut self.tilt_x,
            &mut self.tilt_y,
            &mut self.clip_time,
        ] {
            if !value.is_finite() {
                *value = 0.0;
            }
        }
        if !self.clip_speed.is_finite() {
            self.clip_speed = 1.0;
        }
    }

    /// Seconds into the clip, `elapsed_frames` after the placement began.
    pub fn clip_seconds(&self, elapsed_frames: f32, fps: u16, duration: f32) -> f32 {
        let time = self.clip_time + elapsed_frames / f32::from(fps.max(1)) * self.clip_speed;
        if duration <= 0.0 {
            return 0.0;
        }
        if self.clip_loop {
            time.rem_euclid(duration)
        } else {
            time.clamp(0.0, duration)
        }
    }

    pub fn tilt(&self) -> Quat {
        Quat::from_axis_angle(V3::X, self.tilt_x.to_radians())
            * Quat::from_axis_angle(V3::Y, self.tilt_y.to_radians())
    }
}

/// A 2D matrix as a 3D one; depth scales by the 2D size so things keep
/// their proportions.
pub fn lift(a: Affine) -> Mat4 {
    let depth = (a.a11 * a.a22 - a.a12 * a.a21).abs().sqrt();
    Mat4::from_rows([
        [a.a11, a.a12, 0.0, a.tx],
        [a.a21, a.a22, 0.0, a.ty],
        [0.0, 0.0, depth, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

/// Where a placed object's own coordinates land in stage space: its 2D
/// transform, turned in 3D about its pivot and pushed back by its depth,
/// inside `parent`.
pub fn placement_matrix(parent: &Mat4, local: Affine, space: Option<Space3d>, pivot: Vec2) -> Mat4 {
    let Some(space) = space else {
        return *parent * lift(local);
    };
    let pivot = V3::new(pivot.x, pivot.y, 0.0);
    *parent
        * Mat4::translation(V3::new(0.0, 0.0, space.z))
        * lift(local)
        * Mat4::translation(pivot)
        * Mat4::from_quat(space.tilt())
        * Mat4::translation(-pivot)
}

/// Model units (y up) into an object's stage coordinates (y down).
pub fn model_to_stage(scale: f32) -> Mat4 {
    Mat4::scale(V3::new(scale, -scale, -scale))
}

/// The camera's own frame: what it looks at and how far it zooms.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CameraPose {
    pub position: V3,
    pub target: V3,
    /// Degrees about the view direction.
    pub roll: f32,
    /// Pixels from the camera at which things show at their own size.
    pub zoom: f32,
}

impl CameraPose {
    /// Looking straight at the stage from the distance at which depth 0
    /// shows one to one.
    pub fn stage(width: u16, height: u16) -> Self {
        let (w, h) = (f32::from(width.max(1)), f32::from(height.max(1)));
        // The usual 50 mm lens: 39.6 degrees across.
        let zoom = (w * 0.5) / (39.6f32.to_radians() * 0.5).tan();
        Self {
            position: V3::new(w * 0.5, h * 0.5, -zoom),
            target: V3::new(w * 0.5, h * 0.5, 0.0),
            roll: 0.0,
            zoom,
        }
    }

    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            position: self.position.lerp(other.position, t),
            target: self.target.lerp(other.target, t),
            roll: mix(self.roll, other.roll),
            zoom: mix(self.zoom, other.zoom),
        }
    }

    /// Stage space to camera space: x right, y down, z ahead.
    pub fn view_matrix(&self) -> Mat4 {
        let mut forward = (self.target - self.position).normalized();
        if forward == V3::ZERO {
            forward = V3::Z;
        }
        let down = V3::Y;
        let mut right = down.cross(forward).normalized();
        if right == V3::ZERO {
            right = V3::X;
        }
        let mut down = forward.cross(right).normalized();
        if self.roll != 0.0 {
            let turn = Quat::from_axis_angle(forward, self.roll.to_radians());
            right = turn.rotate(right);
            down = turn.rotate(down);
        }
        let eye = self.position;
        Mat4::from_rows([
            [right.x, right.y, right.z, -right.dot(eye)],
            [down.x, down.y, down.z, -down.dot(eye)],
            [forward.x, forward.y, forward.z, -forward.dot(eye)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CameraKey3d {
    pub frame: u16,
    pub pose: CameraPose,
}

/// The timeline's 3D camera.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Camera3d {
    /// Off looks through the stage camera.
    pub enabled: bool,
    /// No perspective: far things keep their size.
    pub orthographic: bool,
    pub pose: CameraPose,
    pub keys: Vec<CameraKey3d>,
}

impl Camera3d {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            enabled: false,
            orthographic: false,
            pose: CameraPose::stage(width, height),
            keys: Vec::new(),
        }
    }

    pub fn pose_at(&self, frame: u16, width: u16, height: u16) -> CameraPose {
        if !self.enabled {
            return CameraPose::stage(width, height);
        }
        eased_keys(
            &self.keys,
            frame,
            |key| key.frame,
            |key| key.pose,
            CameraPose::lerp,
        )
        .unwrap_or(self.pose)
    }

    pub fn set_key(&mut self, frame: u16) {
        let key = CameraKey3d {
            frame,
            pose: self.pose,
        };
        match self.keys.binary_search_by_key(&frame, |key| key.frame) {
            Ok(index) => self.keys[index] = key,
            Err(index) => self.keys.insert(index, key),
        }
    }
}

/// Values on keyframes, smoothly eased in between and held outside.
fn eased_keys<K, V>(
    keys: &[K],
    frame: u16,
    frame_of: impl Fn(&K) -> u16,
    value_of: impl Fn(&K) -> V,
    lerp: impl Fn(&V, &V, f32) -> V,
) -> Option<V> {
    let (first, last) = (keys.first()?, keys.last()?);
    if frame <= frame_of(first) {
        return Some(value_of(first));
    }
    if frame >= frame_of(last) {
        return Some(value_of(last));
    }
    let next = keys.partition_point(|key| frame_of(key) <= frame);
    let (a, b) = (&keys[next - 1], &keys[next]);
    let span = f32::from((frame_of(b) - frame_of(a)).max(1));
    let t = f32::from(frame - frame_of(a)) / span;
    let t = t * t * (3.0 - 2.0 * t);
    Some(lerp(&value_of(a), &value_of(b), t))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Light3dKind {
    /// A bulb: shines all around.
    #[default]
    Point,
    /// A torch: a cone from its position toward its target.
    Spot,
    /// The sun: parallel light from its position toward its target.
    Sun,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Light3dKey {
    pub frame: u16,
    pub position: V3,
    pub target: V3,
    pub color: Rgba,
    pub intensity: f32,
    pub range: f32,
}

/// A lamp in 3D. It lights models and drawings placed in 3D alike.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Light3d {
    pub light_id: u16,
    pub name: String,
    pub kind: Light3dKind,
    pub enabled: bool,
    /// Stage space, or offset in the followed object's own space.
    pub position: V3,
    /// Where a spot or the sun points.
    pub target: V3,
    /// An object (instance id) it rides along with.
    pub follow: Option<u32>,
    pub color: Rgba,
    pub intensity: f32,
    /// Pixels it reaches; 0 never fades.
    pub range: f32,
    /// Spot: degrees across the cone.
    pub cone: f32,
    /// Spot: 0 is a hard edge, 1 fades across the whole cone.
    pub softness: f32,
    pub shadows: bool,
    pub keys: Vec<Light3dKey>,
}

impl Light3d {
    pub fn new(light_id: u16, kind: Light3dKind, position: V3, target: V3) -> Self {
        Self {
            light_id,
            name: format!("light3d{light_id}"),
            kind,
            enabled: true,
            position,
            target,
            follow: None,
            color: Rgba {
                r: 255,
                g: 244,
                b: 226,
                a: 255,
            },
            intensity: 1.0,
            range: if kind == Light3dKind::Sun { 0.0 } else { 900.0 },
            cone: 45.0,
            softness: 0.4,
            shadows: false,
            keys: Vec::new(),
        }
    }

    fn own_key(&self, frame: u16) -> Light3dKey {
        Light3dKey {
            frame,
            position: self.position,
            target: self.target,
            color: self.color,
            intensity: self.intensity,
            range: self.range,
        }
    }

    pub fn set_key(&mut self, frame: u16) {
        let key = self.own_key(frame);
        match self.keys.binary_search_by_key(&frame, |key| key.frame) {
            Ok(index) => self.keys[index] = key,
            Err(index) => self.keys.insert(index, key),
        }
    }

    pub fn animated(&self, frame: u16) -> Light3dKey {
        eased_keys(
            &self.keys,
            frame,
            |key| key.frame,
            |key| *key,
            |a, b, t| {
                let mix = |x: f32, y: f32| x + (y - x) * t;
                let channel =
                    |x: u8, y: u8| mix(f32::from(x), f32::from(y)).round().clamp(0.0, 255.0) as u8;
                Light3dKey {
                    frame,
                    position: a.position.lerp(b.position, t),
                    target: a.target.lerp(b.target, t),
                    color: Rgba {
                        r: channel(a.color.r, b.color.r),
                        g: channel(a.color.g, b.color.g),
                        b: channel(a.color.b, b.color.b),
                        a: 255,
                    },
                    intensity: mix(a.intensity, b.intensity),
                    range: mix(a.range, b.range),
                }
            },
        )
        .map(|key| Light3dKey { frame, ..key })
        .unwrap_or_else(|| self.own_key(frame))
    }
}

/// A bone pose keyed on the timeline for one placed model.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PoseKey {
    pub frame: u16,
    /// Nodes and the local transforms they take instead of the clip's.
    pub bones: Vec<(u16, Trs)>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PoseTrack {
    pub q0rg_id: u16,
    pub instance_id: u32,
    pub keys: Vec<PoseKey>,
}

impl PoseTrack {
    /// Applies the keyed bones at `frame` over `locals`, easing each bone
    /// between the keys that hold it.
    pub fn apply(&self, frame: u16, locals: &mut [Trs]) {
        let mut nodes: Vec<u16> = self
            .keys
            .iter()
            .flat_map(|key| key.bones.iter().map(|(node, _)| *node))
            .collect();
        nodes.sort_unstable();
        nodes.dedup();
        for node in nodes {
            let held: Vec<(u16, Trs)> = self
                .keys
                .iter()
                .filter_map(|key| {
                    key.bones
                        .iter()
                        .find(|(n, _)| *n == node)
                        .map(|(_, trs)| (key.frame, *trs))
                })
                .collect();
            let value = eased_keys(
                &held,
                frame,
                |(f, _)| *f,
                |(_, trs)| *trs,
                |a, b, t| a.lerp(b, t),
            );
            if let (Some(value), Some(local)) = (value, locals.get_mut(usize::from(node))) {
                *local = value;
            }
        }
    }

    pub fn key_at_mut(&mut self, frame: u16) -> &mut PoseKey {
        let index = match self.keys.binary_search_by_key(&frame, |key| key.frame) {
            Ok(index) => index,
            Err(index) => {
                self.keys.insert(
                    index,
                    PoseKey {
                        frame,
                        bones: Vec::new(),
                    },
                );
                index
            }
        };
        &mut self.keys[index]
    }
}

/// How one timeline looks in 3D.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Scene3dSetup {
    pub q0rg_id: u16,
    pub camera: Camera3d,
    pub lights: Vec<Light3d>,
    /// Light that is everywhere.
    pub ambient: Rgba,
    pub ambient_intensity: f32,
    /// Flat drawings are lit by the 3D lights as planes at depth 0...
    pub light_flat: bool,
    /// ...on the layers up to and including this one (`None`: all).
    pub light_flat_layer: Option<u16>,
    pub shadows: bool,
    /// Bodies that fall and bump in 3D, baked into keys in the editor.
    #[cfg_attr(feature = "serde", serde(default))]
    pub physics: Physics3d,
    /// Liquids poured in 3D, kept as the particles of every baked frame.
    #[cfg_attr(feature = "serde", serde(default))]
    pub liquids: Vec<Liquid3d>,
    /// Haze that far things fade into.
    #[cfg_attr(feature = "serde", serde(default))]
    pub fog: Fog3d,
}

/// Air thick enough to see: 3D things fade into its colour the further
/// they are from the camera, from `near` to `far` stage pixels away.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fog3d {
    pub on: bool,
    pub color: Rgba,
    pub near: f32,
    pub far: f32,
}

impl Default for Fog3d {
    fn default() -> Self {
        Self {
            on: false,
            color: Rgba {
                r: 196,
                g: 206,
                b: 222,
                a: 255,
            },
            near: 900.0,
            far: 4000.0,
        }
    }
}

/// A liquid in 3D: a block of it let go on a frame, splashing over the
/// floor and the still bodies. Its baked frames hold where its drops are;
/// the renderers draw a smooth surface over them.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Liquid3d {
    pub liquid_id: u16,
    pub name: String,
    /// Drawn with this layer, in its place among the others.
    pub layer_id: u16,
    pub color: Rgba,
    /// Clear like water; otherwise thick and opaque like paint.
    pub clear: bool,
    /// Opposite corners of the block it starts as, in stage space.
    pub from: V3,
    pub to: V3,
    /// Stage pixels between drops: smaller is smoother and slower.
    pub spacing: f32,
    /// Speed at the start, in stage pixels a second.
    pub splash: V3,
    /// 0 runs like water, 1 oozes like honey.
    pub thickness: f32,
    /// How much it clings together into drops.
    pub cling: f32,
    /// Walls at the stage's sides, front and back keep it in.
    pub tank: bool,
    /// The frame its first baked frame shows on.
    pub start_frame: u16,
    /// Every drop on every baked frame, in whole stage pixels.
    pub frames: Vec<Vec<[i16; 3]>>,
}

impl Liquid3d {
    pub fn new(liquid_id: u16, layer_id: u16, from: V3, to: V3) -> Self {
        Self {
            liquid_id,
            name: format!("liquid {liquid_id}"),
            layer_id,
            color: Rgba {
                r: 90,
                g: 170,
                b: 255,
                a: 110,
            },
            clear: true,
            from,
            to,
            spacing: 14.0,
            splash: V3::ZERO,
            thickness: 0.1,
            cling: 0.3,
            tank: false,
            start_frame: 0,
            frames: Vec::new(),
        }
    }

    /// The drops showing on `frame`: the last baked ones after the bake
    /// runs out, none before it starts.
    pub fn drops_at(&self, frame: u16) -> Option<&[[i16; 3]]> {
        let index = usize::from(frame.checked_sub(self.start_frame)?);
        self.frames
            .get(index)
            .or_else(|| self.frames.last())
            .map(Vec::as_slice)
    }
}

/// The shape a 3D body collides as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BodyShape3d {
    /// The object's box.
    #[default]
    Box,
    /// A ball as wide as its box.
    Ball,
}

/// An object that takes part in the 3D physics.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Body3d {
    pub instance_id: u32,
    /// Falls and gets knocked about; otherwise it stays put and others hit it.
    pub falls: bool,
    pub shape: BodyShape3d,
    /// 0 lands dead, 1 bounces back as high.
    pub bounce: f32,
    pub friction: f32,
    pub weight: f32,
    /// Speed at the start, in stage pixels a second.
    pub push: V3,
    /// Turns a second at the start, about x, y and z.
    pub spin: V3,
}

impl Body3d {
    pub fn new(instance_id: u32) -> Self {
        Self {
            instance_id,
            falls: true,
            shape: BodyShape3d::Box,
            bounce: 0.3,
            friction: 0.6,
            weight: 1.0,
            push: V3::ZERO,
            spin: V3::ZERO,
        }
    }
}

/// The 3D physics of one timeline.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Physics3d {
    /// Stage pixels a second, a second; down the stage.
    pub gravity: f32,
    /// A floor things land on.
    pub floor: bool,
    /// How far below the stage's bottom edge the floor lies.
    pub floor_offset: f32,
    pub bodies: Vec<Body3d>,
}

impl Default for Physics3d {
    fn default() -> Self {
        Self {
            gravity: 980.0,
            floor: true,
            floor_offset: 0.0,
            bodies: Vec::new(),
        }
    }
}

impl Physics3d {
    pub fn body(&self, instance_id: u32) -> Option<&Body3d> {
        self.bodies
            .iter()
            .find(|body| body.instance_id == instance_id)
    }

    pub fn body_mut(&mut self, instance_id: u32) -> Option<&mut Body3d> {
        self.bodies
            .iter_mut()
            .find(|body| body.instance_id == instance_id)
    }
}

impl Scene3dSetup {
    pub fn new(q0rg_id: u16, width: u16, height: u16) -> Self {
        Self {
            q0rg_id,
            camera: Camera3d::new(width, height),
            lights: Vec::new(),
            ambient: Rgba {
                r: 70,
                g: 76,
                b: 92,
                a: 255,
            },
            ambient_intensity: 1.0,
            light_flat: false,
            light_flat_layer: None,
            shadows: true,
            physics: Physics3d::default(),
            liquids: Vec::new(),
            fog: Fog3d::default(),
        }
    }

    pub fn light(&self, light_id: u16) -> Option<&Light3d> {
        self.lights.iter().find(|light| light.light_id == light_id)
    }

    pub fn light_mut(&mut self, light_id: u16) -> Option<&mut Light3d> {
        self.lights
            .iter_mut()
            .find(|light| light.light_id == light_id)
    }

    pub fn next_light_id(&self) -> u16 {
        (1..=u16::MAX)
            .find(|id| self.light(*id).is_none())
            .unwrap_or(u16::MAX)
    }

    pub fn has_lights(&self) -> bool {
        self.lights.iter().any(|light| light.enabled)
    }
}

/// All 3D settings of a project.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Scene3dData {
    pub setups: Vec<Scene3dSetup>,
    pub poses: Vec<PoseTrack>,
}

impl Scene3dData {
    pub fn is_empty(&self) -> bool {
        self.setups.is_empty() && self.poses.is_empty()
    }

    pub fn setup(&self, q0rg_id: u16) -> Option<&Scene3dSetup> {
        self.setups.iter().find(|setup| setup.q0rg_id == q0rg_id)
    }

    pub fn setup_mut(&mut self, q0rg_id: u16, width: u16, height: u16) -> &mut Scene3dSetup {
        let index = match self
            .setups
            .iter()
            .position(|setup| setup.q0rg_id == q0rg_id)
        {
            Some(index) => index,
            None => {
                self.setups.push(Scene3dSetup::new(q0rg_id, width, height));
                self.setups.len() - 1
            }
        };
        &mut self.setups[index]
    }

    pub fn pose(&self, q0rg_id: u16, instance_id: u32) -> Option<&PoseTrack> {
        (instance_id != 0)
            .then(|| {
                self.poses
                    .iter()
                    .find(|track| track.q0rg_id == q0rg_id && track.instance_id == instance_id)
            })
            .flatten()
    }

    pub fn pose_mut(&mut self, q0rg_id: u16, instance_id: u32) -> &mut PoseTrack {
        let index = match self
            .poses
            .iter()
            .position(|track| track.q0rg_id == q0rg_id && track.instance_id == instance_id)
        {
            Some(index) => index,
            None => {
                self.poses.push(PoseTrack {
                    q0rg_id,
                    instance_id,
                    keys: Vec::new(),
                });
                self.poses.len() - 1
            }
        };
        &mut self.poses[index]
    }

    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.setups.retain(|setup| seen.insert(setup.q0rg_id));
        for setup in &mut self.setups {
            setup.camera.keys.sort_by_key(|key| key.frame);
            setup.camera.keys.dedup_by_key(|key| key.frame);
            if !setup.camera.pose.zoom.is_finite() || setup.camera.pose.zoom < 1.0 {
                setup.camera.pose.zoom = 1.0;
            }
            for key in &mut setup.camera.keys {
                key.pose.zoom = key.pose.zoom.max(1.0);
            }
            let mut ids = std::collections::HashSet::new();
            setup.lights.retain(|light| ids.insert(light.light_id));
            for light in &mut setup.lights {
                light.keys.sort_by_key(|key| key.frame);
                light.keys.dedup_by_key(|key| key.frame);
                light.intensity = light.intensity.clamp(0.0, 100.0);
                light.range = light.range.max(0.0);
                light.cone = light.cone.clamp(1.0, 179.0);
                light.softness = light.softness.clamp(0.0, 1.0);
            }
        }
        self.poses
            .retain(|track| track.instance_id != 0 && !track.keys.is_empty());
        for track in &mut self.poses {
            track.keys.sort_by_key(|key| key.frame);
            track.keys.dedup_by_key(|key| key.frame);
        }
    }
}

/// One light as it shines on one frame, in stage space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LitLight3d {
    pub kind: Light3dKind,
    pub position: V3,
    /// Unit vector the light travels along (spot and sun).
    pub direction: V3,
    /// Colour times intensity.
    pub color: [f32; 3],
    pub range: f32,
    /// Spot: cosines of the inner and outer cone edges.
    pub cos_inner: f32,
    pub cos_outer: f32,
    pub shadows: bool,
}

impl LitLight3d {
    /// The unit vector toward the light from `point`, and how much of it
    /// arrives there (0..1) before the surface turns.
    pub fn toward(&self, point: V3) -> (V3, f32) {
        match self.kind {
            Light3dKind::Sun => (-self.direction, 1.0),
            Light3dKind::Point | Light3dKind::Spot => {
                let offset = self.position - point;
                let distance = offset.length();
                let l = if distance > 1.0e-6 {
                    offset / distance
                } else {
                    V3::Z * -1.0
                };
                let mut reach = if self.range > 0.0 {
                    let f = (distance / self.range).min(1.0);
                    let window = (1.0 - f * f * f * f).max(0.0);
                    window * window
                } else {
                    1.0
                };
                if self.kind == Light3dKind::Spot {
                    let cos = (-l).dot(self.direction);
                    reach *= smoothstep(self.cos_outer, self.cos_inner, cos);
                }
                (l, reach)
            }
        }
    }
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Lights of one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct LightRig {
    pub ambient: [f32; 3],
    pub lights: Vec<LitLight3d>,
    /// The lights were placed by someone; without them models still get a
    /// soft default light, but flat drawings stay as drawn.
    pub authored: bool,
    pub shadows: bool,
    /// Haze: its colour, and where it starts and is complete, in stage
    /// pixels from the camera.
    pub fog: Option<([f32; 3], f32, f32)>,
}

impl LightRig {
    /// How much of the haze covers a point `distance` from the camera.
    pub fn fog_at(&self, distance: f32) -> Option<([f32; 3], f32)> {
        let (color, near, far) = self.fog?;
        let amount = ((distance - near) / (far - near).max(1.0)).clamp(0.0, 1.0);
        (amount > 0.0).then_some((color, amount))
    }
}

impl LightRig {
    /// Soft light from above-left in front, so models read as solid.
    pub fn default_for(camera: &CameraPose) -> Self {
        let forward = (camera.target - camera.position).normalized();
        let view = camera.view_matrix();
        let inverse = view.inverse().unwrap_or(Mat4::IDENTITY);
        let key = inverse
            .transform_vector(V3::new(0.45, 0.6, 0.65))
            .normalized();
        let fill = inverse
            .transform_vector(V3::new(-0.6, -0.1, 0.8))
            .normalized();
        let _ = forward;
        let sun = |direction: V3, strength: f32| LitLight3d {
            kind: Light3dKind::Sun,
            position: V3::ZERO,
            direction,
            color: [strength; 3],
            range: 0.0,
            cos_inner: 1.0,
            cos_outer: 1.0,
            shadows: false,
        };
        Self {
            ambient: [0.36, 0.37, 0.4],
            lights: vec![sun(key, 0.85), sun(fill, 0.25)],
            authored: false,
            shadows: false,
            fog: None,
        }
    }
}

pub(crate) fn unit_rgb(color: Rgba) -> [f32; 3] {
    [color.r, color.g, color.b].map(|channel| f32::from(channel) / 255.0)
}

impl Scene3dSetup {
    /// The lights of `frame`. `anchor` says where a followed object is.
    pub fn light_rig(
        &self,
        frame: u16,
        camera: &CameraPose,
        mut anchor: impl FnMut(u32) -> Option<Mat4>,
    ) -> LightRig {
        let fog = self.fog.on.then(|| {
            (
                unit_rgb(self.fog.color),
                self.fog.near,
                self.fog.far.max(self.fog.near + 1.0),
            )
        });
        if !self.has_lights() {
            let mut rig = LightRig::default_for(camera);
            rig.ambient = rig
                .ambient
                .map(|c| c * self.ambient_intensity.clamp(0.0, 4.0));
            rig.fog = fog;
            return rig;
        }
        let ambient = unit_rgb(self.ambient).map(|c| c * self.ambient_intensity);
        let lights = self
            .lights
            .iter()
            .filter(|light| light.enabled)
            .take(super::MAX_LIGHTS_3D)
            .filter_map(|light| {
                let key = light.animated(frame);
                let (position, target) = match light.follow {
                    Some(instance) => {
                        let m = anchor(instance)?;
                        (
                            m.transform_point(key.position),
                            m.transform_point(key.target),
                        )
                    }
                    None => (key.position, key.target),
                };
                let mut direction = (target - position).normalized();
                if direction == V3::ZERO {
                    direction = V3::Z;
                }
                let half = (light.cone * 0.5).to_radians();
                let soft = light.softness.clamp(0.0, 1.0);
                Some(LitLight3d {
                    kind: light.kind,
                    position,
                    direction,
                    color: unit_rgb(key.color).map(|c| c * key.intensity),
                    range: key.range,
                    cos_inner: (half * (1.0 - soft)).cos(),
                    cos_outer: half.cos(),
                    shadows: light.shadows && self.shadows,
                })
            })
            .collect();
        LightRig {
            ambient,
            lights,
            authored: true,
            shadows: self.shadows,
            fog,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_camera_shows_depth_zero_one_to_one() {
        let camera = CameraPose::stage(800, 600);
        let view = camera.view_matrix();
        for point in [
            V3::new(0.0, 0.0, 0.0),
            V3::new(800.0, 600.0, 0.0),
            V3::new(123.0, 456.0, 0.0),
        ] {
            let v = view.transform_point(point);
            let x = 400.0 + camera.zoom * v.x / v.z;
            let y = 300.0 + camera.zoom * v.y / v.z;
            assert!(
                (x - point.x).abs() < 0.01 && (y - point.y).abs() < 0.01,
                "{x} {y}"
            );
        }
        let far = view.transform_point(V3::new(800.0, 300.0, camera.zoom));
        let x = 400.0 + camera.zoom * far.x / far.z;
        assert!(
            (x - 600.0).abs() < 0.01,
            "twice as far is half as wide: {x}"
        );
    }

    #[test]
    fn a_placement_without_depth_stays_flat() {
        let local = Affine::from_transform(crate::v2::Transform2D {
            tx: 30.0,
            ty: 40.0,
            sx: 2.0,
            sy: 2.0,
            rotation: 0.5,
            skew_x: 0.0,
            skew_y: 0.0,
        });
        let m = placement_matrix(&Mat4::IDENTITY, local, None, Vec2::new(0.0, 0.0));
        let p = m.transform_point(V3::new(5.0, 7.0, 0.0));
        let q = local.apply(Vec2::new(5.0, 7.0));
        assert!((p.x - q.x).abs() < 1.0e-4 && (p.y - q.y).abs() < 1.0e-4 && p.z.abs() < 1.0e-6);
        let turned = placement_matrix(
            &Mat4::IDENTITY,
            Affine::IDENTITY,
            Some(Space3d {
                tilt_y: 90.0,
                ..Space3d::default()
            }),
            Vec2::new(10.0, 0.0),
        );
        let edge = turned.transform_point(V3::new(20.0, 0.0, 0.0));
        assert!(
            (edge.x - 10.0).abs() < 1.0e-3,
            "turned about its pivot: {edge:?}"
        );
    }

    #[test]
    fn pose_keys_ease_each_bone() {
        let mut track = PoseTrack {
            q0rg_id: 1,
            instance_id: 4,
            keys: Vec::new(),
        };
        let rest = Trs {
            translation: V3::ZERO,
            rotation: Quat::IDENTITY,
            scale: V3::ONE,
        };
        track.key_at_mut(0).bones.push((0, rest));
        track.key_at_mut(10).bones.push((
            0,
            Trs {
                translation: V3::new(10.0, 0.0, 0.0),
                ..rest
            },
        ));
        let mut locals = vec![rest];
        track.apply(5, &mut locals);
        assert!((locals[0].translation.x - 5.0).abs() < 1.0e-4);
    }
}
