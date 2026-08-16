//! .q1s format v2/v3 Р В Р вЂ Р В РІР‚С™Р Р†Р вЂљРЎСљ vector shapes + recursive q0rg (MovieClip) symbols.
//!
//! Binary layout (little-endian):
//!
//!   Header:
//!     [4]   magic "Q1S\0"
//!     [2]   version = 2 (legacy) | 3 (skew) | 4 (blank keyframes) | 5 (asset names) | 6 (layer folders) | 7 (q0v assets) | 8 (easing) | 9 (vector appearance masks) | 10 (post-material appearance fragments) | 11 (appearance field affine) | 12 (layer visibility/locks) | 13 (nested layer folders) | 14 (placement FX)
//!     [2]   flags (reserved = 0)
//!     [2]   asset_count
//!     [2]   q0rg_count
//!   Meta: (same in v2 and v3)
//!     [2+N] name (u16 length + utf8)
//!     [2]   fps, stage_w, stage_h, entry_q0rg_id
//!   Assets[asset_count]:
//!     [2]   asset_id
//!     [1]   kind (1=bitmap, 2=vector, 3=q0v)
//!     v5+: [2+N] optional user-facing name (empty = default label)
//!     ...
//!   Q0rgs[q0rg_count]:
//!     [2]   q0rg_id, [2+N] name, [2] frame_count, [2+N] script, [2] layer_count
//!     layers[layer_count]: [2] layer_id, [2+N] name, [2] placement_count,
//!       placements[]: [2] frame, [1] target_kind, [2] target_id,
//!         v2: transform [20]: tx ty sx sy rot (5 Р В РІР‚СљР Р†Р вЂљРІР‚Сњ f32)
//!         v3+: transform [28]: tx ty sx sy rot skew_x skew_y (7 Р В РІР‚СљР Р†Р вЂљРІР‚Сњ f32)
//!         tween: [1] kind (0=none, 1=linear, 2=eased), [2 if motion] to_frame,
//!           v8 eased: [1] easing kind + payload
//!         v14+: placement FX: [1] sparse flags + optional alpha/blend/blur/glow/shadow payloads
//!       v4+: [2] explicit_keyframe_count, explicit_keyframes[]: [2] frame
//!   v6 layer metadata: [2] entry_count, then entries:
//!     [2] q0rg_id, [2] layer_id, [1] kind, [1] parent flag, [2 if present] parent id, [1] collapsed
//!     v12 appends [1] hidden, [1] locked to every metadata entry
//!   v9 vector appearance metadata: [2] entry_count, then entries:
//!     [2] asset_id, [1] material kind, material payload, [2] erase path count, erase paths[]
//!   v10 post-material fragments append per appearance:
//!     [2] material source path count, source paths[], [2] clip path count, clip paths[]
//!
//! Reading: v2 through v14 are accepted; v2 placements get skew_x/y = 0,
//! v2/v3 layers get no explicit blank-keyframe markers, v2-v4 assets
//! keep deterministic default labels, and v2-v5 projects have ordinary
//! top-level layers without folders.
//!   v11 transformed appearance fields append per appearance:
//!     [24] field affine a11 a12 a21 a22 tx ty (6 x f32)
//! Writing: always v14 (see `Q1S_VERSION_CURRENT`).

use crate::transform::Affine;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::Error;
use crate::io::{write_string_u16, Cursor};

pub const Q1S_V2_MAGIC: [u8; 4] = *b"Q1S\0";
pub const Q1S_VERSION_LEGACY: u16 = 2;
pub const Q1S_VERSION_SKEW: u16 = 3;
pub const Q1S_VERSION_KEYFRAMES: u16 = 4;
pub const Q1S_VERSION_ASSET_NAMES: u16 = 5;
pub const Q1S_VERSION_LAYER_FOLDERS: u16 = 6;
pub const Q1S_VERSION_Q0V_ASSETS: u16 = 7;
pub const Q1S_VERSION_EASING: u16 = 8;
pub const Q1S_VERSION_APPEARANCE_MASKS: u16 = 9;
pub const Q1S_VERSION_APPEARANCE_FRAGMENTS: u16 = 10;
pub const Q1S_VERSION_APPEARANCE_AFFINE: u16 = 11;
pub const Q1S_VERSION_LAYER_STATE: u16 = 12;
pub const Q1S_VERSION_NESTED_LAYER_FOLDERS: u16 = 13;
pub const Q1S_VERSION_PLACEMENT_FX: u16 = 14;
pub const Q1S_VERSION_RIGGING: u16 = 15;
/// Pro rig extensions: toggle controls, node transform channels and the
/// extended deterministic constraint set. v15 remains the basic rig wire.
pub const Q1S_VERSION_RIG_PRO: u16 = 16;
/// Non-destructive vector rig deformers. v15/v16 rig payloads remain unchanged.
pub const Q1S_VERSION_RIG_DEFORMERS: u16 = 17;
/// Pose drivers, explicit mirror metadata and drawing substitutions.
pub const Q1S_VERSION_RIG_POSE_VARIANTS: u16 = 18;
/// Legacy audio gain/mute stored on media placements.
pub const Q1S_VERSION_AUDIO_CLIP_FX: u16 = 19;
/// Audio leaves display-object placements and becomes independent timeline data.
pub const Q1S_VERSION_AUDIO_TIMELINE_CLIPS: u16 = 20;
/// Linked project graph, frame scripts and stable runtime object names.
pub const Q1S_VERSION_PROJECT_RUNTIME: u16 = 21;
pub const Q1S_VERSION_CURRENT: u16 = Q1S_VERSION_PROJECT_RUNTIME;
/// Kept as an alias so external code that imported the v2-era constant keeps
/// compiling. It now means "the current write-out version".
pub const Q1S_V2_VERSION: u16 = Q1S_VERSION_CURRENT;

/// Maximum number of q0rg-to-q0rg edges below a render root.
///
/// A root q0rg is at depth 0, so a chain containing nine q0rgs (eight nested
/// edges) is valid. Validation and every renderer must use this same limit so
/// a project can never validate successfully and then lose deeper content.
pub const MAX_Q0RG_NESTING_DEPTH: u8 = 8;
/// Maximum folder-parent edges in one timeline layer tree.
pub const MAX_LAYER_FOLDER_NESTING_DEPTH: usize = 64;

const ASSET_KIND_BITMAP: u8 = 1;
const ASSET_KIND_VECTOR: u8 = 2;
const ASSET_KIND_Q0V: u8 = 3;
const ASSET_KIND_RIG: u8 = 4;
const TARGET_KIND_ASSET: u8 = 1;
const TARGET_KIND_Q0RG: u8 = 2;
const TWEEN_KIND_NONE: u8 = 0;
const TWEEN_KIND_LINEAR: u8 = 1;
const TWEEN_KIND_EASED: u8 = 2;
const EASING_KIND_LINEAR: u8 = 0;
const EASING_KIND_PRESET: u8 = 1;
const EASING_KIND_CUBIC_BEZIER: u8 = 2;
const LAYER_KIND_NORMAL: u8 = 0;
const LAYER_KIND_FOLDER: u8 = 1;
const VECTOR_MATERIAL_SOLID: u8 = 0;
const VECTOR_MATERIAL_SOFT_HALO: u8 = 1;
const PLACEMENT_FX_OPACITY: u8 = 1 << 0;
const PLACEMENT_FX_BLEND: u8 = 1 << 1;
const PLACEMENT_FX_BLUR: u8 = 1 << 2;
const PLACEMENT_FX_GLOW: u8 = 1 << 3;
const PLACEMENT_FX_SHADOW: u8 = 1 << 4;
const PLACEMENT_FX_AUDIO_GAIN: u8 = 1 << 5;
const PLACEMENT_FX_AUDIO_MUTED: u8 = 1 << 6;
const BLEND_MODE_NORMAL: u8 = 0;
const BLEND_MODE_MULTIPLY: u8 = 1;
const BLEND_MODE_SCREEN: u8 = 2;
const BLEND_MODE_ADD: u8 = 3;
const BLEND_MODE_OVERLAY: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub color: Rgba,
    pub width: f32,
    /// Cap shape applied to open paths' endpoints. On-disk the field is
    /// optional (legacy strokes default to `Round`) so old files still
    /// load Р В Р вЂ Р В РІР‚С™Р Р†Р вЂљРЎСљ see the `flag == 1` vs `flag == 2` branches in the
    /// parser/writer.
    pub cap: crate::geom::CapShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMeta {
    pub name: String,
    pub fps: u16,
    pub stage_width: u16,
    pub stage_height: u16,
    pub entry_q0rg_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitmapAsset {
    pub asset_id: u16,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub point: Vec2,
    pub in_handle: Option<Vec2>,
    pub out_handle: Option<Vec2>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorAsset {
    pub asset_id: u16,
    pub paths: Vec<Path>,
    pub fill: Option<Rgba>,
    pub stroke: Option<Stroke>,
}

/// Rendering material attached to a vector asset. Geometry stays canonical;
/// material rendering and destructive-looking edits such as the mask eraser
/// live on top of it instead of rewriting the source paths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VectorMaterial {
    Solid,
    SoftHalo { radius: f32, opacity: f32 },
}

/// Sparse per-vector appearance state. `erase_mask` is expressed in the
/// vector asset's local coordinates and is applied after material evaluation.
/// `material_source` freezes the pre-split source used by filters, while
/// `clip_mask` partitions that resolved appearance after the filter. `field_transform`
/// transforms that already-resolved field as one affine surface; this is what makes
/// rotate/skew/scale affect the glow itself instead of re-running an axis-aligned blur.
/// Empty source/clip vectors preserve the legacy behaviour: current vector geometry
/// is the material source and the whole finite material support is visible.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorAppearance {
    pub material: VectorMaterial,
    pub erase_mask: Vec<Path>,
    pub material_source: Vec<Path>,
    pub clip_mask: Vec<Path>,
    pub field_transform: Affine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Q0vAsset {
    pub asset_id: u16,
    /// Complete embedded `.q0v` bytes. Projects stay portable and never rely
    /// on an external absolute media path.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigControlKind {
    Position2D,
    Rotation,
    Slider,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigBinding {
    /// Persistent identity of the logical display object track. Unlike a
    /// placement vector index or target occurrence this survives reordering,
    /// keyframe materialization and target substitutions.
    pub instance_id: u32,
    /// Matrix from the owning bone's rest-space to the bound display object.
    pub bind_offset: Affine,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigNode {
    pub node_id: u16,
    pub name: String,
    pub parent: Option<u16>,
    pub rest: Transform2D,
    pub length: f32,
    pub binding: Option<RigBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigControl {
    pub control_id: u16,
    pub name: String,
    pub kind: RigControlKind,
    pub target_node: Option<u16>,
    pub rest_x: f32,
    pub rest_y: f32,
    pub rest_value: f32,
    pub min_value: f32,
    pub max_value: f32,
    pub public_in_simple: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigConstraint {
    RotationLimit {
        constraint_id: u16,
        node_id: u16,
        min_radians: f32,
        max_radians: f32,
    },
    PositionLimit {
        constraint_id: u16,
        node_id: u16,
        min_x: f32,
        max_x: f32,
        min_y: f32,
        max_y: f32,
    },
    Aim {
        constraint_id: u16,
        node_id: u16,
        target_control: u16,
        angle_offset: f32,
        weight: f32,
    },
    Transform {
        constraint_id: u16,
        node_id: u16,
        target_node: u16,
        position_weight: f32,
        rotation_weight: f32,
    },
    Distance {
        constraint_id: u16,
        node_id: u16,
        target_control: u16,
        min_distance: f32,
        max_distance: f32,
        weight: f32,
    },
    TwoBoneIk {
        constraint_id: u16,
        root_node: u16,
        mid_node: u16,
        tip_node: u16,
        target_control: u16,
        pole_control: Option<u16>,
        weight: f32,
        allow_stretch: bool,
        max_stretch: f32,
    },
}

impl RigConstraint {
    pub const fn id(&self) -> u16 {
        match *self {
            Self::RotationLimit { constraint_id, .. }
            | Self::PositionLimit { constraint_id, .. }
            | Self::Aim { constraint_id, .. }
            | Self::Transform { constraint_id, .. }
            | Self::Distance { constraint_id, .. }
            | Self::TwoBoneIk { constraint_id, .. } => constraint_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RigPropertyRef {
    NodeTx(u16),
    NodeTy(u16),
    NodeRotation(u16),
    NodeScaleX(u16),
    NodeScaleY(u16),
    ControlX(u16),
    ControlY(u16),
    ControlValue(u16),
    ConstraintWeight(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigKey {
    pub frame: u16,
    pub value: f32,
    pub easing: Easing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigChannel {
    pub property: RigPropertyRef,
    pub keys: Vec<RigKey>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigDriver {
    pub driver_id: u16,
    pub source_control: u16,
    pub source_min: f32,
    pub source_max: f32,
    pub target: RigPropertyRef,
    pub target_min: f32,
    pub target_max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigPoseValue {
    pub property: RigPropertyRef,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigPosePreset {
    pub pose_id: u16,
    pub name: String,
    pub values: Vec<RigPoseValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigPoseBlendMode {
    Override,
    Additive,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigPoseDriver {
    pub driver_id: u16,
    pub source_control: u16,
    pub pose_id: u16,
    pub source_min: f32,
    pub source_max: f32,
    pub weight_min: f32,
    pub weight_max: f32,
    pub mode: RigPoseBlendMode,
}

/// Explicit mirror relation between exact rig properties. Forward mapping is
/// `right = left * multiplier + offset`; reverse uses the algebraic inverse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigMirrorPair {
    pub left: RigPropertyRef,
    pub right: RigPropertyRef,
    pub multiplier: f32,
    pub offset: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigVariantChoice {
    pub name: String,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigVariantSet {
    pub variant_id: u16,
    pub name: String,
    pub instance_id: u32,
    pub source_control: u16,
    pub choices: Vec<RigVariantChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigSkinBoneBind {
    pub node_id: u16,
    pub inverse_rest_world: Affine,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigSkinWeight {
    pub node_id: u16,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigSkinAnchorWeights {
    pub path_index: u16,
    pub anchor_index: u16,
    pub weights: Vec<RigSkinWeight>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigDeformer {
    Skin {
        deformer_id: u16,
        instance_id: u32,
        asset_id: u16,
        bind_transform: Affine,
        bones: Vec<RigSkinBoneBind>,
        anchors: Vec<RigSkinAnchorWeights>,
    },
    Bend {
        deformer_id: u16,
        instance_id: u32,
        asset_id: u16,
        bind_transform: Affine,
        axis_start: Vec2,
        axis_end: Vec2,
        start_control: u16,
        middle_control: u16,
        end_control: u16,
    },
    Cage {
        deformer_id: u16,
        instance_id: u32,
        asset_id: u16,
        bind_transform: Affine,
        rest_min: Vec2,
        rest_max: Vec2,
        controls: [u16; 4],
    },
}

impl RigDeformer {
    pub const fn id(&self) -> u16 {
        match *self {
            Self::Skin { deformer_id, .. }
            | Self::Bend { deformer_id, .. }
            | Self::Cage { deformer_id, .. } => deformer_id,
        }
    }

    pub const fn instance_id(&self) -> u32 {
        match *self {
            Self::Skin { instance_id, .. }
            | Self::Bend { instance_id, .. }
            | Self::Cage { instance_id, .. } => instance_id,
        }
    }

    pub const fn asset_id(&self) -> u16 {
        match *self {
            Self::Skin { asset_id, .. }
            | Self::Bend { asset_id, .. }
            | Self::Cage { asset_id, .. } => asset_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigAsset {
    pub asset_id: u16,
    pub owner_q0rg_id: u16,
    pub nodes: Vec<RigNode>,
    pub controls: Vec<RigControl>,
    pub constraints: Vec<RigConstraint>,
    pub channels: Vec<RigChannel>,
    pub drivers: Vec<RigDriver>,
    pub poses: Vec<RigPosePreset>,
    pub deformers: Vec<RigDeformer>,
    pub pose_drivers: Vec<RigPoseDriver>,
    pub mirror_pairs: Vec<RigMirrorPair>,
    pub variants: Vec<RigVariantSet>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Asset {
    Bitmap(BitmapAsset),
    Vector(VectorAsset),
    Q0v(Q0vAsset),
    Rig(RigAsset),
}

impl Asset {
    pub fn id(&self) -> u16 {
        match self {
            Asset::Bitmap(b) => b.asset_id,
            Asset::Vector(v) => v.asset_id,
            Asset::Q0v(v) => v.asset_id,
            Asset::Rig(v) => v.asset_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub tx: f32,
    pub ty: f32,
    pub sx: f32,
    pub sy: f32,
    pub rotation: f32,
    /// Horizontal shear in radians: 0 = no skew. Applied in local space along
    /// with sy (i.e. y-coordinates pull horizontally with `tan(skew_x)`).
    pub skew_x: f32,
    /// Vertical shear in radians: 0 = no skew. x-coordinates push vertically
    /// with `tan(skew_y)`.
    pub skew_y: f32,
}

impl Transform2D {
    pub const IDENTITY: Self = Self {
        tx: 0.0,
        ty: 0.0,
        sx: 1.0,
        sy: 1.0,
        rotation: 0.0,
        skew_x: 0.0,
        skew_y: 0.0,
    };
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Asset(u16),
    Q0rg(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EasingFamily {
    Quad = 0,
    Cubic = 1,
    Quart = 2,
    Quint = 3,
    Sine = 4,
    Expo = 5,
    Circ = 6,
    Back = 7,
    Elastic = 8,
    Bounce = 9,
}

impl EasingFamily {
    pub const ALL: [Self; 10] = [
        Self::Quad,
        Self::Cubic,
        Self::Quart,
        Self::Quint,
        Self::Sine,
        Self::Expo,
        Self::Circ,
        Self::Back,
        Self::Elastic,
        Self::Bounce,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Quad => "Quad",
            Self::Cubic => "Cubic",
            Self::Quart => "Quart",
            Self::Quint => "Quint",
            Self::Sine => "Sine",
            Self::Expo => "Expo",
            Self::Circ => "Circ",
            Self::Back => "Back",
            Self::Elastic => "Elastic",
            Self::Bounce => "Bounce",
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EasingMode {
    In = 0,
    Out = 1,
    InOut = 2,
}

impl EasingMode {
    pub const ALL: [Self; 3] = [Self::In, Self::Out, Self::InOut];

    pub const fn label(self) -> &'static str {
        match self {
            Self::In => "Ease In",
            Self::Out => "Ease Out",
            Self::InOut => "Ease In Out",
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Easing {
    #[default]
    Linear,
    Preset {
        family: EasingFamily,
        mode: EasingMode,
    },
    CubicBezier {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

impl Easing {
    pub fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Preset { family, mode } => match mode {
                EasingMode::In => easing_in(family, t),
                EasingMode::Out => 1.0 - easing_in(family, 1.0 - t),
                EasingMode::InOut => {
                    if t < 0.5 {
                        0.5 * easing_in(family, t * 2.0)
                    } else {
                        1.0 - 0.5 * easing_in(family, (1.0 - t) * 2.0)
                    }
                }
            },
            Self::CubicBezier { x1, y1, x2, y2 } => sample_cubic_bezier(x1, y1, x2, y2, t),
        }
    }

    pub fn is_valid(self) -> bool {
        match self {
            Self::Linear | Self::Preset { .. } => true,
            Self::CubicBezier { x1, y1, x2, y2 } => {
                x1.is_finite()
                    && y1.is_finite()
                    && x2.is_finite()
                    && y2.is_finite()
                    && (0.0..=1.0).contains(&x1)
                    && (0.0..=1.0).contains(&x2)
                    && (-8.0..=8.0).contains(&y1)
                    && (-8.0..=8.0).contains(&y2)
            }
        }
    }
}

fn easing_in(family: EasingFamily, t: f32) -> f32 {
    use std::f32::consts::PI;
    match family {
        EasingFamily::Quad => t * t,
        EasingFamily::Cubic => t * t * t,
        EasingFamily::Quart => t.powi(4),
        EasingFamily::Quint => t.powi(5),
        EasingFamily::Sine => 1.0 - (t * PI * 0.5).cos(),
        EasingFamily::Expo => {
            if t <= 0.0 {
                0.0
            } else {
                2.0_f32.powf(10.0 * t - 10.0)
            }
        }
        EasingFamily::Circ => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
        EasingFamily::Back => {
            const C1: f32 = 1.70158;
            const C3: f32 = C1 + 1.0;
            C3 * t * t * t - C1 * t * t
        }
        EasingFamily::Elastic => {
            if t <= 0.0 || t >= 1.0 {
                t
            } else {
                const C4: f32 = (2.0 * PI) / 3.0;
                -2.0_f32.powf(10.0 * t - 10.0) * ((t * 10.0 - 10.75) * C4).sin()
            }
        }
        EasingFamily::Bounce => 1.0 - bounce_out(1.0 - t),
    }
}

fn bounce_out(t: f32) -> f32 {
    const N1: f32 = 7.5625;
    const D1: f32 = 2.75;
    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let t = t - 1.5 / D1;
        N1 * t * t + 0.75
    } else if t < 2.5 / D1 {
        let t = t - 2.25 / D1;
        N1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / D1;
        N1 * t * t + 0.984375
    }
}

fn cubic_bezier_coordinate(a: f32, b: f32, t: f32) -> f32 {
    let inv = 1.0 - t;
    3.0 * inv * inv * t * a + 3.0 * inv * t * t * b + t * t * t
}

fn sample_cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..18 {
        let mid = (low + high) * 0.5;
        if cubic_bezier_coordinate(x1, x2, mid) < x {
            low = mid;
        } else {
            high = mid;
        }
    }
    cubic_bezier_coordinate(y1, y2, (low + high) * 0.5)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tween {
    None,
    Linear { to_frame: u16 },
    Eased { to_frame: u16, easing: Easing },
}

impl Tween {
    pub const fn to_frame(self) -> Option<u16> {
        match self {
            Self::None => None,
            Self::Linear { to_frame } | Self::Eased { to_frame, .. } => Some(to_frame),
        }
    }

    pub const fn easing(self) -> Easing {
        match self {
            Self::Eased { easing, .. } => easing,
            Self::None | Self::Linear { .. } => Easing::Linear,
        }
    }

    pub const fn with_to_frame(self, to_frame: u16) -> Self {
        match self {
            Self::Eased { easing, .. } => Self::Eased { to_frame, easing },
            Self::None | Self::Linear { .. } => Self::Linear { to_frame },
        }
    }

    pub const fn with_easing(self, easing: Easing) -> Self {
        match self.to_frame() {
            Some(to_frame) => {
                if matches!(easing, Easing::Linear) {
                    Self::Linear { to_frame }
                } else {
                    Self::Eased { to_frame, easing }
                }
            }
            None => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Add,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlurFx {
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowFx {
    pub color: Rgba,
    pub radius: f32,
    pub strength: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropShadowFx {
    pub color: Rgba,
    pub blur_radius: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub strength: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementFx {
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub blur: Option<BlurFx>,
    pub glow: Option<GlowFx>,
    pub shadow: Option<DropShadowFx>,
    pub audio_gain: f32,
    pub audio_muted: bool,
}

impl Default for PlacementFx {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            blur: None,
            glow: None,
            shadow: None,
            audio_gain: 1.0,
            audio_muted: false,
        }
    }
}

impl PlacementFx {
    pub fn is_identity(self) -> bool {
        let blur_identity = self.blur.is_none_or(|blur| blur.radius <= 1.0e-6);
        let glow_identity = self
            .glow
            .is_none_or(|glow| glow.strength <= 1.0e-6 || glow.color.a == 0);
        let shadow_identity = self
            .shadow
            .is_none_or(|shadow| shadow.strength <= 1.0e-6 || shadow.color.a == 0);
        self.opacity >= 0.999_999
            && self.blend_mode == BlendMode::Normal
            && blur_identity
            && glow_identity
            && shadow_identity
    }

    pub fn lerp_to(self, target: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            opacity: lerp_f32(self.opacity, target.opacity, t),
            blend_mode: if t >= 1.0 {
                target.blend_mode
            } else {
                self.blend_mode
            },
            blur: lerp_blur(self.blur, target.blur, t),
            glow: lerp_glow(self.glow, target.glow, t),
            shadow: lerp_shadow(self.shadow, target.shadow, t),
            audio_gain: lerp_f32(self.audio_gain, target.audio_gain, t),
            audio_muted: if t >= 1.0 {
                target.audio_muted
            } else {
                self.audio_muted
            },
        }
    }
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    lerp_f32(f32::from(a), f32::from(b), t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: lerp_u8(a.r, b.r, t),
        g: lerp_u8(a.g, b.g, t),
        b: lerp_u8(a.b, b.b, t),
        a: lerp_u8(a.a, b.a, t),
    }
}

fn lerp_blur(a: Option<BlurFx>, b: Option<BlurFx>, t: f32) -> Option<BlurFx> {
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(BlurFx {
            radius: lerp_f32(a.radius, b.radius, t),
        }),
        (None, Some(b)) => Some(BlurFx {
            radius: lerp_f32(0.0, b.radius, t),
        }),
        (Some(a), None) => (t < 1.0).then_some(BlurFx {
            radius: lerp_f32(a.radius, 0.0, t),
        }),
    }
}

fn lerp_glow(a: Option<GlowFx>, b: Option<GlowFx>, t: f32) -> Option<GlowFx> {
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(GlowFx {
            color: lerp_rgba(a.color, b.color, t),
            radius: lerp_f32(a.radius, b.radius, t),
            strength: lerp_f32(a.strength, b.strength, t),
        }),
        (None, Some(b)) => Some(GlowFx {
            color: b.color,
            radius: b.radius,
            strength: lerp_f32(0.0, b.strength, t),
        }),
        (Some(a), None) => (t < 1.0).then_some(GlowFx {
            strength: lerp_f32(a.strength, 0.0, t),
            ..a
        }),
    }
}

fn lerp_shadow(a: Option<DropShadowFx>, b: Option<DropShadowFx>, t: f32) -> Option<DropShadowFx> {
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(DropShadowFx {
            color: lerp_rgba(a.color, b.color, t),
            blur_radius: lerp_f32(a.blur_radius, b.blur_radius, t),
            offset_x: lerp_f32(a.offset_x, b.offset_x, t),
            offset_y: lerp_f32(a.offset_y, b.offset_y, t),
            strength: lerp_f32(a.strength, b.strength, t),
        }),
        (None, Some(b)) => Some(DropShadowFx {
            strength: lerp_f32(0.0, b.strength, t),
            ..b
        }),
        (Some(a), None) => (t < 1.0).then_some(DropShadowFx {
            strength: lerp_f32(a.strength, 0.0, t),
            ..a
        }),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// Persistent logical display-object identity. Zero means "not assigned"
    /// and is valid for ordinary unrigged content; rig bindings always require
    /// a non-zero id.
    pub instance_id: u32,
    pub frame: u16,
    pub target: Target,
    pub transform: Transform2D,
    pub tween: Tween,
    pub fx: PlacementFx,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub layer_id: u16,
    pub name: String,
    /// Explicit keyframe markers. Placement frames are keyframes implicitly;
    /// this list is what lets a layer contain a real keyframe with no content.
    pub explicit_keyframes: Vec<u16>,
    pub placements: Vec<Placement>,
}

impl Layer {
    /// Sorted, deduplicated union of explicit markers and placement frames.
    pub fn keyframe_frames(&self) -> Vec<u16> {
        let mut frames = self.explicit_keyframes.clone();
        frames.extend(self.placements.iter().map(|placement| placement.frame));
        frames.sort_unstable();
        frames.dedup();
        frames
    }

    pub fn has_keyframe(&self, frame: u16) -> bool {
        self.explicit_keyframes.contains(&frame)
            || self
                .placements
                .iter()
                .any(|placement| placement.frame == frame)
    }

    pub fn is_blank_keyframe(&self, frame: u16) -> bool {
        self.explicit_keyframes.contains(&frame)
            && !self
                .placements
                .iter()
                .any(|placement| placement.frame == frame)
    }

    pub fn ensure_explicit_keyframe(&mut self, frame: u16) {
        if !self.explicit_keyframes.contains(&frame) {
            self.explicit_keyframes.push(frame);
            self.explicit_keyframes.sort_unstable();
        }
    }

    pub fn remove_keyframe(&mut self, frame: u16) -> usize {
        self.explicit_keyframes
            .retain(|candidate| *candidate != frame);
        let before = self.placements.len();
        self.placements.retain(|placement| placement.frame != frame);
        before - self.placements.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerKey {
    pub q0rg_id: u16,
    pub layer_id: u16,
}

impl LayerKey {
    pub const fn new(q0rg_id: u16, layer_id: u16) -> Self {
        Self { q0rg_id, layer_id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerKind {
    #[default]
    Normal,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LayerMetadata {
    pub kind: LayerKind,
    pub parent_folder_id: Option<u16>,
    pub collapsed: bool,
    /// Hidden layers are omitted from editor rendering and exported playback.
    /// Missing legacy metadata defaults to visible (`false`).
    pub hidden: bool,
    /// Locked layers remain visible but are excluded from artwork editing and
    /// selection. Folder locks apply effectively to their child layers.
    pub locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioClip {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub start_frame: u16,
    pub asset_id: u16,
    pub gain: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectDependencyKind {
    Movie,
    Q0lang,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDependencySource {
    External(String),
    Embedded(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDependencyNode {
    pub node_id: u16,
    pub parent_node_id: Option<u16>,
    pub alias: String,
    pub kind: ProjectDependencyKind,
    pub source: ProjectDependencySource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectGraph {
    pub nodes: Vec<ProjectDependencyNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameScript {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceKey {
    pub q0rg_id: u16,
    pub instance_id: u32,
}

impl InstanceKey {
    pub const fn new(q0rg_id: u16, instance_id: u32) -> Self {
        Self {
            q0rg_id,
            instance_id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectRuntimeData {
    pub project_graph: ProjectGraph,
    pub frame_scripts: Vec<FrameScript>,
    pub instance_names: HashMap<InstanceKey, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Q0rg {
    pub q0rg_id: u16,
    pub name: String,
    pub frame_count: u16,
    pub script: String,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectV2 {
    pub meta: ProjectMeta,
    pub assets: Vec<Asset>,
    /// Optional user-facing names for assets. Missing entries keep the
    /// deterministic legacy label (for example `Vector 7`).
    pub asset_names: HashMap<u16, String>,
    /// Sparse appearance state for vector assets. Missing entries render with
    /// the legacy fill/stroke path and preserve old project behaviour exactly.
    pub asset_appearances: HashMap<u16, VectorAppearance>,
    /// Sparse structural metadata for timeline layers. Missing entries are
    /// ordinary top-level layers, preserving legacy project behaviour.
    pub layer_metadata: HashMap<LayerKey, LayerMetadata>,
    /// Audio clips live beside the visual timeline rather than inside display
    /// placements, so they can never become stage objects or visual keyframes.
    pub audio_clips: Vec<AudioClip>,
    /// Runtime/project-link metadata introduced after the visual/audio core.
    /// Old files decode this as an empty/default block.
    pub runtime: ProjectRuntimeData,
    pub q0rgs: Vec<Q0rg>,
}

pub fn audio_clip_end_frame(project: &ProjectV2, clip: AudioClip) -> Option<u16> {
    let media = project.assets.iter().find_map(|asset| match asset {
        Asset::Q0v(media) if media.asset_id == clip.asset_id => Some(media),
        _ => None,
    })?;
    let header = q0video::q0v::probe_header(&media.bytes).ok()?;
    if !header.spec.audio || header.spec.video {
        return None;
    }
    let duration_frames = header
        .audio_samples_per_channel
        .saturating_mul(u64::from(project.meta.fps.max(1)))
        .div_ceil(u64::from(header.spec.audio_sample_rate.max(1)))
        .max(1);
    u64::from(clip.start_frame)
        .checked_add(duration_frames)
        .and_then(|end| u16::try_from(end).ok())
}

/// Return the exact deterministic ordering used by the current q1s/q0s writer.
///
/// Asset ids, q0rg ids, placement frame groups, and explicit keyframe markers
/// have no cross-group ordering semantics in the file format. Layer order and
/// the relative order of placements on the same frame remain untouched because
/// they affect rendering.
#[cfg(test)]
pub(crate) fn canonicalized_for_wire(project: &ProjectV2) -> ProjectV2 {
    let mut canonical = project.clone();
    canonical.assets.sort_by_key(Asset::id);
    canonical
        .audio_clips
        .sort_by_key(|clip| (clip.q0rg_id, clip.layer_id, clip.start_frame, clip.asset_id));
    canonical
        .runtime
        .project_graph
        .nodes
        .sort_by_key(|node| node.node_id);
    canonical
        .runtime
        .frame_scripts
        .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));
    canonical.q0rgs.sort_by_key(|q0rg| q0rg.q0rg_id);
    for q0rg in &mut canonical.q0rgs {
        for layer in &mut q0rg.layers {
            layer.placements.sort_by_key(|placement| placement.frame);
            layer.explicit_keyframes.sort_unstable();
        }
    }
    canonical
}

/// Compare two projects exactly as the current q1s/q0s wire format represents
/// them, without cloning embedded bitmap or q0v payloads.
/// Deterministically assign persistent display-object identities to placements
/// that came from pre-rig project revisions (or other id-less editor data).
/// Existing non-zero ids are preserved. For legacy data the old display-track
/// semantics were target + occurrence on each authored frame, so the migration
/// uses that exact relation only once to bootstrap stable ids.
pub fn assign_missing_instance_ids(project: &mut ProjectV2) -> Result<(), Error> {
    let mut used = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| (placement.instance_id != 0).then_some(placement.instance_id))
        .collect::<HashSet<_>>();
    let mut next = 1_u32;
    let mut allocate = || -> Result<u32, Error> {
        while next != 0 && used.contains(&next) {
            next = next
                .checked_add(1)
                .ok_or(Error::Overflow("placement instance id space exhausted"))?;
        }
        if next == 0 {
            return Err(Error::Overflow("placement instance id space exhausted"));
        }
        let id = next;
        used.insert(id);
        next = next.checked_add(1).unwrap_or(0);
        Ok(id)
    };

    for q0rg in &mut project.q0rgs {
        for layer in &mut q0rg.layers {
            let mut ordered = (0..layer.placements.len()).collect::<Vec<_>>();
            ordered.sort_by_key(|index| (layer.placements[*index].frame, *index));
            let mut occurrence_by_target = HashMap::<(u8, u16), u16>::new();
            let mut current_frame = None;
            let mut track_ids = HashMap::<(u8, u16, u16), u32>::new();

            // Learn existing identities first so a partially migrated track
            // propagates its already-persistent id to id-less keyframes.
            for index in &ordered {
                let placement = &layer.placements[*index];
                if current_frame != Some(placement.frame) {
                    current_frame = Some(placement.frame);
                    occurrence_by_target.clear();
                }
                let target = match placement.target {
                    Target::Asset(id) => (0_u8, id),
                    Target::Q0rg(id) => (1_u8, id),
                };
                let occurrence = occurrence_by_target.entry(target).or_insert(0);
                let key = (target.0, target.1, *occurrence);
                *occurrence = occurrence.saturating_add(1);
                if placement.instance_id != 0 {
                    track_ids.entry(key).or_insert(placement.instance_id);
                }
            }

            occurrence_by_target.clear();
            current_frame = None;
            for index in ordered {
                let placement = &mut layer.placements[index];
                if current_frame != Some(placement.frame) {
                    current_frame = Some(placement.frame);
                    occurrence_by_target.clear();
                }
                let target = match placement.target {
                    Target::Asset(id) => (0_u8, id),
                    Target::Q0rg(id) => (1_u8, id),
                };
                let occurrence = occurrence_by_target.entry(target).or_insert(0);
                let key = (target.0, target.1, *occurrence);
                *occurrence = occurrence.saturating_add(1);
                if placement.instance_id == 0 {
                    let id = match track_ids.get(&key).copied() {
                        Some(id) => id,
                        None => {
                            let id = allocate()?;
                            track_ids.insert(key, id);
                            id
                        }
                    };
                    placement.instance_id = id;
                }
            }
        }
    }
    validate(project)
}

pub fn wire_equivalent(left: &ProjectV2, right: &ProjectV2) -> bool {
    if left.meta != right.meta
        || left.asset_names != right.asset_names
        || left.asset_appearances != right.asset_appearances
        || left.layer_metadata != right.layer_metadata
        || {
            let mut left_runtime = left.runtime.clone();
            let mut right_runtime = right.runtime.clone();
            left_runtime
                .project_graph
                .nodes
                .sort_by_key(|node| node.node_id);
            right_runtime
                .project_graph
                .nodes
                .sort_by_key(|node| node.node_id);
            left_runtime
                .frame_scripts
                .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));
            right_runtime
                .frame_scripts
                .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));
            left_runtime != right_runtime
        }
        || left.assets.len() != right.assets.len()
        || left.q0rgs.len() != right.q0rgs.len()
    {
        return false;
    }

    let mut left_assets = left.assets.iter().collect::<Vec<_>>();
    let mut right_assets = right.assets.iter().collect::<Vec<_>>();
    left_assets.sort_by_key(|asset| asset.id());
    right_assets.sort_by_key(|asset| asset.id());
    if left_assets != right_assets {
        return false;
    }

    let mut left_q0rgs = left.q0rgs.iter().collect::<Vec<_>>();
    let mut right_q0rgs = right.q0rgs.iter().collect::<Vec<_>>();
    left_q0rgs.sort_by_key(|q0rg| q0rg.q0rg_id);
    right_q0rgs.sort_by_key(|q0rg| q0rg.q0rg_id);
    for (left_q0rg, right_q0rg) in left_q0rgs.into_iter().zip(right_q0rgs) {
        if left_q0rg.q0rg_id != right_q0rg.q0rg_id
            || left_q0rg.name != right_q0rg.name
            || left_q0rg.frame_count != right_q0rg.frame_count
            || left_q0rg.script != right_q0rg.script
            || left_q0rg.layers.len() != right_q0rg.layers.len()
        {
            return false;
        }

        for (left_layer, right_layer) in left_q0rg.layers.iter().zip(&right_q0rg.layers) {
            if left_layer.layer_id != right_layer.layer_id
                || left_layer.name != right_layer.name
                || left_layer.placements.len() != right_layer.placements.len()
                || left_layer.explicit_keyframes.len() != right_layer.explicit_keyframes.len()
            {
                return false;
            }

            let mut left_keyframes = left_layer.explicit_keyframes.clone();
            let mut right_keyframes = right_layer.explicit_keyframes.clone();
            left_keyframes.sort_unstable();
            right_keyframes.sort_unstable();
            if left_keyframes != right_keyframes {
                return false;
            }

            let mut left_placements = left_layer.placements.iter().collect::<Vec<_>>();
            let mut right_placements = right_layer.placements.iter().collect::<Vec<_>>();
            left_placements.sort_by_key(|placement| placement.frame);
            right_placements.sort_by_key(|placement| placement.frame);
            if left_placements != right_placements {
                return false;
            }
        }
    }
    true
}

impl ProjectV2 {
    pub fn layer_metadata(&self, q0rg_id: u16, layer_id: u16) -> LayerMetadata {
        self.layer_metadata
            .get(&LayerKey::new(q0rg_id, layer_id))
            .copied()
            .unwrap_or_default()
    }

    pub fn layer_is_folder(&self, q0rg_id: u16, layer_id: u16) -> bool {
        self.layer_metadata(q0rg_id, layer_id).kind == LayerKind::Folder
    }

    pub fn layer_parent_folder(&self, q0rg_id: u16, layer_id: u16) -> Option<u16> {
        self.layer_metadata(q0rg_id, layer_id).parent_folder_id
    }

    /// Number of containing layer folders. Invalid/cyclic transient editor state
    /// is bounded so UI queries can never loop forever before validation runs.
    pub fn layer_folder_depth(&self, q0rg_id: u16, layer_id: u16) -> usize {
        let mut depth = 0usize;
        let mut current = self.layer_parent_folder(q0rg_id, layer_id);
        let mut visited = HashSet::new();
        while let Some(parent_id) = current {
            if !visited.insert(parent_id) || depth >= MAX_LAYER_FOLDER_NESTING_DEPTH {
                break;
            }
            depth += 1;
            current = self.layer_parent_folder(q0rg_id, parent_id);
        }
        depth
    }

    /// True when `layer_id` lives anywhere below `ancestor_folder_id`.
    pub fn layer_is_descendant_of(
        &self,
        q0rg_id: u16,
        layer_id: u16,
        ancestor_folder_id: u16,
    ) -> bool {
        let mut current = self.layer_parent_folder(q0rg_id, layer_id);
        let mut visited = HashSet::new();
        while let Some(parent_id) = current {
            if parent_id == ancestor_folder_id {
                return true;
            }
            if !visited.insert(parent_id) || visited.len() > MAX_LAYER_FOLDER_NESTING_DEPTH {
                return false;
            }
            current = self.layer_parent_folder(q0rg_id, parent_id);
        }
        false
    }

    /// Whether artwork on this layer should be rendered. Hidden state inherits
    /// through every containing folder without overwriting descendant flags.
    pub fn layer_is_visible(&self, q0rg_id: u16, layer_id: u16) -> bool {
        let mut current = Some(layer_id);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id) || visited.len() > MAX_LAYER_FOLDER_NESTING_DEPTH + 1 {
                return false;
            }
            let metadata = self.layer_metadata(q0rg_id, id);
            if metadata.hidden {
                return false;
            }
            current = metadata.parent_folder_id;
        }
        true
    }

    /// Whether artwork on this layer is protected from editing. Lock state
    /// inherits through every containing folder while preserving local flags.
    pub fn layer_is_locked(&self, q0rg_id: u16, layer_id: u16) -> bool {
        let mut current = Some(layer_id);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id) || visited.len() > MAX_LAYER_FOLDER_NESTING_DEPTH + 1 {
                return true;
            }
            let metadata = self.layer_metadata(q0rg_id, id);
            if metadata.locked {
                return true;
            }
            current = metadata.parent_folder_id;
        }
        false
    }
}

fn rig_uses_pro_extensions(rig: &RigAsset) -> bool {
    rig.controls
        .iter()
        .any(|control| control.kind == RigControlKind::Toggle)
        || rig.constraints.iter().any(|constraint| {
            matches!(
                constraint,
                RigConstraint::PositionLimit { .. }
                    | RigConstraint::Aim { .. }
                    | RigConstraint::Transform { .. }
                    | RigConstraint::Distance { .. }
            )
        })
        || rig.channels.iter().any(|channel| {
            matches!(
                channel.property,
                RigPropertyRef::NodeTx(_)
                    | RigPropertyRef::NodeTy(_)
                    | RigPropertyRef::NodeScaleX(_)
                    | RigPropertyRef::NodeScaleY(_)
            )
        })
        || rig.drivers.iter().any(|driver| {
            matches!(
                driver.target,
                RigPropertyRef::NodeTx(_)
                    | RigPropertyRef::NodeTy(_)
                    | RigPropertyRef::NodeScaleX(_)
                    | RigPropertyRef::NodeScaleY(_)
            )
        })
        || rig.poses.iter().any(|pose| {
            pose.values.iter().any(|value| {
                matches!(
                    value.property,
                    RigPropertyRef::NodeTx(_)
                        | RigPropertyRef::NodeTy(_)
                        | RigPropertyRef::NodeScaleX(_)
                        | RigPropertyRef::NodeScaleY(_)
                )
            })
        })
}

fn validate_rig_asset(rig: &RigAsset) -> Result<(), Error> {
    if rig.nodes.len() > usize::from(u16::MAX)
        || rig.controls.len() > usize::from(u16::MAX)
        || rig.constraints.len() > usize::from(u16::MAX)
        || rig.channels.len() > usize::from(u16::MAX)
        || rig.drivers.len() > usize::from(u16::MAX)
        || rig.poses.len() > usize::from(u16::MAX)
        || rig.deformers.len() > usize::from(u16::MAX)
        || rig.pose_drivers.len() > usize::from(u16::MAX)
        || rig.mirror_pairs.len() > usize::from(u16::MAX)
        || rig.variants.len() > usize::from(u16::MAX)
    {
        return Err(Error::Overflow("rig collection count exceeds u16"));
    }

    let node_ids = rig
        .nodes
        .iter()
        .map(|node| node.node_id)
        .collect::<HashSet<_>>();
    if node_ids.len() != rig.nodes.len() {
        return Err(Error::Validation("rig node_id must be unique"));
    }
    let control_ids = rig
        .controls
        .iter()
        .map(|control| control.control_id)
        .collect::<HashSet<_>>();
    if control_ids.len() != rig.controls.len() {
        return Err(Error::Validation("rig control_id must be unique"));
    }
    let constraint_ids = rig
        .constraints
        .iter()
        .map(RigConstraint::id)
        .collect::<HashSet<_>>();
    if constraint_ids.len() != rig.constraints.len() {
        return Err(Error::Validation("rig constraint_id must be unique"));
    }

    let driver_ids = rig
        .drivers
        .iter()
        .map(|driver| driver.driver_id)
        .collect::<HashSet<_>>();
    if driver_ids.len() != rig.drivers.len() {
        return Err(Error::Validation("rig driver_id must be unique"));
    }
    let pose_ids = rig
        .poses
        .iter()
        .map(|pose| pose.pose_id)
        .collect::<HashSet<_>>();
    if pose_ids.len() != rig.poses.len() {
        return Err(Error::Validation("rig pose_id must be unique"));
    }

    let mut binding_ids = HashSet::new();
    for node in &rig.nodes {
        if let Some(binding) = node.binding {
            if binding.instance_id == 0 {
                return Err(Error::Validation(
                    "rig display binding instance_id must be non-zero",
                ));
            }
            if !binding_ids.insert(binding.instance_id) {
                return Err(Error::Validation(
                    "rig display binding instance_id must be unique",
                ));
            }
        }
    }
    let mut direct_drivers = HashSet::new();
    for control in &rig.controls {
        let Some(node_id) = control.target_node else {
            continue;
        };
        let kind = match control.kind {
            RigControlKind::Position2D => Some(0_u8),
            RigControlKind::Rotation => Some(1_u8),
            RigControlKind::Slider | RigControlKind::Toggle => None,
        };
        if let Some(kind) = kind {
            if !direct_drivers.insert((node_id, kind)) {
                return Err(Error::Validation(
                    "rig direct control driver must be unique",
                ));
            }
        }
    }

    for node in &rig.nodes {
        if ![
            node.rest.tx,
            node.rest.ty,
            node.rest.sx,
            node.rest.sy,
            node.rest.rotation,
            node.rest.skew_x,
            node.rest.skew_y,
            node.length,
        ]
        .into_iter()
        .all(f32::is_finite)
        {
            return Err(Error::Validation("rig node transform must be finite"));
        }
        if node.rest.sx <= 0.0 || node.rest.sy <= 0.0 || node.length < 0.0 {
            return Err(Error::Validation("rig node scale/length is invalid"));
        }
        if let Some(parent) = node.parent {
            if parent == node.node_id || !node_ids.contains(&parent) {
                return Err(Error::Validation("rig node parent is invalid"));
            }
        }
        if let Some(binding) = node.binding {
            if ![
                binding.bind_offset.a11,
                binding.bind_offset.a12,
                binding.bind_offset.a21,
                binding.bind_offset.a22,
                binding.bind_offset.tx,
                binding.bind_offset.ty,
            ]
            .into_iter()
            .all(f32::is_finite)
            {
                return Err(Error::Validation("rig binding transform must be finite"));
            }
        }
    }

    // Parent graph must be acyclic.
    for node in &rig.nodes {
        let mut current = node.parent;
        let mut visited = HashSet::new();
        while let Some(parent) = current {
            if !visited.insert(parent) || parent == node.node_id {
                return Err(Error::Validation("rig node parent cycle"));
            }
            current = rig
                .nodes
                .iter()
                .find(|candidate| candidate.node_id == parent)
                .and_then(|candidate| candidate.parent);
        }
    }

    for control in &rig.controls {
        if control
            .target_node
            .is_some_and(|node| !node_ids.contains(&node))
        {
            return Err(Error::Validation("rig control references a missing node"));
        }
        if ![
            control.rest_x,
            control.rest_y,
            control.rest_value,
            control.min_value,
            control.max_value,
        ]
        .into_iter()
        .all(f32::is_finite)
            || control.min_value > control.max_value
        {
            return Err(Error::Validation("rig control range/value is invalid"));
        }
        if control.kind == RigControlKind::Toggle
            && (control.target_node.is_some()
                || control.min_value != 0.0
                || control.max_value != 1.0
                || (control.rest_value != 0.0 && control.rest_value != 1.0))
        {
            return Err(Error::Validation("rig toggle control must use a 0/1 range"));
        }
    }

    for constraint in &rig.constraints {
        let targetless_position_control = |control_id: u16| {
            rig.controls.iter().any(|control| {
                control.control_id == control_id
                    && control.kind == RigControlKind::Position2D
                    && control.target_node.is_none()
            })
        };
        match *constraint {
            RigConstraint::RotationLimit {
                node_id,
                min_radians,
                max_radians,
                ..
            } => {
                if !node_ids.contains(&node_id)
                    || !min_radians.is_finite()
                    || !max_radians.is_finite()
                    || min_radians > max_radians
                {
                    return Err(Error::Validation("rig rotation limit is invalid"));
                }
            }
            RigConstraint::PositionLimit {
                node_id,
                min_x,
                max_x,
                min_y,
                max_y,
                ..
            } => {
                if !node_ids.contains(&node_id)
                    || ![min_x, max_x, min_y, max_y].into_iter().all(f32::is_finite)
                    || min_x > max_x
                    || min_y > max_y
                {
                    return Err(Error::Validation("rig position limit is invalid"));
                }
            }
            RigConstraint::Aim {
                node_id,
                target_control,
                angle_offset,
                weight,
                ..
            } => {
                if !node_ids.contains(&node_id)
                    || !targetless_position_control(target_control)
                    || !angle_offset.is_finite()
                    || !weight.is_finite()
                    || !(0.0..=1.0).contains(&weight)
                {
                    return Err(Error::Validation("rig aim constraint is invalid"));
                }
            }
            RigConstraint::Transform {
                node_id,
                target_node,
                position_weight,
                rotation_weight,
                ..
            } => {
                if !node_ids.contains(&node_id)
                    || !node_ids.contains(&target_node)
                    || node_id == target_node
                    || !position_weight.is_finite()
                    || !rotation_weight.is_finite()
                    || !(0.0..=1.0).contains(&position_weight)
                    || !(0.0..=1.0).contains(&rotation_weight)
                {
                    return Err(Error::Validation("rig transform constraint is invalid"));
                }
            }
            RigConstraint::Distance {
                node_id,
                target_control,
                min_distance,
                max_distance,
                weight,
                ..
            } => {
                if !node_ids.contains(&node_id)
                    || !targetless_position_control(target_control)
                    || !min_distance.is_finite()
                    || !max_distance.is_finite()
                    || min_distance < 0.0
                    || min_distance > max_distance
                    || !weight.is_finite()
                    || !(0.0..=1.0).contains(&weight)
                {
                    return Err(Error::Validation("rig distance constraint is invalid"));
                }
            }
            RigConstraint::TwoBoneIk {
                root_node,
                mid_node,
                tip_node,
                target_control,
                pole_control,
                weight,
                max_stretch,
                ..
            } => {
                if !node_ids.contains(&root_node)
                    || !node_ids.contains(&mid_node)
                    || !node_ids.contains(&tip_node)
                    || root_node == mid_node
                    || mid_node == tip_node
                    || root_node == tip_node
                    || !targetless_position_control(target_control)
                    || pole_control.is_some_and(|id| !targetless_position_control(id))
                    || !weight.is_finite()
                    || !(0.0..=1.0).contains(&weight)
                    || !max_stretch.is_finite()
                    || max_stretch < 1.0
                {
                    return Err(Error::Validation("rig two-bone ik is invalid"));
                }
                let mid = rig
                    .nodes
                    .iter()
                    .find(|node| node.node_id == mid_node)
                    .unwrap();
                let tip = rig
                    .nodes
                    .iter()
                    .find(|node| node.node_id == tip_node)
                    .unwrap();
                if mid.parent != Some(root_node) || tip.parent != Some(mid_node) {
                    return Err(Error::Validation("rig ik chain must be root -> mid -> tip"));
                }
            }
        }
    }

    // The node dependency graph combines hierarchy and transform constraints.
    // A cycle would make evaluation order-dependent, so reject it up front.
    let mut dependencies = rig
        .nodes
        .iter()
        .map(|node| {
            let mut deps = Vec::new();
            if let Some(parent) = node.parent {
                deps.push(parent);
            }
            (node.node_id, deps)
        })
        .collect::<HashMap<_, _>>();
    for constraint in &rig.constraints {
        if let RigConstraint::Transform {
            node_id,
            target_node,
            ..
        } = *constraint
        {
            dependencies.entry(node_id).or_default().push(target_node);
        }
    }
    for start_node in &rig.nodes {
        let mut stack = dependencies
            .get(&start_node.node_id)
            .cloned()
            .unwrap_or_default();
        let mut visited = HashSet::new();
        while let Some(dep) = stack.pop() {
            if dep == start_node.node_id {
                return Err(Error::Validation("rig constraint dependency cycle"));
            }
            if visited.insert(dep) {
                if let Some(next) = dependencies.get(&dep) {
                    stack.extend(next.iter().copied());
                }
            }
        }
    }

    let mut driver_targets = HashSet::new();
    for driver in &rig.drivers {
        let source_is_scalar = rig.controls.iter().any(|control| {
            control.control_id == driver.source_control
                && matches!(
                    control.kind,
                    RigControlKind::Slider | RigControlKind::Toggle
                )
        });
        if !source_is_scalar
            || ![
                driver.source_min,
                driver.source_max,
                driver.target_min,
                driver.target_max,
            ]
            .into_iter()
            .all(f32::is_finite)
            || (driver.source_max - driver.source_min).abs() < 1.0e-9
            || !driver_targets.insert(driver.target)
        {
            return Err(Error::Validation("rig driver is invalid"));
        }
        match driver.target {
            RigPropertyRef::NodeTx(id)
            | RigPropertyRef::NodeTy(id)
            | RigPropertyRef::NodeRotation(id)
            | RigPropertyRef::NodeScaleX(id)
            | RigPropertyRef::NodeScaleY(id)
                if !node_ids.contains(&id) =>
            {
                return Err(Error::Validation("rig driver references a missing node"));
            }
            RigPropertyRef::ConstraintWeight(id) if !constraint_ids.contains(&id) => {
                return Err(Error::Validation(
                    "rig driver references a missing constraint",
                ));
            }
            RigPropertyRef::ControlX(_)
            | RigPropertyRef::ControlY(_)
            | RigPropertyRef::ControlValue(_) => {
                return Err(Error::Validation(
                    "rig master drivers cannot target another control",
                ));
            }
            _ => {}
        }
    }
    for pose in &rig.poses {
        if pose.name.trim().is_empty() {
            return Err(Error::Validation("rig pose name must not be empty"));
        }
        if pose.name.len() > usize::from(u16::MAX) {
            return Err(Error::Overflow("rig pose name length exceeds u16"));
        }
        let mut properties = HashSet::new();
        for value in &pose.values {
            if !value.value.is_finite() || !properties.insert(value.property) {
                return Err(Error::Validation(
                    "rig pose values must be finite and unique",
                ));
            }
            match value.property {
                RigPropertyRef::ControlX(id)
                | RigPropertyRef::ControlY(id)
                | RigPropertyRef::ControlValue(id)
                    if !control_ids.contains(&id) =>
                {
                    return Err(Error::Validation("rig pose references a missing control"));
                }
                RigPropertyRef::NodeTx(id)
                | RigPropertyRef::NodeTy(id)
                | RigPropertyRef::NodeRotation(id)
                | RigPropertyRef::NodeScaleX(id)
                | RigPropertyRef::NodeScaleY(id)
                    if !node_ids.contains(&id) =>
                {
                    return Err(Error::Validation("rig pose references a missing node"));
                }
                RigPropertyRef::ConstraintWeight(id) if !constraint_ids.contains(&id) => {
                    return Err(Error::Validation(
                        "rig pose references a missing constraint",
                    ));
                }
                _ => {}
            }
        }
    }

    let pose_ids = rig
        .poses
        .iter()
        .map(|pose| pose.pose_id)
        .collect::<HashSet<_>>();
    if pose_ids.len() != rig.poses.len() {
        return Err(Error::Validation("rig pose_id must be unique"));
    }
    let scalar_control = |id: u16| {
        rig.controls.iter().any(|control| {
            control.control_id == id
                && matches!(
                    control.kind,
                    RigControlKind::Slider | RigControlKind::Toggle
                )
        })
    };
    let property_exists = |property: RigPropertyRef| match property {
        RigPropertyRef::NodeTx(id)
        | RigPropertyRef::NodeTy(id)
        | RigPropertyRef::NodeRotation(id)
        | RigPropertyRef::NodeScaleX(id)
        | RigPropertyRef::NodeScaleY(id) => node_ids.contains(&id),
        RigPropertyRef::ControlX(id)
        | RigPropertyRef::ControlY(id)
        | RigPropertyRef::ControlValue(id) => control_ids.contains(&id),
        RigPropertyRef::ConstraintWeight(id) => constraint_ids.contains(&id),
    };
    let mut pose_driver_ids = HashSet::new();
    for driver in &rig.pose_drivers {
        if !pose_driver_ids.insert(driver.driver_id)
            || !scalar_control(driver.source_control)
            || !pose_ids.contains(&driver.pose_id)
            || ![
                driver.source_min,
                driver.source_max,
                driver.weight_min,
                driver.weight_max,
            ]
            .into_iter()
            .all(f32::is_finite)
            || (driver.source_max - driver.source_min).abs() < 1.0e-9
            || !(0.0..=1.0).contains(&driver.weight_min)
            || !(0.0..=1.0).contains(&driver.weight_max)
        {
            return Err(Error::Validation("rig pose driver is invalid"));
        }
        let pose = rig
            .poses
            .iter()
            .find(|pose| pose.pose_id == driver.pose_id)
            .unwrap();
        if pose.values.iter().any(|value| {
            matches!(value.property, RigPropertyRef::ControlValue(id) if id == driver.source_control)
        }) {
            return Err(Error::Validation("rig pose driver cannot drive its own source control"));
        }
    }
    let mut mirrored_properties = HashSet::new();
    for pair in &rig.mirror_pairs {
        if pair.left == pair.right
            || !property_exists(pair.left)
            || !property_exists(pair.right)
            || !pair.multiplier.is_finite()
            || pair.multiplier.abs() < 1.0e-6
            || !pair.offset.is_finite()
            || !mirrored_properties.insert(pair.left)
            || !mirrored_properties.insert(pair.right)
        {
            return Err(Error::Validation(
                "rig mirror pair is invalid or overlaps another pair",
            ));
        }
    }
    let mut variant_ids = HashSet::new();
    let mut variant_instances = HashSet::new();
    for variant in &rig.variants {
        if !variant_ids.insert(variant.variant_id)
            || variant.name.trim().is_empty()
            || variant.name.len() > usize::from(u16::MAX)
            || variant.instance_id == 0
            || !variant_instances.insert(variant.instance_id)
            || !scalar_control(variant.source_control)
            || variant.choices.is_empty()
            || variant.choices.len() > usize::from(u16::MAX)
        {
            return Err(Error::Validation("rig variant set is invalid"));
        }
        for choice in &variant.choices {
            if choice.name.trim().is_empty() || choice.name.len() > usize::from(u16::MAX) {
                return Err(Error::Validation("rig variant choice name is invalid"));
            }
        }
    }

    let deformer_ids = rig
        .deformers
        .iter()
        .map(RigDeformer::id)
        .collect::<HashSet<_>>();
    if deformer_ids.len() != rig.deformers.len() {
        return Err(Error::Validation("rig deformer_id must be unique"));
    }
    let mut deformed_instances = HashSet::new();
    let affine_valid = |value: Affine| {
        [
            value.a11, value.a12, value.a21, value.a22, value.tx, value.ty,
        ]
        .into_iter()
        .all(f32::is_finite)
            && value.inverse().is_some()
    };
    let position_control = |id: u16| {
        rig.controls.iter().any(|control| {
            control.control_id == id
                && control.kind == RigControlKind::Position2D
                && control.target_node.is_none()
        })
    };
    for deformer in &rig.deformers {
        if deformer.instance_id() == 0
            || binding_ids.contains(&deformer.instance_id())
            || !deformed_instances.insert(deformer.instance_id())
        {
            return Err(Error::Validation(
                "rig deformer instance_id must be non-zero, unique and not rigid-bound",
            ));
        }
        match deformer {
            RigDeformer::Skin {
                bind_transform,
                bones,
                anchors,
                ..
            } => {
                if !affine_valid(*bind_transform) || bones.is_empty() || anchors.is_empty() {
                    return Err(Error::Validation("rig skin deformer bind data is invalid"));
                }
                let mut skin_nodes = HashSet::new();
                for bone in bones {
                    if !node_ids.contains(&bone.node_id)
                        || !skin_nodes.insert(bone.node_id)
                        || !affine_valid(bone.inverse_rest_world)
                    {
                        return Err(Error::Validation("rig skin bone bind is invalid"));
                    }
                }
                let mut anchor_keys = HashSet::new();
                for anchor in anchors {
                    if !anchor_keys.insert((anchor.path_index, anchor.anchor_index))
                        || anchor.weights.is_empty()
                        || anchor.weights.len() > 4
                    {
                        return Err(Error::Validation("rig skin anchor weights are invalid"));
                    }
                    let mut weighted_nodes = HashSet::new();
                    let mut sum = 0.0_f32;
                    for weight in &anchor.weights {
                        if !skin_nodes.contains(&weight.node_id)
                            || !weighted_nodes.insert(weight.node_id)
                            || !weight.weight.is_finite()
                            || weight.weight <= 0.0
                        {
                            return Err(Error::Validation("rig skin weight is invalid"));
                        }
                        sum += weight.weight;
                    }
                    if !sum.is_finite() || (sum - 1.0).abs() > 1.0e-3 {
                        return Err(Error::Validation("rig skin weights must normalize to one"));
                    }
                }
            }
            RigDeformer::Bend {
                bind_transform,
                axis_start,
                axis_end,
                start_control,
                middle_control,
                end_control,
                ..
            } => {
                let dx = axis_end.x - axis_start.x;
                let dy = axis_end.y - axis_start.y;
                if !affine_valid(*bind_transform)
                    || ![axis_start.x, axis_start.y, axis_end.x, axis_end.y]
                        .into_iter()
                        .all(f32::is_finite)
                    || dx * dx + dy * dy < 1.0e-8
                    || !position_control(*start_control)
                    || !position_control(*middle_control)
                    || !position_control(*end_control)
                    || start_control == middle_control
                    || middle_control == end_control
                    || start_control == end_control
                {
                    return Err(Error::Validation("rig bend deformer is invalid"));
                }
            }
            RigDeformer::Cage {
                bind_transform,
                rest_min,
                rest_max,
                controls,
                ..
            } => {
                let unique = controls.iter().copied().collect::<HashSet<_>>();
                if !affine_valid(*bind_transform)
                    || ![rest_min.x, rest_min.y, rest_max.x, rest_max.y]
                        .into_iter()
                        .all(f32::is_finite)
                    || rest_max.x - rest_min.x <= 1.0e-5
                    || rest_max.y - rest_min.y <= 1.0e-5
                    || unique.len() != 4
                    || controls.iter().any(|id| !position_control(*id))
                {
                    return Err(Error::Validation("rig cage deformer is invalid"));
                }
            }
        }
    }

    if rig
        .variants
        .iter()
        .any(|variant| deformed_instances.contains(&variant.instance_id))
    {
        return Err(Error::Validation(
            "rig variant cannot share an instance with a vector deformer",
        ));
    }
    for variant in &rig.variants {
        if let Some(control) = rig
            .controls
            .iter()
            .find(|control| control.control_id == variant.source_control)
        {
            if control.kind == RigControlKind::Toggle && variant.choices.len() > 2 {
                return Err(Error::Validation(
                    "rig toggle variant cannot have more than two choices",
                ));
            }
        }
    }

    let mut channel_props = HashSet::new();
    for channel in &rig.channels {
        if !channel_props.insert(channel.property) {
            return Err(Error::Validation("rig channel property must be unique"));
        }
        match channel.property {
            RigPropertyRef::ControlX(id)
            | RigPropertyRef::ControlY(id)
            | RigPropertyRef::ControlValue(id)
                if !control_ids.contains(&id) =>
            {
                return Err(Error::Validation(
                    "rig channel references a missing control",
                ));
            }
            RigPropertyRef::NodeTx(id)
            | RigPropertyRef::NodeTy(id)
            | RigPropertyRef::NodeRotation(id)
            | RigPropertyRef::NodeScaleX(id)
            | RigPropertyRef::NodeScaleY(id)
                if !node_ids.contains(&id) =>
            {
                return Err(Error::Validation("rig channel references a missing node"));
            }
            RigPropertyRef::ConstraintWeight(id) if !constraint_ids.contains(&id) => {
                return Err(Error::Validation(
                    "rig channel references a missing constraint",
                ));
            }
            _ => {}
        }
        let mut frames = HashSet::new();
        for key in &channel.keys {
            if !frames.insert(key.frame) || !key.value.is_finite() || !key.easing.is_valid() {
                return Err(Error::Validation("rig key is invalid or duplicated"));
            }
        }
    }
    Ok(())
}

const MAX_PROJECT_GRAPH_DEPTH: usize = 64;
const MAX_EMBEDDED_DEPENDENCY_BYTES: usize = 256 * 1024 * 1024;

fn runtime_alias_is_valid(alias: &str) -> bool {
    let mut chars = alias.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn validate_project_runtime(project: &ProjectV2) -> Result<(), Error> {
    let nodes = &project.runtime.project_graph.nodes;
    let mut node_ids = HashSet::new();
    let mut parent_by_id = HashMap::new();
    let mut sibling_aliases = HashSet::new();
    for node in nodes {
        if node.node_id == 0 || !node_ids.insert(node.node_id) {
            return Err(Error::Validation(
                "project dependency node ids must be unique and non-zero",
            ));
        }
        if !runtime_alias_is_valid(&node.alias) {
            return Err(Error::Validation(
                "project dependency alias must be an identifier",
            ));
        }
        if !sibling_aliases.insert((node.parent_node_id, node.alias.clone())) {
            return Err(Error::Validation(
                "project dependency aliases must be unique among siblings",
            ));
        }
        match &node.source {
            ProjectDependencySource::External(path) if path.trim().is_empty() => {
                return Err(Error::Validation(
                    "external project dependency path cannot be empty",
                ));
            }
            ProjectDependencySource::Embedded(bytes)
                if bytes.len() > MAX_EMBEDDED_DEPENDENCY_BYTES =>
            {
                return Err(Error::Validation(
                    "embedded project dependency is too large",
                ));
            }
            _ => {}
        }
        parent_by_id.insert(node.node_id, node.parent_node_id);
    }
    for node in nodes {
        if let Some(parent) = node.parent_node_id {
            if parent == node.node_id || !node_ids.contains(&parent) {
                return Err(Error::Validation(
                    "project dependency parent references a missing/self node",
                ));
            }
        }
        let mut seen = HashSet::new();
        let mut cursor = Some(node.node_id);
        let mut depth = 0usize;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(Error::Validation("project dependency cycle detected"));
            }
            depth += 1;
            if depth > MAX_PROJECT_GRAPH_DEPTH {
                return Err(Error::Validation("project dependency nesting is too deep"));
            }
            cursor = parent_by_id.get(&id).copied().flatten();
        }
    }

    let mut frame_script_keys = HashSet::new();
    for script in &project.runtime.frame_scripts {
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == script.q0rg_id)
            .ok_or(Error::Validation("frame script references a missing q0rg"))?;
        if script.frame >= q0rg.frame_count {
            return Err(Error::Validation(
                "frame script is outside the q0rg timeline",
            ));
        }
        if !q0rg
            .layers
            .iter()
            .any(|layer| layer.layer_id == script.layer_id)
        {
            return Err(Error::Validation("frame script references a missing layer"));
        }
        if project.layer_is_folder(script.q0rg_id, script.layer_id) {
            return Err(Error::Validation(
                "frame script cannot live on a folder row",
            ));
        }
        if !frame_script_keys.insert((script.q0rg_id, script.layer_id, script.frame)) {
            return Err(Error::Validation(
                "only one frame script is allowed per timeline cell",
            ));
        }
        if script.source.len() > usize::from(u16::MAX) {
            return Err(Error::Overflow("frame script exceeds u16 source length"));
        }
    }

    let mut names_by_q0rg = HashSet::new();
    for (key, name) in &project.runtime.instance_names {
        if key.instance_id == 0 || name.trim().is_empty() {
            return Err(Error::Validation(
                "runtime instance names require a non-zero instance id and non-empty name",
            ));
        }
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == key.q0rg_id)
            .ok_or(Error::Validation(
                "runtime instance name references a missing q0rg",
            ))?;
        if !q0rg
            .layers
            .iter()
            .flat_map(|layer| &layer.placements)
            .any(|placement| placement.instance_id == key.instance_id)
        {
            return Err(Error::Validation(
                "runtime instance name references a missing placement identity",
            ));
        }
        if !names_by_q0rg.insert((key.q0rg_id, name.clone())) {
            return Err(Error::Validation(
                "runtime instance names must be unique within a q0rg",
            ));
        }
    }
    Ok(())
}

pub fn validate(project: &ProjectV2) -> Result<(), Error> {
    if project.meta.fps == 0 {
        return Err(Error::Validation("fps must be > 0"));
    }
    if project.meta.stage_width == 0 || project.meta.stage_height == 0 {
        return Err(Error::Validation("stage size must be > 0"));
    }
    if project.q0rgs.is_empty() {
        return Err(Error::Validation("at least one q0rg is required"));
    }

    validate_project_runtime(project)?;

    let mut asset_ids = HashSet::new();
    for asset in &project.assets {
        if !asset_ids.insert(asset.id()) {
            return Err(Error::Validation("asset_id must be unique"));
        }
        match asset {
            Asset::Bitmap(b) => {
                let expected = usize::from(b.width)
                    .saturating_mul(usize::from(b.height))
                    .saturating_mul(4);
                if b.rgba.len() != expected {
                    return Err(Error::Validation(
                        "bitmap rgba length must match width * height * 4",
                    ));
                }
            }
            Asset::Vector(v) => {
                for path in &v.paths {
                    if path.anchors.is_empty() {
                        return Err(Error::Validation("vector path must have >= 1 anchor"));
                    }
                    for anchor in &path.anchors {
                        if !anchor.point.is_finite() {
                            return Err(Error::Validation("vector anchor point must be finite"));
                        }
                        if anchor
                            .in_handle
                            .into_iter()
                            .chain(anchor.out_handle)
                            .any(|handle| !handle.is_finite())
                        {
                            return Err(Error::Validation("vector anchor handle must be finite"));
                        }
                    }
                }
                if let Some(stroke) = &v.stroke {
                    if !stroke.width.is_finite() {
                        return Err(Error::Validation("stroke width must be finite"));
                    }
                    if stroke.width <= 0.0 {
                        return Err(Error::Validation("stroke width must be > 0"));
                    }
                }
            }
            Asset::Q0v(v) => {
                q0video::q0v::validate_bytes(&v.bytes)
                    .map_err(|_| Error::Validation("q0v asset payload is invalid"))?;
            }
            Asset::Rig(rig) => validate_rig_asset(rig)?,
        }
    }

    for rig in project.assets.iter().filter_map(|asset| match asset {
        Asset::Rig(rig) => Some(rig),
        _ => None,
    }) {
        let owner = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == rig.owner_q0rg_id)
            .ok_or(Error::Validation("rig owner q0rg is missing"))?;
        for deformer in &rig.deformers {
            let Some(Asset::Vector(vector)) = project
                .assets
                .iter()
                .find(|asset| asset.id() == deformer.asset_id())
            else {
                return Err(Error::Validation(
                    "rig deformer must reference a vector asset",
                ));
            };
            let placement_exists =
                owner
                    .layers
                    .iter()
                    .flat_map(|layer| &layer.placements)
                    .any(|placement| {
                        placement.instance_id == deformer.instance_id()
                            && placement.target == Target::Asset(deformer.asset_id())
                    });
            if !placement_exists {
                return Err(Error::Validation(
                    "rig deformer references a missing vector instance",
                ));
            }
            if let RigDeformer::Skin { anchors, .. } = deformer {
                for anchor in anchors {
                    let Some(path) = vector.paths.get(usize::from(anchor.path_index)) else {
                        return Err(Error::Validation("rig skin path index is out of bounds"));
                    };
                    if usize::from(anchor.anchor_index) >= path.anchors.len() {
                        return Err(Error::Validation("rig skin anchor index is out of bounds"));
                    }
                }
            }
        }
    }

    for (&asset_id, name) in &project.asset_names {
        if !asset_ids.contains(&asset_id) {
            return Err(Error::Validation("asset name references a missing asset"));
        }
        if name.trim().is_empty() {
            return Err(Error::Validation("asset name must not be empty"));
        }
        if name.len() > usize::from(u16::MAX) {
            return Err(Error::Overflow("asset name length exceeds u16"));
        }
    }

    for (&asset_id, appearance) in &project.asset_appearances {
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            return Err(Error::Validation(
                "vector appearance references a missing or non-vector asset",
            ));
        };
        if vector.fill.is_none() || vector.stroke.is_some() {
            return Err(Error::Validation(
                "vector appearance requires a fill-only vector asset",
            ));
        }
        let field = appearance.field_transform;
        if ![
            field.a11, field.a12, field.a21, field.a22, field.tx, field.ty,
        ]
        .into_iter()
        .all(f32::is_finite)
        {
            return Err(Error::Validation(
                "appearance field transform must be finite",
            ));
        }
        let determinant = field.a11 * field.a22 - field.a12 * field.a21;
        if determinant.abs() < 1.0e-9 {
            return Err(Error::Validation(
                "appearance field transform must be invertible",
            ));
        }
        if field != Affine::IDENTITY && appearance.material_source.is_empty() {
            return Err(Error::Validation(
                "transformed appearance field requires a frozen material source",
            ));
        }
        match appearance.material {
            VectorMaterial::Solid => {}
            VectorMaterial::SoftHalo { radius, opacity } => {
                if !radius.is_finite() || radius <= 0.0 {
                    return Err(Error::Validation("soft halo radius must be finite and > 0"));
                }
                if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
                    return Err(Error::Validation(
                        "soft halo opacity must be finite and between 0 and 1",
                    ));
                }
            }
        }
        for paths in [
            &appearance.erase_mask,
            &appearance.material_source,
            &appearance.clip_mask,
        ] {
            for path in paths {
                if !path.closed || path.anchors.len() < 3 {
                    return Err(Error::Validation(
                        "appearance paths must be closed with >= 3 anchors",
                    ));
                }
                for anchor in &path.anchors {
                    if !anchor.point.is_finite()
                        || anchor
                            .in_handle
                            .into_iter()
                            .chain(anchor.out_handle)
                            .any(|handle| !handle.is_finite())
                    {
                        return Err(Error::Validation("appearance anchors must be finite"));
                    }
                }
            }
        }
    }

    let mut q0rg_ids = HashSet::new();
    let mut layer_ids_by_q0rg: HashMap<u16, HashSet<u16>> = HashMap::new();
    for q0rg in &project.q0rgs {
        if !q0rg_ids.insert(q0rg.q0rg_id) {
            return Err(Error::Validation("q0rg_id must be unique"));
        }
        if q0rg.frame_count == 0 {
            return Err(Error::Validation("q0rg frame_count must be > 0"));
        }
        let mut layer_ids = HashSet::new();
        let mut instance_layers = HashMap::<u32, u16>::new();
        for layer in &q0rg.layers {
            if !layer_ids.insert(layer.layer_id) {
                return Err(Error::Validation("layer_id must be unique within q0rg"));
            }
            let mut explicit_keyframes = HashSet::new();
            for &frame in &layer.explicit_keyframes {
                if frame >= q0rg.frame_count {
                    return Err(Error::Validation(
                        "explicit keyframe is out of q0rg frame_count bounds",
                    ));
                }
                if !explicit_keyframes.insert(frame) {
                    return Err(Error::Validation(
                        "explicit keyframe frames must be unique within layer",
                    ));
                }
            }
            let mut instance_frames = HashSet::<(u32, u16)>::new();
            for p in &layer.placements {
                if p.instance_id != 0 {
                    if let Some(previous_layer) =
                        instance_layers.insert(p.instance_id, layer.layer_id)
                    {
                        if previous_layer != layer.layer_id {
                            return Err(Error::Validation(
                                "placement instance_id must stay within one layer",
                            ));
                        }
                    }
                    if !instance_frames.insert((p.instance_id, p.frame)) {
                        return Err(Error::Validation(
                            "placement instance_id must be unique on a layer keyframe",
                        ));
                    }
                }
                if p.frame >= q0rg.frame_count {
                    return Err(Error::Validation(
                        "placement frame is out of q0rg frame_count bounds",
                    ));
                }
                if !p.transform.tx.is_finite() || !p.transform.ty.is_finite() {
                    return Err(Error::Validation("placement translation must be finite"));
                }
                if !p.transform.sx.is_finite() || !p.transform.sy.is_finite() {
                    return Err(Error::Validation("placement scale must be finite"));
                }
                if p.transform.sx <= 0.0 || p.transform.sy <= 0.0 {
                    return Err(Error::Validation("placement scale must be positive"));
                }
                if !p.transform.rotation.is_finite() {
                    return Err(Error::Validation("placement rotation must be finite"));
                }
                if !p.transform.skew_x.is_finite() || !p.transform.skew_y.is_finite() {
                    return Err(Error::Validation("placement skew must be finite"));
                }
                if !p.fx.opacity.is_finite() || !(0.0..=1.0).contains(&p.fx.opacity) {
                    return Err(Error::Validation(
                        "placement opacity must be finite and between 0 and 1",
                    ));
                }
                if !p.fx.audio_gain.is_finite() || !(0.0..=4.0).contains(&p.fx.audio_gain) {
                    return Err(Error::Validation(
                        "placement audio gain must be finite and between 0 and 4",
                    ));
                }
                if let Some(blur) = p.fx.blur {
                    if !blur.radius.is_finite() || !(0.0..=512.0).contains(&blur.radius) {
                        return Err(Error::Validation(
                            "placement blur radius must be finite and between 0 and 512",
                        ));
                    }
                }
                if let Some(glow) = p.fx.glow {
                    if !glow.radius.is_finite() || !(0.0..=512.0).contains(&glow.radius) {
                        return Err(Error::Validation(
                            "placement glow radius must be finite and between 0 and 512",
                        ));
                    }
                    if !glow.strength.is_finite() || !(0.0..=4.0).contains(&glow.strength) {
                        return Err(Error::Validation(
                            "placement glow strength must be finite and between 0 and 4",
                        ));
                    }
                }
                if let Some(shadow) = p.fx.shadow {
                    if !shadow.blur_radius.is_finite()
                        || !(0.0..=512.0).contains(&shadow.blur_radius)
                    {
                        return Err(Error::Validation(
                            "placement shadow blur must be finite and between 0 and 512",
                        ));
                    }
                    if !shadow.offset_x.is_finite() || !shadow.offset_y.is_finite() {
                        return Err(Error::Validation("placement shadow offset must be finite"));
                    }
                    if !shadow.strength.is_finite() || !(0.0..=4.0).contains(&shadow.strength) {
                        return Err(Error::Validation(
                            "placement shadow strength must be finite and between 0 and 4",
                        ));
                    }
                }
                match p.target {
                    Target::Asset(id) => {
                        if !asset_ids.contains(&id) {
                            return Err(Error::Validation("placement references unknown asset_id"));
                        }
                        if project
                            .assets
                            .iter()
                            .any(|asset| matches!(asset, Asset::Rig(rig) if rig.asset_id == id))
                        {
                            return Err(Error::Validation("placement cannot target rig metadata"));
                        }
                    }
                    Target::Q0rg(id) => {
                        if id == q0rg.q0rg_id {
                            return Err(Error::Validation("q0rg cannot reference itself"));
                        }
                    }
                }
                if let Some(to_frame) = p.tween.to_frame() {
                    if to_frame <= p.frame {
                        return Err(Error::Validation("tween to_frame must be > frame"));
                    }
                    if to_frame >= q0rg.frame_count {
                        return Err(Error::Validation(
                            "tween to_frame is out of q0rg frame_count bounds",
                        ));
                    }
                    if !p.tween.easing().is_valid() {
                        return Err(Error::Validation("invalid tween easing curve"));
                    }
                }
            }
        }
        layer_ids_by_q0rg.insert(q0rg.q0rg_id, layer_ids);
    }

    let mut audio_ranges = HashMap::<(u16, u16), Vec<(u16, u16)>>::new();
    for &clip in &project.audio_clips {
        if !clip.gain.is_finite() || !(0.0..=4.0).contains(&clip.gain) {
            return Err(Error::Validation(
                "audio clip gain must be finite and between 0 and 4",
            ));
        }
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == clip.q0rg_id)
            .ok_or(Error::Validation("audio clip references a missing q0rg"))?;
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == clip.layer_id)
            .ok_or(Error::Validation("audio clip references a missing layer"))?;
        if project.layer_is_folder(clip.q0rg_id, clip.layer_id) {
            return Err(Error::Validation("audio clip cannot live on a folder row"));
        }
        if clip.start_frame >= q0rg.frame_count {
            return Err(Error::Validation(
                "audio clip start is outside the timeline",
            ));
        }
        let Some(end_frame) = audio_clip_end_frame(project, clip) else {
            return Err(Error::Validation(
                "audio clip must reference an audio-only q0v asset",
            ));
        };
        if end_frame > q0rg.frame_count {
            return Err(Error::Validation("audio clip extends past the timeline"));
        }
        if layer
            .keyframe_frames()
            .into_iter()
            .any(|frame| (clip.start_frame..end_frame).contains(&frame))
        {
            return Err(Error::Validation(
                "audio clip frames cannot contain visual keyframes",
            ));
        }
        if let Some(previous_key) = layer
            .keyframe_frames()
            .into_iter()
            .filter(|frame| *frame < clip.start_frame)
            .max()
        {
            if layer
                .placements
                .iter()
                .any(|placement| placement.frame == previous_key)
            {
                return Err(Error::Validation(
                    "audio clip frames cannot contain held visual content",
                ));
            }
        }
        let ranges = audio_ranges
            .entry((clip.q0rg_id, clip.layer_id))
            .or_default();
        if ranges
            .iter()
            .any(|&(start, end)| start < end_frame && clip.start_frame < end)
        {
            return Err(Error::Validation(
                "audio clips on one layer must not overlap",
            ));
        }
        ranges.push((clip.start_frame, end_frame));
    }

    for (&key, &metadata) in &project.layer_metadata {
        let Some(layer_ids) = layer_ids_by_q0rg.get(&key.q0rg_id) else {
            return Err(Error::Validation(
                "layer metadata references a missing q0rg",
            ));
        };
        if !layer_ids.contains(&key.layer_id) {
            return Err(Error::Validation(
                "layer metadata references a missing layer",
            ));
        }
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == key.q0rg_id)
            .expect("validated q0rg metadata key");
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == key.layer_id)
            .expect("validated layer metadata key");
        match metadata.kind {
            LayerKind::Folder => {
                if !layer.explicit_keyframes.is_empty() || !layer.placements.is_empty() {
                    return Err(Error::Validation("layer folder must not contain keyframes"));
                }
            }
            LayerKind::Normal => {
                if metadata.collapsed {
                    return Err(Error::Validation("ordinary layer cannot be collapsed"));
                }
            }
        }
        if let Some(parent_id) = metadata.parent_folder_id {
            let parent = project.layer_metadata(key.q0rg_id, parent_id);
            if parent.kind != LayerKind::Folder {
                return Err(Error::Validation("layer parent must reference a folder"));
            }
        }
    }

    for q0rg in &project.q0rgs {
        let index_by_id = q0rg
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| (layer.layer_id, index))
            .collect::<HashMap<_, _>>();

        for (index, layer) in q0rg.layers.iter().enumerate() {
            let mut current_id = layer.layer_id;
            let mut visited = HashSet::new();
            let mut depth = 0usize;
            while let Some(parent_id) = project.layer_parent_folder(q0rg.q0rg_id, current_id) {
                if !visited.insert(parent_id) || parent_id == layer.layer_id {
                    return Err(Error::Validation("layer folder parent cycle"));
                }
                depth += 1;
                if depth > MAX_LAYER_FOLDER_NESTING_DEPTH {
                    return Err(Error::Validation("layer folder nesting depth exceeds 64"));
                }
                let Some(&parent_index) = index_by_id.get(&parent_id) else {
                    return Err(Error::Validation(
                        "layer parent references a missing folder",
                    ));
                };
                if parent_index <= index {
                    return Err(Error::Validation(
                        "folder child layers must immediately precede their folder",
                    ));
                }
                current_id = parent_id;
            }
        }

        for (folder_index, folder) in q0rg
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| project.layer_is_folder(q0rg.q0rg_id, layer.layer_id))
        {
            let descendant_indices = q0rg
                .layers
                .iter()
                .enumerate()
                .filter(|(_, layer)| {
                    project.layer_is_descendant_of(q0rg.q0rg_id, layer.layer_id, folder.layer_id)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if descendant_indices.len() > folder_index {
                return Err(Error::Validation(
                    "folder child layers must immediately precede their folder",
                ));
            }
            let expected_start = folder_index - descendant_indices.len();
            if descendant_indices
                .iter()
                .copied()
                .ne(expected_start..folder_index)
            {
                return Err(Error::Validation(
                    "folder child layers must immediately precede their folder",
                ));
            }
        }
    }

    let mut rig_owners = HashSet::new();
    for asset in &project.assets {
        if let Asset::Rig(rig) = asset {
            if !rig_owners.insert(rig.owner_q0rg_id) {
                return Err(Error::Validation("only one rig is allowed per q0rg"));
            }
            if !q0rg_ids.contains(&rig.owner_q0rg_id) {
                return Err(Error::Validation("rig references a missing owner q0rg"));
            }
            let owner = project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == rig.owner_q0rg_id)
                .expect("validated rig owner");
            for channel in &rig.channels {
                if channel
                    .keys
                    .iter()
                    .any(|key| key.frame >= owner.frame_count)
                {
                    return Err(Error::Validation(
                        "rig key is out of owner q0rg frame bounds",
                    ));
                }
            }
            let owner_instance_ids = owner
                .layers
                .iter()
                .flat_map(|layer| &layer.placements)
                .filter_map(|placement| {
                    (placement.instance_id != 0).then_some(placement.instance_id)
                })
                .collect::<HashSet<_>>();
            for node in &rig.nodes {
                if let Some(binding) = node.binding {
                    if !owner_instance_ids.contains(&binding.instance_id) {
                        return Err(Error::Validation(
                            "rig binding references a missing placement instance_id",
                        ));
                    }
                }
            }
            for variant in &rig.variants {
                if !owner_instance_ids.contains(&variant.instance_id) {
                    return Err(Error::Validation(
                        "rig variant references a missing placement instance_id",
                    ));
                }
                for choice in &variant.choices {
                    match choice.target {
                        Target::Asset(id) => {
                            let Some(asset) = project.assets.iter().find(|asset| asset.id() == id)
                            else {
                                return Err(Error::Validation(
                                    "rig variant references a missing asset",
                                ));
                            };
                            if matches!(asset, Asset::Rig(_)) {
                                return Err(Error::Validation(
                                    "rig variant cannot target rig metadata",
                                ));
                            }
                        }
                        Target::Q0rg(id) => {
                            if id == rig.owner_q0rg_id || !q0rg_ids.contains(&id) {
                                return Err(Error::Validation(
                                    "rig variant q0rg target is invalid",
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    if !q0rg_ids.contains(&project.meta.entry_q0rg_id) {
        return Err(Error::Validation(
            "entry_q0rg_id must reference an existing q0rg",
        ));
    }

    // q0rg target ids exist + stack-safe cycle/depth validation.
    let mut q0rg_index: HashMap<u16, usize> = HashMap::with_capacity(project.q0rgs.len());
    for (i, q) in project.q0rgs.iter().enumerate() {
        q0rg_index.insert(q.q0rg_id, i);
    }
    validate_q0rg_graph(project, &q0rg_index)?;

    Ok(())
}

fn validate_q0rg_graph(project: &ProjectV2, index: &HashMap<u16, usize>) -> Result<(), Error> {
    let q0rgs = &project.q0rgs;
    let mut children = vec![Vec::<usize>::new(); q0rgs.len()];
    let mut indegree = vec![0usize; q0rgs.len()];

    for (parent, q0rg) in q0rgs.iter().enumerate() {
        for layer in &q0rg.layers {
            for placement in &layer.placements {
                if let Target::Q0rg(id) = placement.target {
                    let child = index
                        .get(&id)
                        .copied()
                        .ok_or(Error::Validation("placement references unknown q0rg_id"))?;
                    children[parent].push(child);
                    indegree[child] = indegree[child].saturating_add(1);
                }
            }
        }
    }
    for rig in project.assets.iter().filter_map(|asset| match asset {
        Asset::Rig(rig) => Some(rig),
        _ => None,
    }) {
        let parent = index
            .get(&rig.owner_q0rg_id)
            .copied()
            .ok_or(Error::Validation("rig owner q0rg is missing"))?;
        for variant in &rig.variants {
            for choice in &variant.choices {
                if let Target::Q0rg(id) = choice.target {
                    let child = index
                        .get(&id)
                        .copied()
                        .ok_or(Error::Validation("rig variant references unknown q0rg_id"))?;
                    children[parent].push(child);
                    indegree[child] = indegree[child].saturating_add(1);
                }
            }
        }
    }

    // Kahn's algorithm detects cycles without recursing through attacker-owned
    // graph depth and also gives us the order needed for longest-path depth.
    let mut ready = VecDeque::new();
    for (node, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            ready.push_back(node);
        }
    }

    let mut order = Vec::with_capacity(q0rgs.len());
    while let Some(node) = ready.pop_front() {
        order.push(node);
        for &child in &children[node] {
            indegree[child] -= 1;
            if indegree[child] == 0 {
                ready.push_back(child);
            }
        }
    }
    if order.len() != q0rgs.len() {
        return Err(Error::Validation("q0rg cycle detected"));
    }

    let mut depth = vec![0usize; q0rgs.len()];
    for node in order {
        for &child in &children[node] {
            let child_depth = depth[node] + 1;
            if child_depth > usize::from(MAX_Q0RG_NESTING_DEPTH) {
                return Err(Error::Validation("q0rg nesting depth exceeds 8"));
            }
            depth[child] = depth[child].max(child_depth);
        }
    }

    Ok(())
}

pub fn write(project: &ProjectV2) -> Result<Vec<u8>, Error> {
    write_version(project, Q1S_VERSION_CURRENT)
}

pub(crate) fn write_version(project: &ProjectV2, version: u16) -> Result<Vec<u8>, Error> {
    validate(project)?;
    if version != Q1S_VERSION_LEGACY
        && version != Q1S_VERSION_SKEW
        && version != Q1S_VERSION_KEYFRAMES
        && version != Q1S_VERSION_ASSET_NAMES
        && version != Q1S_VERSION_LAYER_FOLDERS
        && version != Q1S_VERSION_Q0V_ASSETS
        && version != Q1S_VERSION_EASING
        && version != Q1S_VERSION_APPEARANCE_MASKS
        && version != Q1S_VERSION_APPEARANCE_FRAGMENTS
        && version != Q1S_VERSION_APPEARANCE_AFFINE
        && version != Q1S_VERSION_LAYER_STATE
        && version != Q1S_VERSION_NESTED_LAYER_FOLDERS
        && version != Q1S_VERSION_PLACEMENT_FX
        && version != Q1S_VERSION_RIGGING
        && version != Q1S_VERSION_RIG_PRO
        && version != Q1S_VERSION_RIG_DEFORMERS
        && version != Q1S_VERSION_RIG_POSE_VARIANTS
        && version != Q1S_VERSION_AUDIO_CLIP_FX
        && version != Q1S_VERSION_AUDIO_TIMELINE_CLIPS
        && version != Q1S_VERSION_CURRENT
    {
        return Err(Error::UnsupportedVersion(version));
    }
    if version < Q1S_VERSION_LAYER_FOLDERS && !project.layer_metadata.is_empty() {
        return Err(Error::Validation(
            "legacy q1s versions cannot store layer folders",
        ));
    }
    if version < Q1S_VERSION_NESTED_LAYER_FOLDERS
        && project.layer_metadata.values().any(|metadata| {
            metadata.kind == LayerKind::Folder && metadata.parent_folder_id.is_some()
        })
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store nested layer folders",
        ));
    }
    if version < Q1S_VERSION_LAYER_STATE
        && project
            .layer_metadata
            .values()
            .any(|metadata| metadata.hidden || metadata.locked)
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store layer visibility or locks",
        ));
    }
    if version < Q1S_VERSION_PLACEMENT_FX
        && project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .any(|placement| !placement.fx.is_identity())
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store placement effects",
        ));
    }
    if version < Q1S_VERSION_AUDIO_CLIP_FX
        && project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .any(|placement| {
                (placement.fx.audio_gain - 1.0).abs() > 1.0e-6 || placement.fx.audio_muted
            })
    {
        return Err(Error::Validation(
            "q1s v18 cannot store audio clip gain or mute",
        ));
    }
    if version < Q1S_VERSION_AUDIO_TIMELINE_CLIPS && !project.audio_clips.is_empty() {
        return Err(Error::Validation(
            "q1s v19 cannot store timeline audio clips",
        ));
    }
    if version < Q1S_VERSION_PROJECT_RUNTIME && project.runtime != ProjectRuntimeData::default() {
        return Err(Error::Validation(
            "q1s v20 cannot store project runtime metadata",
        ));
    }
    if version < Q1S_VERSION_RIGGING
        && project
            .assets
            .iter()
            .any(|asset| matches!(asset, Asset::Rig(_)))
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store rigging",
        ));
    }
    if version < Q1S_VERSION_RIG_PRO
        && project.assets.iter().any(|asset| match asset {
            Asset::Rig(rig) => rig_uses_pro_extensions(rig),
            _ => false,
        })
    {
        return Err(Error::Validation("q1s v15 cannot store pro rig extensions"));
    }
    if version < Q1S_VERSION_RIG_DEFORMERS
        && project.assets.iter().any(|asset| match asset {
            Asset::Rig(rig) => !rig.deformers.is_empty(),
            _ => false,
        })
    {
        return Err(Error::Validation("q1s v16 cannot store rig deformers"));
    }
    if version < Q1S_VERSION_RIG_POSE_VARIANTS
        && project.assets.iter().any(|asset| match asset {
            Asset::Rig(rig) => {
                !rig.pose_drivers.is_empty()
                    || !rig.mirror_pairs.is_empty()
                    || !rig.variants.is_empty()
            }
            _ => false,
        })
    {
        return Err(Error::Validation(
            "q1s v17 cannot store pose drivers, mirror pairs or variants",
        ));
    }
    if version < Q1S_VERSION_RIGGING
        && project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .any(|placement| placement.instance_id != 0)
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store placement instance identities",
        ));
    }
    if version < Q1S_VERSION_KEYFRAMES
        && project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .any(|layer| !layer.explicit_keyframes.is_empty())
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store explicit keyframes",
        ));
    }
    if version < Q1S_VERSION_ASSET_NAMES && !project.asset_names.is_empty() {
        return Err(Error::Validation(
            "legacy q1s versions cannot store asset names",
        ));
    }
    if version < Q1S_VERSION_Q0V_ASSETS
        && project
            .assets
            .iter()
            .any(|asset| matches!(asset, Asset::Q0v(_)))
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store q0v assets",
        ));
    }
    if version < Q1S_VERSION_APPEARANCE_MASKS && !project.asset_appearances.is_empty() {
        return Err(Error::Validation(
            "legacy q1s versions cannot store vector appearance masks",
        ));
    }
    if version < Q1S_VERSION_APPEARANCE_FRAGMENTS
        && project.asset_appearances.values().any(|appearance| {
            !appearance.material_source.is_empty() || !appearance.clip_mask.is_empty()
        })
    {
        return Err(Error::Validation(
            "q1s v9 cannot store post-material appearance fragments",
        ));
    }
    if version < Q1S_VERSION_APPEARANCE_AFFINE
        && project
            .asset_appearances
            .values()
            .any(|appearance| appearance.field_transform != Affine::IDENTITY)
    {
        return Err(Error::Validation(
            "q1s v10 cannot store transformed appearance fields",
        ));
    }

    if version < Q1S_VERSION_EASING
        && project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .any(|placement| matches!(placement.tween, Tween::Eased { .. }))
    {
        return Err(Error::Validation(
            "legacy q1s versions cannot store easing curves",
        ));
    }

    let asset_count = u16::try_from(project.assets.len())
        .map_err(|_| Error::Overflow("asset count exceeds u16"))?;
    let q0rg_count = u16::try_from(project.q0rgs.len())
        .map_err(|_| Error::Overflow("q0rg count exceeds u16"))?;

    let mut out = Vec::new();
    out.extend_from_slice(&Q1S_V2_MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // flags
    out.extend_from_slice(&asset_count.to_le_bytes());
    out.extend_from_slice(&q0rg_count.to_le_bytes());

    write_string_u16(&mut out, &project.meta.name)?;
    out.extend_from_slice(&project.meta.fps.to_le_bytes());
    out.extend_from_slice(&project.meta.stage_width.to_le_bytes());
    out.extend_from_slice(&project.meta.stage_height.to_le_bytes());
    out.extend_from_slice(&project.meta.entry_q0rg_id.to_le_bytes());

    let mut assets: Vec<&Asset> = project.assets.iter().collect();
    assets.sort_by_key(|a| a.id());
    for asset in assets {
        write_asset(
            &mut out,
            asset,
            project.asset_names.get(&asset.id()).map(String::as_str),
            version,
        )?;
    }

    let mut q0rgs: Vec<&Q0rg> = project.q0rgs.iter().collect();
    q0rgs.sort_by_key(|q| q.q0rg_id);
    for q0rg in q0rgs {
        write_q0rg(&mut out, q0rg, version)?;
    }

    if version >= Q1S_VERSION_LAYER_FOLDERS {
        let metadata_count = u16::try_from(project.layer_metadata.len())
            .map_err(|_| Error::Overflow("layer metadata count exceeds u16"))?;
        out.extend_from_slice(&metadata_count.to_le_bytes());
        let mut metadata: Vec<_> = project.layer_metadata.iter().collect();
        metadata.sort_by_key(|(key, _)| **key);
        for (key, value) in metadata {
            out.extend_from_slice(&key.q0rg_id.to_le_bytes());
            out.extend_from_slice(&key.layer_id.to_le_bytes());
            out.push(match value.kind {
                LayerKind::Normal => LAYER_KIND_NORMAL,
                LayerKind::Folder => LAYER_KIND_FOLDER,
            });
            match value.parent_folder_id {
                Some(parent_id) => {
                    out.push(1);
                    out.extend_from_slice(&parent_id.to_le_bytes());
                }
                None => out.push(0),
            }
            out.push(u8::from(value.collapsed));
            if version >= Q1S_VERSION_LAYER_STATE {
                out.push(u8::from(value.hidden));
                out.push(u8::from(value.locked));
            }
        }
    }

    if version >= Q1S_VERSION_APPEARANCE_MASKS {
        let appearance_count = u16::try_from(project.asset_appearances.len())
            .map_err(|_| Error::Overflow("vector appearance count exceeds u16"))?;
        out.extend_from_slice(&appearance_count.to_le_bytes());
        let mut appearances: Vec<_> = project.asset_appearances.iter().collect();
        appearances.sort_by_key(|(asset_id, _)| **asset_id);
        for (asset_id, appearance) in appearances {
            out.extend_from_slice(&asset_id.to_le_bytes());
            match appearance.material {
                VectorMaterial::Solid => out.push(VECTOR_MATERIAL_SOLID),
                VectorMaterial::SoftHalo { radius, opacity } => {
                    out.push(VECTOR_MATERIAL_SOFT_HALO);
                    out.extend_from_slice(&radius.to_le_bytes());
                    out.extend_from_slice(&opacity.to_le_bytes());
                }
            }
            let path_count = u16::try_from(appearance.erase_mask.len())
                .map_err(|_| Error::Overflow("appearance erase path count exceeds u16"))?;
            out.extend_from_slice(&path_count.to_le_bytes());
            for path in &appearance.erase_mask {
                write_mask_path(&mut out, path)?;
            }
            if version >= Q1S_VERSION_APPEARANCE_FRAGMENTS {
                let source_count =
                    u16::try_from(appearance.material_source.len()).map_err(|_| {
                        Error::Overflow("appearance material source path count exceeds u16")
                    })?;
                out.extend_from_slice(&source_count.to_le_bytes());
                for path in &appearance.material_source {
                    write_mask_path(&mut out, path)?;
                }
                let clip_count = u16::try_from(appearance.clip_mask.len())
                    .map_err(|_| Error::Overflow("appearance clip path count exceeds u16"))?;
                out.extend_from_slice(&clip_count.to_le_bytes());
                for path in &appearance.clip_mask {
                    write_mask_path(&mut out, path)?;
                }
            }
            if version >= Q1S_VERSION_APPEARANCE_AFFINE {
                for value in [
                    appearance.field_transform.a11,
                    appearance.field_transform.a12,
                    appearance.field_transform.a21,
                    appearance.field_transform.a22,
                    appearance.field_transform.tx,
                    appearance.field_transform.ty,
                ] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }

    if version >= Q1S_VERSION_AUDIO_TIMELINE_CLIPS {
        let clip_count = u16::try_from(project.audio_clips.len())
            .map_err(|_| Error::Overflow("audio clip count exceeds u16"))?;
        out.extend_from_slice(&clip_count.to_le_bytes());
        let mut clips = project.audio_clips.clone();
        clips.sort_by_key(|clip| (clip.q0rg_id, clip.layer_id, clip.start_frame, clip.asset_id));
        for clip in clips {
            out.extend_from_slice(&clip.q0rg_id.to_le_bytes());
            out.extend_from_slice(&clip.layer_id.to_le_bytes());
            out.extend_from_slice(&clip.start_frame.to_le_bytes());
            out.extend_from_slice(&clip.asset_id.to_le_bytes());
            out.extend_from_slice(&clip.gain.to_le_bytes());
            out.push(u8::from(clip.muted));
        }
    }

    if version >= Q1S_VERSION_PROJECT_RUNTIME {
        let node_count = u16::try_from(project.runtime.project_graph.nodes.len())
            .map_err(|_| Error::Overflow("project dependency node count exceeds u16"))?;
        out.extend_from_slice(&node_count.to_le_bytes());
        let mut nodes = project
            .runtime
            .project_graph
            .nodes
            .iter()
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| node.node_id);
        for node in nodes {
            out.extend_from_slice(&node.node_id.to_le_bytes());
            match node.parent_node_id {
                Some(parent) => {
                    out.push(1);
                    out.extend_from_slice(&parent.to_le_bytes());
                }
                None => out.push(0),
            }
            write_string_u16(&mut out, &node.alias)?;
            out.push(match node.kind {
                ProjectDependencyKind::Movie => 0,
                ProjectDependencyKind::Q0lang => 1,
            });
            match &node.source {
                ProjectDependencySource::External(path) => {
                    out.push(0);
                    write_string_u16(&mut out, path)?;
                }
                ProjectDependencySource::Embedded(bytes) => {
                    out.push(1);
                    let len = u32::try_from(bytes.len())
                        .map_err(|_| Error::Overflow("embedded dependency exceeds u32"))?;
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(bytes);
                }
            }
        }

        let script_count = u16::try_from(project.runtime.frame_scripts.len())
            .map_err(|_| Error::Overflow("frame script count exceeds u16"))?;
        out.extend_from_slice(&script_count.to_le_bytes());
        let mut scripts = project.runtime.frame_scripts.iter().collect::<Vec<_>>();
        scripts.sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));
        for script in scripts {
            out.extend_from_slice(&script.q0rg_id.to_le_bytes());
            out.extend_from_slice(&script.layer_id.to_le_bytes());
            out.extend_from_slice(&script.frame.to_le_bytes());
            write_string_u16(&mut out, &script.source)?;
        }

        let name_count = u16::try_from(project.runtime.instance_names.len())
            .map_err(|_| Error::Overflow("runtime instance name count exceeds u16"))?;
        out.extend_from_slice(&name_count.to_le_bytes());
        let mut names = project.runtime.instance_names.iter().collect::<Vec<_>>();
        names.sort_by_key(|(key, _)| (key.q0rg_id, key.instance_id));
        for (key, name) in names {
            out.extend_from_slice(&key.q0rg_id.to_le_bytes());
            out.extend_from_slice(&key.instance_id.to_le_bytes());
            write_string_u16(&mut out, name)?;
        }
    }

    Ok(out)
}

fn write_asset(
    out: &mut Vec<u8>,
    asset: &Asset,
    asset_name: Option<&str>,
    version: u16,
) -> Result<(), Error> {
    match asset {
        Asset::Bitmap(b) => {
            out.extend_from_slice(&b.asset_id.to_le_bytes());
            out.push(ASSET_KIND_BITMAP);
            if version >= Q1S_VERSION_ASSET_NAMES {
                write_string_u16(out, asset_name.unwrap_or(""))?;
            }
            out.extend_from_slice(&b.width.to_le_bytes());
            out.extend_from_slice(&b.height.to_le_bytes());
            let payload_len = u32::try_from(b.rgba.len())
                .map_err(|_| Error::Overflow("bitmap payload length exceeds u32"))?;
            out.extend_from_slice(&payload_len.to_le_bytes());
            out.extend_from_slice(&b.rgba);
        }
        Asset::Vector(v) => {
            out.extend_from_slice(&v.asset_id.to_le_bytes());
            out.push(ASSET_KIND_VECTOR);
            if version >= Q1S_VERSION_ASSET_NAMES {
                write_string_u16(out, asset_name.unwrap_or(""))?;
            }

            match &v.fill {
                Some(c) => {
                    out.push(1);
                    out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
                }
                None => out.push(0),
            }
            match &v.stroke {
                Some(s) => {
                    // Flag 2 = "stroke + explicit cap byte". Flag 1 is
                    // the legacy (round-cap-implicit) form, still
                    // accepted by the reader but never emitted.
                    out.push(2);
                    out.extend_from_slice(&[s.color.r, s.color.g, s.color.b, s.color.a]);
                    out.extend_from_slice(&s.width.to_le_bytes());
                    out.push(match s.cap {
                        crate::geom::CapShape::Round => 0,
                        crate::geom::CapShape::Butt => 1,
                    });
                }
                None => out.push(0),
            }

            let path_count = u16::try_from(v.paths.len())
                .map_err(|_| Error::Overflow("path count exceeds u16"))?;
            out.extend_from_slice(&path_count.to_le_bytes());
            for path in &v.paths {
                out.push(if path.closed { 1 } else { 0 });
                let anchor_count = u16::try_from(path.anchors.len())
                    .map_err(|_| Error::Overflow("anchor count exceeds u16"))?;
                out.extend_from_slice(&anchor_count.to_le_bytes());
                for anchor in &path.anchors {
                    out.extend_from_slice(&anchor.point.x.to_le_bytes());
                    out.extend_from_slice(&anchor.point.y.to_le_bytes());
                    write_optional_vec2(out, anchor.in_handle);
                    write_optional_vec2(out, anchor.out_handle);
                }
            }
        }
        Asset::Q0v(v) => {
            out.extend_from_slice(&v.asset_id.to_le_bytes());
            out.push(ASSET_KIND_Q0V);
            if version >= Q1S_VERSION_ASSET_NAMES {
                write_string_u16(out, asset_name.unwrap_or(""))?;
            }
            let payload_len = u32::try_from(v.bytes.len())
                .map_err(|_| Error::Overflow("q0v payload length exceeds u32"))?;
            out.extend_from_slice(&payload_len.to_le_bytes());
            out.extend_from_slice(&v.bytes);
        }
        Asset::Rig(rig) => {
            if version < Q1S_VERSION_RIGGING {
                return Err(Error::Validation(
                    "legacy q1s versions cannot store rigging",
                ));
            }
            write_rig_asset(out, rig, asset_name, version)?;
        }
    }
    Ok(())
}

fn write_rig_asset(
    out: &mut Vec<u8>,
    rig: &RigAsset,
    asset_name: Option<&str>,
    version: u16,
) -> Result<(), Error> {
    out.extend_from_slice(&rig.asset_id.to_le_bytes());
    out.push(ASSET_KIND_RIG);
    write_string_u16(out, asset_name.unwrap_or(""))?;
    out.extend_from_slice(&rig.owner_q0rg_id.to_le_bytes());
    let mut collection_lengths = vec![
        rig.nodes.len(),
        rig.controls.len(),
        rig.constraints.len(),
        rig.channels.len(),
        rig.drivers.len(),
        rig.poses.len(),
    ];
    if version >= Q1S_VERSION_RIG_DEFORMERS {
        collection_lengths.push(rig.deformers.len());
    }
    if version >= Q1S_VERSION_RIG_POSE_VARIANTS {
        collection_lengths.push(rig.pose_drivers.len());
        collection_lengths.push(rig.mirror_pairs.len());
        collection_lengths.push(rig.variants.len());
    }
    for len in collection_lengths {
        out.extend_from_slice(
            &u16::try_from(len)
                .map_err(|_| Error::Overflow("rig collection count exceeds u16"))?
                .to_le_bytes(),
        );
    }
    for node in &rig.nodes {
        out.extend_from_slice(&node.node_id.to_le_bytes());
        write_string_u16(out, &node.name)?;
        write_optional_u16(out, node.parent);
        write_transform2d(out, node.rest);
        out.extend_from_slice(&node.length.to_le_bytes());
        match node.binding {
            None => out.push(0),
            Some(binding) => {
                out.push(1);
                out.extend_from_slice(&binding.instance_id.to_le_bytes());
                write_affine(out, binding.bind_offset);
            }
        }
    }
    for control in &rig.controls {
        out.extend_from_slice(&control.control_id.to_le_bytes());
        write_string_u16(out, &control.name)?;
        out.push(match control.kind {
            RigControlKind::Position2D => 0,
            RigControlKind::Rotation => 1,
            RigControlKind::Slider => 2,
            RigControlKind::Toggle => 3,
        });
        write_optional_u16(out, control.target_node);
        for value in [
            control.rest_x,
            control.rest_y,
            control.rest_value,
            control.min_value,
            control.max_value,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(u8::from(control.public_in_simple));
    }
    for constraint in &rig.constraints {
        match *constraint {
            RigConstraint::RotationLimit {
                constraint_id,
                node_id,
                min_radians,
                max_radians,
            } => {
                out.push(0);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&node_id.to_le_bytes());
                out.extend_from_slice(&min_radians.to_le_bytes());
                out.extend_from_slice(&max_radians.to_le_bytes());
            }
            RigConstraint::PositionLimit {
                constraint_id,
                node_id,
                min_x,
                max_x,
                min_y,
                max_y,
            } => {
                out.push(2);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&node_id.to_le_bytes());
                out.extend_from_slice(&min_x.to_le_bytes());
                out.extend_from_slice(&max_x.to_le_bytes());
                out.extend_from_slice(&min_y.to_le_bytes());
                out.extend_from_slice(&max_y.to_le_bytes());
            }
            RigConstraint::Aim {
                constraint_id,
                node_id,
                target_control,
                angle_offset,
                weight,
            } => {
                out.push(3);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&node_id.to_le_bytes());
                out.extend_from_slice(&target_control.to_le_bytes());
                out.extend_from_slice(&angle_offset.to_le_bytes());
                out.extend_from_slice(&weight.to_le_bytes());
            }
            RigConstraint::Transform {
                constraint_id,
                node_id,
                target_node,
                position_weight,
                rotation_weight,
            } => {
                out.push(4);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&node_id.to_le_bytes());
                out.extend_from_slice(&target_node.to_le_bytes());
                out.extend_from_slice(&position_weight.to_le_bytes());
                out.extend_from_slice(&rotation_weight.to_le_bytes());
            }
            RigConstraint::Distance {
                constraint_id,
                node_id,
                target_control,
                min_distance,
                max_distance,
                weight,
            } => {
                out.push(5);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&node_id.to_le_bytes());
                out.extend_from_slice(&target_control.to_le_bytes());
                out.extend_from_slice(&min_distance.to_le_bytes());
                out.extend_from_slice(&max_distance.to_le_bytes());
                out.extend_from_slice(&weight.to_le_bytes());
            }
            RigConstraint::TwoBoneIk {
                constraint_id,
                root_node,
                mid_node,
                tip_node,
                target_control,
                pole_control,
                weight,
                allow_stretch,
                max_stretch,
            } => {
                out.push(1);
                out.extend_from_slice(&constraint_id.to_le_bytes());
                out.extend_from_slice(&root_node.to_le_bytes());
                out.extend_from_slice(&mid_node.to_le_bytes());
                out.extend_from_slice(&tip_node.to_le_bytes());
                out.extend_from_slice(&target_control.to_le_bytes());
                write_optional_u16(out, pole_control);
                out.extend_from_slice(&weight.to_le_bytes());
                out.push(u8::from(allow_stretch));
                out.extend_from_slice(&max_stretch.to_le_bytes());
            }
        }
    }
    for channel in &rig.channels {
        write_rig_property_ref(out, channel.property);
        out.extend_from_slice(
            &u16::try_from(channel.keys.len())
                .map_err(|_| Error::Overflow("rig key count exceeds u16"))?
                .to_le_bytes(),
        );
        let mut keys = channel.keys.clone();
        keys.sort_by_key(|key| key.frame);
        for key in keys {
            out.extend_from_slice(&key.frame.to_le_bytes());
            out.extend_from_slice(&key.value.to_le_bytes());
            write_easing(out, key.easing);
        }
    }
    for driver in &rig.drivers {
        out.extend_from_slice(&driver.driver_id.to_le_bytes());
        out.extend_from_slice(&driver.source_control.to_le_bytes());
        out.extend_from_slice(&driver.source_min.to_le_bytes());
        out.extend_from_slice(&driver.source_max.to_le_bytes());
        write_rig_property_ref(out, driver.target);
        out.extend_from_slice(&driver.target_min.to_le_bytes());
        out.extend_from_slice(&driver.target_max.to_le_bytes());
    }
    for pose in &rig.poses {
        out.extend_from_slice(&pose.pose_id.to_le_bytes());
        write_string_u16(out, &pose.name)?;
        out.extend_from_slice(
            &u16::try_from(pose.values.len())
                .map_err(|_| Error::Overflow("rig pose value count exceeds u16"))?
                .to_le_bytes(),
        );
        for value in &pose.values {
            write_rig_property_ref(out, value.property);
            out.extend_from_slice(&value.value.to_le_bytes());
        }
    }
    if version >= Q1S_VERSION_RIG_DEFORMERS {
        for deformer in &rig.deformers {
            match deformer {
                RigDeformer::Skin {
                    deformer_id,
                    instance_id,
                    asset_id,
                    bind_transform,
                    bones,
                    anchors,
                } => {
                    out.push(0);
                    out.extend_from_slice(&deformer_id.to_le_bytes());
                    out.extend_from_slice(&instance_id.to_le_bytes());
                    out.extend_from_slice(&asset_id.to_le_bytes());
                    write_affine(out, *bind_transform);
                    out.extend_from_slice(
                        &u16::try_from(bones.len())
                            .map_err(|_| Error::Overflow("rig skin bone count exceeds u16"))?
                            .to_le_bytes(),
                    );
                    for bone in bones {
                        out.extend_from_slice(&bone.node_id.to_le_bytes());
                        write_affine(out, bone.inverse_rest_world);
                    }
                    out.extend_from_slice(
                        &u16::try_from(anchors.len())
                            .map_err(|_| Error::Overflow("rig skin anchor count exceeds u16"))?
                            .to_le_bytes(),
                    );
                    for anchor in anchors {
                        out.extend_from_slice(&anchor.path_index.to_le_bytes());
                        out.extend_from_slice(&anchor.anchor_index.to_le_bytes());
                        out.push(
                            u8::try_from(anchor.weights.len())
                                .map_err(|_| Error::Overflow("rig skin weight count exceeds u8"))?,
                        );
                        for weight in &anchor.weights {
                            out.extend_from_slice(&weight.node_id.to_le_bytes());
                            out.extend_from_slice(&weight.weight.to_le_bytes());
                        }
                    }
                }
                RigDeformer::Bend {
                    deformer_id,
                    instance_id,
                    asset_id,
                    bind_transform,
                    axis_start,
                    axis_end,
                    start_control,
                    middle_control,
                    end_control,
                } => {
                    out.push(1);
                    out.extend_from_slice(&deformer_id.to_le_bytes());
                    out.extend_from_slice(&instance_id.to_le_bytes());
                    out.extend_from_slice(&asset_id.to_le_bytes());
                    write_affine(out, *bind_transform);
                    for point in [axis_start, axis_end] {
                        out.extend_from_slice(&point.x.to_le_bytes());
                        out.extend_from_slice(&point.y.to_le_bytes());
                    }
                    for control in [start_control, middle_control, end_control] {
                        out.extend_from_slice(&control.to_le_bytes());
                    }
                }
                RigDeformer::Cage {
                    deformer_id,
                    instance_id,
                    asset_id,
                    bind_transform,
                    rest_min,
                    rest_max,
                    controls,
                } => {
                    out.push(2);
                    out.extend_from_slice(&deformer_id.to_le_bytes());
                    out.extend_from_slice(&instance_id.to_le_bytes());
                    out.extend_from_slice(&asset_id.to_le_bytes());
                    write_affine(out, *bind_transform);
                    for point in [rest_min, rest_max] {
                        out.extend_from_slice(&point.x.to_le_bytes());
                        out.extend_from_slice(&point.y.to_le_bytes());
                    }
                    for control in controls {
                        out.extend_from_slice(&control.to_le_bytes());
                    }
                }
            }
        }
    }
    if version >= Q1S_VERSION_RIG_POSE_VARIANTS {
        for driver in &rig.pose_drivers {
            out.extend_from_slice(&driver.driver_id.to_le_bytes());
            out.extend_from_slice(&driver.source_control.to_le_bytes());
            out.extend_from_slice(&driver.pose_id.to_le_bytes());
            out.extend_from_slice(&driver.source_min.to_le_bytes());
            out.extend_from_slice(&driver.source_max.to_le_bytes());
            out.extend_from_slice(&driver.weight_min.to_le_bytes());
            out.extend_from_slice(&driver.weight_max.to_le_bytes());
            out.push(match driver.mode {
                RigPoseBlendMode::Override => 0,
                RigPoseBlendMode::Additive => 1,
            });
        }
        for pair in &rig.mirror_pairs {
            write_rig_property_ref(out, pair.left);
            write_rig_property_ref(out, pair.right);
            out.extend_from_slice(&pair.multiplier.to_le_bytes());
            out.extend_from_slice(&pair.offset.to_le_bytes());
        }
        for variant in &rig.variants {
            out.extend_from_slice(&variant.variant_id.to_le_bytes());
            write_string_u16(out, &variant.name)?;
            out.extend_from_slice(&variant.instance_id.to_le_bytes());
            out.extend_from_slice(&variant.source_control.to_le_bytes());
            out.extend_from_slice(
                &u16::try_from(variant.choices.len())
                    .map_err(|_| Error::Overflow("rig variant choice count exceeds u16"))?
                    .to_le_bytes(),
            );
            for choice in &variant.choices {
                write_string_u16(out, &choice.name)?;
                match choice.target {
                    Target::Asset(id) => {
                        out.push(0);
                        out.extend_from_slice(&id.to_le_bytes());
                    }
                    Target::Q0rg(id) => {
                        out.push(1);
                        out.extend_from_slice(&id.to_le_bytes());
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_rig_property_ref(out: &mut Vec<u8>, property: RigPropertyRef) {
    let (kind, id) = match property {
        RigPropertyRef::ControlX(id) => (0, id),
        RigPropertyRef::ControlY(id) => (1, id),
        RigPropertyRef::ControlValue(id) => (2, id),
        RigPropertyRef::NodeRotation(id) => (3, id),
        RigPropertyRef::ConstraintWeight(id) => (4, id),
        RigPropertyRef::NodeTx(id) => (5, id),
        RigPropertyRef::NodeTy(id) => (6, id),
        RigPropertyRef::NodeScaleX(id) => (7, id),
        RigPropertyRef::NodeScaleY(id) => (8, id),
    };
    out.push(kind);
    out.extend_from_slice(&id.to_le_bytes());
}

fn write_optional_u16(out: &mut Vec<u8>, value: Option<u16>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        None => out.push(0),
    }
}

fn write_transform2d(out: &mut Vec<u8>, transform: Transform2D) {
    for value in [
        transform.tx,
        transform.ty,
        transform.sx,
        transform.sy,
        transform.rotation,
        transform.skew_x,
        transform.skew_y,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn write_affine(out: &mut Vec<u8>, affine: Affine) {
    for value in [
        affine.a11, affine.a12, affine.a21, affine.a22, affine.tx, affine.ty,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn write_mask_path(out: &mut Vec<u8>, path: &Path) -> Result<(), Error> {
    out.push(u8::from(path.closed));
    let anchor_count = u16::try_from(path.anchors.len())
        .map_err(|_| Error::Overflow("appearance mask anchor count exceeds u16"))?;
    out.extend_from_slice(&anchor_count.to_le_bytes());
    for anchor in &path.anchors {
        out.extend_from_slice(&anchor.point.x.to_le_bytes());
        out.extend_from_slice(&anchor.point.y.to_le_bytes());
        write_optional_vec2(out, anchor.in_handle);
        write_optional_vec2(out, anchor.out_handle);
    }
    Ok(())
}

fn write_optional_vec2(out: &mut Vec<u8>, v: Option<Vec2>) {
    match v {
        Some(p) => {
            out.push(1);
            out.extend_from_slice(&p.x.to_le_bytes());
            out.extend_from_slice(&p.y.to_le_bytes());
        }
        None => out.push(0),
    }
}

fn write_easing(out: &mut Vec<u8>, easing: Easing) {
    match easing {
        Easing::Linear => out.push(EASING_KIND_LINEAR),
        Easing::Preset { family, mode } => {
            out.push(EASING_KIND_PRESET);
            out.push(family as u8);
            out.push(mode as u8);
        }
        Easing::CubicBezier { x1, y1, x2, y2 } => {
            out.push(EASING_KIND_CUBIC_BEZIER);
            out.extend_from_slice(&x1.to_le_bytes());
            out.extend_from_slice(&y1.to_le_bytes());
            out.extend_from_slice(&x2.to_le_bytes());
            out.extend_from_slice(&y2.to_le_bytes());
        }
    }
}

fn write_placement_fx(out: &mut Vec<u8>, fx: PlacementFx, version: u16) {
    let mut flags = 0u8;
    if (fx.opacity - 1.0).abs() > 1.0e-6 {
        flags |= PLACEMENT_FX_OPACITY;
    }
    if fx.blend_mode != BlendMode::Normal {
        flags |= PLACEMENT_FX_BLEND;
    }
    if fx.blur.is_some() {
        flags |= PLACEMENT_FX_BLUR;
    }
    if fx.glow.is_some() {
        flags |= PLACEMENT_FX_GLOW;
    }
    if fx.shadow.is_some() {
        flags |= PLACEMENT_FX_SHADOW;
    }
    if version >= Q1S_VERSION_AUDIO_CLIP_FX {
        if (fx.audio_gain - 1.0).abs() > 1.0e-6 {
            flags |= PLACEMENT_FX_AUDIO_GAIN;
        }
        if fx.audio_muted {
            flags |= PLACEMENT_FX_AUDIO_MUTED;
        }
    }
    out.push(flags);
    if flags & PLACEMENT_FX_OPACITY != 0 {
        out.extend_from_slice(&fx.opacity.to_le_bytes());
    }
    if flags & PLACEMENT_FX_BLEND != 0 {
        out.push(match fx.blend_mode {
            BlendMode::Normal => BLEND_MODE_NORMAL,
            BlendMode::Multiply => BLEND_MODE_MULTIPLY,
            BlendMode::Screen => BLEND_MODE_SCREEN,
            BlendMode::Add => BLEND_MODE_ADD,
            BlendMode::Overlay => BLEND_MODE_OVERLAY,
        });
    }
    if let Some(blur) = fx.blur {
        out.extend_from_slice(&blur.radius.to_le_bytes());
    }
    if let Some(glow) = fx.glow {
        out.extend_from_slice(&[glow.color.r, glow.color.g, glow.color.b, glow.color.a]);
        out.extend_from_slice(&glow.radius.to_le_bytes());
        out.extend_from_slice(&glow.strength.to_le_bytes());
    }
    if flags & PLACEMENT_FX_AUDIO_GAIN != 0 {
        out.extend_from_slice(&fx.audio_gain.to_le_bytes());
    }
    if let Some(shadow) = fx.shadow {
        out.extend_from_slice(&[
            shadow.color.r,
            shadow.color.g,
            shadow.color.b,
            shadow.color.a,
        ]);
        out.extend_from_slice(&shadow.blur_radius.to_le_bytes());
        out.extend_from_slice(&shadow.offset_x.to_le_bytes());
        out.extend_from_slice(&shadow.offset_y.to_le_bytes());
        out.extend_from_slice(&shadow.strength.to_le_bytes());
    }
}

fn read_placement_fx(c: &mut Cursor<'_>, version: u16) -> Result<PlacementFx, Error> {
    let flags = c.read_u8()?;
    let mut allowed_flags = PLACEMENT_FX_OPACITY
        | PLACEMENT_FX_BLEND
        | PLACEMENT_FX_BLUR
        | PLACEMENT_FX_GLOW
        | PLACEMENT_FX_SHADOW;
    if version >= Q1S_VERSION_AUDIO_CLIP_FX {
        allowed_flags |= PLACEMENT_FX_AUDIO_GAIN | PLACEMENT_FX_AUDIO_MUTED;
    }
    if flags & !allowed_flags != 0 {
        return Err(Error::Validation("invalid placement fx flags"));
    }
    let opacity = if flags & PLACEMENT_FX_OPACITY != 0 {
        c.read_f32()?
    } else {
        1.0
    };
    let blend_mode = if flags & PLACEMENT_FX_BLEND != 0 {
        match c.read_u8()? {
            BLEND_MODE_NORMAL => BlendMode::Normal,
            BLEND_MODE_MULTIPLY => BlendMode::Multiply,
            BLEND_MODE_SCREEN => BlendMode::Screen,
            BLEND_MODE_ADD => BlendMode::Add,
            BLEND_MODE_OVERLAY => BlendMode::Overlay,
            _ => return Err(Error::Validation("invalid placement blend mode")),
        }
    } else {
        BlendMode::Normal
    };
    let blur = if flags & PLACEMENT_FX_BLUR != 0 {
        Some(BlurFx {
            radius: c.read_f32()?,
        })
    } else {
        None
    };
    let glow = if flags & PLACEMENT_FX_GLOW != 0 {
        Some(GlowFx {
            color: Rgba {
                r: c.read_u8()?,
                g: c.read_u8()?,
                b: c.read_u8()?,
                a: c.read_u8()?,
            },
            radius: c.read_f32()?,
            strength: c.read_f32()?,
        })
    } else {
        None
    };
    let audio_gain = if flags & PLACEMENT_FX_AUDIO_GAIN != 0 {
        c.read_f32()?
    } else {
        1.0
    };
    let audio_muted = flags & PLACEMENT_FX_AUDIO_MUTED != 0;
    let shadow = if flags & PLACEMENT_FX_SHADOW != 0 {
        Some(DropShadowFx {
            color: Rgba {
                r: c.read_u8()?,
                g: c.read_u8()?,
                b: c.read_u8()?,
                a: c.read_u8()?,
            },
            blur_radius: c.read_f32()?,
            offset_x: c.read_f32()?,
            offset_y: c.read_f32()?,
            strength: c.read_f32()?,
        })
    } else {
        None
    };
    Ok(PlacementFx {
        opacity,
        blend_mode,
        blur,
        glow,
        shadow,
        audio_gain,
        audio_muted,
    })
}

fn write_q0rg(out: &mut Vec<u8>, q0rg: &Q0rg, version: u16) -> Result<(), Error> {
    out.extend_from_slice(&q0rg.q0rg_id.to_le_bytes());
    write_string_u16(out, &q0rg.name)?;
    out.extend_from_slice(&q0rg.frame_count.to_le_bytes());
    write_string_u16(out, &q0rg.script)?;

    let layer_count =
        u16::try_from(q0rg.layers.len()).map_err(|_| Error::Overflow("layer count exceeds u16"))?;
    out.extend_from_slice(&layer_count.to_le_bytes());

    let mut layers: Vec<&Layer> = q0rg.layers.iter().collect();
    if version < Q1S_VERSION_LAYER_FOLDERS {
        layers.sort_by_key(|layer| layer.layer_id);
    }
    for layer in layers {
        out.extend_from_slice(&layer.layer_id.to_le_bytes());
        write_string_u16(out, &layer.name)?;

        let placement_count = u16::try_from(layer.placements.len())
            .map_err(|_| Error::Overflow("placement count exceeds u16"))?;
        out.extend_from_slice(&placement_count.to_le_bytes());

        let mut placements: Vec<&Placement> = layer.placements.iter().collect();
        placements.sort_by_key(|p| p.frame);
        for p in placements {
            if version >= Q1S_VERSION_RIGGING {
                out.extend_from_slice(&p.instance_id.to_le_bytes());
            }
            out.extend_from_slice(&p.frame.to_le_bytes());
            match p.target {
                Target::Asset(id) => {
                    out.push(TARGET_KIND_ASSET);
                    out.extend_from_slice(&id.to_le_bytes());
                }
                Target::Q0rg(id) => {
                    out.push(TARGET_KIND_Q0RG);
                    out.extend_from_slice(&id.to_le_bytes());
                }
            }
            out.extend_from_slice(&p.transform.tx.to_le_bytes());
            out.extend_from_slice(&p.transform.ty.to_le_bytes());
            out.extend_from_slice(&p.transform.sx.to_le_bytes());
            out.extend_from_slice(&p.transform.sy.to_le_bytes());
            out.extend_from_slice(&p.transform.rotation.to_le_bytes());
            if version >= Q1S_VERSION_SKEW {
                out.extend_from_slice(&p.transform.skew_x.to_le_bytes());
                out.extend_from_slice(&p.transform.skew_y.to_le_bytes());
            }
            match p.tween {
                Tween::None => out.push(TWEEN_KIND_NONE),
                Tween::Linear { to_frame } => {
                    out.push(TWEEN_KIND_LINEAR);
                    out.extend_from_slice(&to_frame.to_le_bytes());
                }
                Tween::Eased { to_frame, easing } => {
                    out.push(TWEEN_KIND_EASED);
                    out.extend_from_slice(&to_frame.to_le_bytes());
                    write_easing(out, easing);
                }
            }
            if version >= Q1S_VERSION_PLACEMENT_FX {
                write_placement_fx(out, p.fx, version);
            }
        }

        if version >= Q1S_VERSION_KEYFRAMES {
            let explicit_keyframe_count = u16::try_from(layer.explicit_keyframes.len())
                .map_err(|_| Error::Overflow("explicit keyframe count exceeds u16"))?;
            out.extend_from_slice(&explicit_keyframe_count.to_le_bytes());
            let mut explicit_keyframes = layer.explicit_keyframes.clone();
            explicit_keyframes.sort_unstable();
            for frame in explicit_keyframes {
                out.extend_from_slice(&frame.to_le_bytes());
            }
        }
    }
    Ok(())
}

pub fn parse(bytes: &[u8]) -> Result<ProjectV2, Error> {
    let mut c = Cursor::new(bytes);

    let magic = c.read_exact(4)?;
    let mut magic_arr = [0_u8; 4];
    magic_arr.copy_from_slice(magic);
    if magic_arr != Q1S_V2_MAGIC {
        return Err(Error::InvalidMagic(magic_arr));
    }

    let version = c.read_u16()?;
    if version != Q1S_VERSION_LEGACY
        && version != Q1S_VERSION_SKEW
        && version != Q1S_VERSION_KEYFRAMES
        && version != Q1S_VERSION_ASSET_NAMES
        && version != Q1S_VERSION_LAYER_FOLDERS
        && version != Q1S_VERSION_Q0V_ASSETS
        && version != Q1S_VERSION_EASING
        && version != Q1S_VERSION_APPEARANCE_MASKS
        && version != Q1S_VERSION_APPEARANCE_FRAGMENTS
        && version != Q1S_VERSION_APPEARANCE_AFFINE
        && version != Q1S_VERSION_LAYER_STATE
        && version != Q1S_VERSION_NESTED_LAYER_FOLDERS
        && version != Q1S_VERSION_PLACEMENT_FX
        && version != Q1S_VERSION_RIGGING
        && version != Q1S_VERSION_RIG_PRO
        && version != Q1S_VERSION_RIG_DEFORMERS
        && version != Q1S_VERSION_RIG_POSE_VARIANTS
        && version != Q1S_VERSION_AUDIO_CLIP_FX
        && version != Q1S_VERSION_AUDIO_TIMELINE_CLIPS
        && version != Q1S_VERSION_CURRENT
    {
        return Err(Error::UnsupportedVersion(version));
    }
    parse_body(c, version)
}

/// Parse a project body after a six-byte wrapper header without copying the
/// full input. Vector q0s revisions reuse q1s body layouts but have their own
/// public version numbers, so the caller supplies the matching q1s body version.
pub(crate) fn parse_body_after_header(bytes: &[u8], body_version: u16) -> Result<ProjectV2, Error> {
    let mut c = Cursor::new(bytes);
    c.read_exact(6)?;
    parse_body(c, body_version)
}

fn parse_body(mut c: Cursor<'_>, version: u16) -> Result<ProjectV2, Error> {
    let flags = c.read_u16()?;
    if flags != 0 {
        return Err(Error::UnsupportedFlags(flags));
    }
    let asset_count = c.read_u16()?;
    let q0rg_count = c.read_u16()?;

    let name = c.read_string_u16()?;
    let fps = c.read_u16()?;
    let stage_width = c.read_u16()?;
    let stage_height = c.read_u16()?;
    let entry_q0rg_id = c.read_u16()?;

    let mut assets = Vec::with_capacity(usize::from(asset_count));
    let mut asset_names = HashMap::new();
    for _ in 0..asset_count {
        let (asset, asset_name) = parse_asset(&mut c, version)?;
        if let Some(name) = asset_name {
            asset_names.insert(asset.id(), name);
        }
        assets.push(asset);
    }

    let mut q0rgs = Vec::with_capacity(usize::from(q0rg_count));
    for _ in 0..q0rg_count {
        q0rgs.push(parse_q0rg(&mut c, version)?);
    }

    let mut layer_metadata = HashMap::new();
    if version >= Q1S_VERSION_LAYER_FOLDERS {
        let count = c.read_u16()?;
        layer_metadata.reserve(usize::from(count));
        for _ in 0..count {
            let key = LayerKey::new(c.read_u16()?, c.read_u16()?);
            let kind = match c.read_u8()? {
                LAYER_KIND_NORMAL => LayerKind::Normal,
                LAYER_KIND_FOLDER => LayerKind::Folder,
                _ => return Err(Error::Validation("invalid layer metadata kind")),
            };
            let parent_folder_id = match c.read_u8()? {
                0 => None,
                1 => Some(c.read_u16()?),
                _ => return Err(Error::Validation("invalid layer parent flag")),
            };
            let collapsed = match c.read_u8()? {
                0 => false,
                1 => true,
                _ => return Err(Error::Validation("invalid layer collapsed flag")),
            };
            let (hidden, locked) = if version >= Q1S_VERSION_LAYER_STATE {
                let hidden = match c.read_u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Validation("invalid layer hidden flag")),
                };
                let locked = match c.read_u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Validation("invalid layer locked flag")),
                };
                (hidden, locked)
            } else {
                (false, false)
            };
            if layer_metadata
                .insert(
                    key,
                    LayerMetadata {
                        kind,
                        parent_folder_id,
                        collapsed,
                        hidden,
                        locked,
                    },
                )
                .is_some()
            {
                return Err(Error::Validation("duplicate layer metadata entry"));
            }
        }
    }

    let mut asset_appearances = HashMap::new();
    if version >= Q1S_VERSION_APPEARANCE_MASKS {
        let count = c.read_u16()?;
        asset_appearances.reserve(usize::from(count));
        for _ in 0..count {
            let asset_id = c.read_u16()?;
            let material = match c.read_u8()? {
                VECTOR_MATERIAL_SOLID => VectorMaterial::Solid,
                VECTOR_MATERIAL_SOFT_HALO => VectorMaterial::SoftHalo {
                    radius: c.read_f32()?,
                    opacity: c.read_f32()?,
                },
                _ => return Err(Error::Validation("invalid vector material kind")),
            };
            let path_count = c.read_u16()?;
            let mut erase_mask = Vec::with_capacity(usize::from(path_count));
            for _ in 0..path_count {
                erase_mask.push(read_mask_path(&mut c)?);
            }
            let (material_source, clip_mask) = if version >= Q1S_VERSION_APPEARANCE_FRAGMENTS {
                let source_count = c.read_u16()?;
                let mut source = Vec::with_capacity(usize::from(source_count));
                for _ in 0..source_count {
                    source.push(read_mask_path(&mut c)?);
                }
                let clip_count = c.read_u16()?;
                let mut clip = Vec::with_capacity(usize::from(clip_count));
                for _ in 0..clip_count {
                    clip.push(read_mask_path(&mut c)?);
                }
                (source, clip)
            } else {
                (Vec::new(), Vec::new())
            };
            let field_transform = if version >= Q1S_VERSION_APPEARANCE_AFFINE {
                Affine {
                    a11: c.read_f32()?,
                    a12: c.read_f32()?,
                    a21: c.read_f32()?,
                    a22: c.read_f32()?,
                    tx: c.read_f32()?,
                    ty: c.read_f32()?,
                }
            } else {
                Affine::IDENTITY
            };
            if asset_appearances
                .insert(
                    asset_id,
                    VectorAppearance {
                        material,
                        erase_mask,
                        material_source,
                        clip_mask,
                        field_transform,
                    },
                )
                .is_some()
            {
                return Err(Error::Validation("duplicate vector appearance entry"));
            }
        }
    }

    let mut audio_clips = Vec::new();
    if version >= Q1S_VERSION_AUDIO_TIMELINE_CLIPS {
        let count = c.read_u16()?;
        audio_clips.reserve(usize::from(count));
        for _ in 0..count {
            let q0rg_id = c.read_u16()?;
            let layer_id = c.read_u16()?;
            let start_frame = c.read_u16()?;
            let asset_id = c.read_u16()?;
            let gain = c.read_f32()?;
            let muted = match c.read_u8()? {
                0 => false,
                1 => true,
                _ => return Err(Error::Validation("invalid audio clip mute flag")),
            };
            audio_clips.push(AudioClip {
                q0rg_id,
                layer_id,
                start_frame,
                asset_id,
                gain,
                muted,
            });
        }
    }

    let mut runtime = ProjectRuntimeData::default();
    if version >= Q1S_VERSION_PROJECT_RUNTIME {
        let node_count = c.read_u16()?;
        runtime.project_graph.nodes.reserve(usize::from(node_count));
        for _ in 0..node_count {
            let node_id = c.read_u16()?;
            let parent_node_id = match c.read_u8()? {
                0 => None,
                1 => Some(c.read_u16()?),
                _ => return Err(Error::Validation("invalid project dependency parent flag")),
            };
            let alias = c.read_string_u16()?;
            let kind = match c.read_u8()? {
                0 => ProjectDependencyKind::Movie,
                1 => ProjectDependencyKind::Q0lang,
                _ => return Err(Error::Validation("invalid project dependency kind")),
            };
            let source = match c.read_u8()? {
                0 => ProjectDependencySource::External(c.read_string_u16()?),
                1 => {
                    let len = usize::try_from(c.read_u32()?)
                        .map_err(|_| Error::Validation("embedded dependency length is invalid"))?;
                    ProjectDependencySource::Embedded(c.read_exact(len)?.to_vec())
                }
                _ => return Err(Error::Validation("invalid project dependency source kind")),
            };
            runtime.project_graph.nodes.push(ProjectDependencyNode {
                node_id,
                parent_node_id,
                alias,
                kind,
                source,
            });
        }

        let script_count = c.read_u16()?;
        runtime.frame_scripts.reserve(usize::from(script_count));
        for _ in 0..script_count {
            runtime.frame_scripts.push(FrameScript {
                q0rg_id: c.read_u16()?,
                layer_id: c.read_u16()?,
                frame: c.read_u16()?,
                source: c.read_string_u16()?,
            });
        }

        let name_count = c.read_u16()?;
        runtime.instance_names.reserve(usize::from(name_count));
        for _ in 0..name_count {
            let key = InstanceKey::new(c.read_u16()?, c.read_u32()?);
            let name = c.read_string_u16()?;
            if runtime.instance_names.insert(key, name).is_some() {
                return Err(Error::Validation("duplicate runtime instance name entry"));
            }
        }
    }

    c.finish()?;

    let project = ProjectV2 {
        meta: ProjectMeta {
            name,
            fps,
            stage_width,
            stage_height,
            entry_q0rg_id,
        },
        assets,
        asset_names,
        asset_appearances,
        layer_metadata,
        audio_clips,
        runtime,
        q0rgs,
    };
    if version < Q1S_VERSION_NESTED_LAYER_FOLDERS
        && project.layer_metadata.values().any(|metadata| {
            metadata.kind == LayerKind::Folder && metadata.parent_folder_id.is_some()
        })
    {
        return Err(Error::Validation(
            "legacy q1s file cannot contain nested layer folders",
        ));
    }
    validate(&project)?;
    Ok(project)
}

fn parse_asset(c: &mut Cursor, version: u16) -> Result<(Asset, Option<String>), Error> {
    let asset_id = c.read_u16()?;
    let kind = c.read_u8()?;
    let asset_name = if version >= Q1S_VERSION_ASSET_NAMES {
        let name = c.read_string_u16()?;
        (!name.is_empty()).then_some(name)
    } else {
        None
    };
    let asset = match kind {
        ASSET_KIND_BITMAP => {
            let width = c.read_u16()?;
            let height = c.read_u16()?;
            let len = usize::try_from(c.read_u32()?)
                .map_err(|_| Error::Validation("asset payload length does not fit usize"))?;
            let rgba = c.read_exact(len)?.to_vec();
            Asset::Bitmap(BitmapAsset {
                asset_id,
                width,
                height,
                rgba,
            })
        }
        ASSET_KIND_VECTOR => {
            let fill = match c.read_u8()? {
                0 => None,
                1 => {
                    let bytes = c.read_exact(4)?;
                    Some(Rgba {
                        r: bytes[0],
                        g: bytes[1],
                        b: bytes[2],
                        a: bytes[3],
                    })
                }
                _ => return Err(Error::Validation("invalid has_fill flag")),
            };
            let stroke = match c.read_u8()? {
                0 => None,
                // Legacy (pre-cap) stroke Р В Р вЂ Р В РІР‚С™Р Р†Р вЂљРЎСљ implicit round cap.
                1 => {
                    let bytes = c.read_exact(4)?;
                    let color = Rgba {
                        r: bytes[0],
                        g: bytes[1],
                        b: bytes[2],
                        a: bytes[3],
                    };
                    let width = c.read_f32()?;
                    Some(Stroke {
                        color,
                        width,
                        cap: crate::geom::CapShape::Round,
                    })
                }
                // Modern: explicit cap byte after width.
                2 => {
                    let bytes = c.read_exact(4)?;
                    let color = Rgba {
                        r: bytes[0],
                        g: bytes[1],
                        b: bytes[2],
                        a: bytes[3],
                    };
                    let width = c.read_f32()?;
                    let cap = match c.read_u8()? {
                        0 => crate::geom::CapShape::Round,
                        1 => crate::geom::CapShape::Butt,
                        _ => return Err(Error::Validation("invalid stroke cap byte")),
                    };
                    Some(Stroke { color, width, cap })
                }
                _ => return Err(Error::Validation("invalid has_stroke flag")),
            };

            let path_count = c.read_u16()?;
            let mut paths = Vec::with_capacity(usize::from(path_count));
            for _ in 0..path_count {
                let closed = match c.read_u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Validation("invalid path closed flag")),
                };
                let anchor_count = c.read_u16()?;
                let mut anchors = Vec::with_capacity(usize::from(anchor_count));
                for _ in 0..anchor_count {
                    let point = Vec2::new(c.read_f32()?, c.read_f32()?);
                    let in_handle = read_optional_vec2(c)?;
                    let out_handle = read_optional_vec2(c)?;
                    anchors.push(Anchor {
                        point,
                        in_handle,
                        out_handle,
                    });
                }
                paths.push(Path { anchors, closed });
            }
            Asset::Vector(VectorAsset {
                asset_id,
                paths,
                fill,
                stroke,
            })
        }
        ASSET_KIND_Q0V if version >= Q1S_VERSION_Q0V_ASSETS => {
            let len = usize::try_from(c.read_u32()?)
                .map_err(|_| Error::Validation("q0v payload length does not fit usize"))?;
            Asset::Q0v(Q0vAsset {
                asset_id,
                bytes: c.read_exact(len)?.to_vec(),
            })
        }
        ASSET_KIND_RIG if version >= Q1S_VERSION_RIGGING => {
            Asset::Rig(read_rig_asset(c, asset_id, version)?)
        }
        other => return Err(Error::InvalidAssetKind(other)),
    };
    Ok((asset, asset_name))
}

fn read_rig_asset(c: &mut Cursor<'_>, asset_id: u16, version: u16) -> Result<RigAsset, Error> {
    let owner_q0rg_id = c.read_u16()?;
    let node_count = c.read_u16()?;
    let control_count = c.read_u16()?;
    let constraint_count = c.read_u16()?;
    let channel_count = c.read_u16()?;
    let driver_count = c.read_u16()?;
    let pose_count = c.read_u16()?;
    let deformer_count = if version >= Q1S_VERSION_RIG_DEFORMERS {
        c.read_u16()?
    } else {
        0
    };
    let (pose_driver_count, mirror_pair_count, variant_count) =
        if version >= Q1S_VERSION_RIG_POSE_VARIANTS {
            (c.read_u16()?, c.read_u16()?, c.read_u16()?)
        } else {
            (0, 0, 0)
        };

    let mut nodes = Vec::with_capacity(usize::from(node_count));
    for _ in 0..node_count {
        let node_id = c.read_u16()?;
        let name = c.read_string_u16()?;
        let parent = read_optional_u16(c)?;
        let rest = read_transform2d(c)?;
        let length = c.read_f32()?;
        let binding = match c.read_u8()? {
            0 => None,
            1 => Some(RigBinding {
                instance_id: c.read_u32()?,
                bind_offset: read_affine(c)?,
            }),
            _ => return Err(Error::Validation("invalid rig binding flag")),
        };
        nodes.push(RigNode {
            node_id,
            name,
            parent,
            rest,
            length,
            binding,
        });
    }

    let mut controls = Vec::with_capacity(usize::from(control_count));
    for _ in 0..control_count {
        let control_id = c.read_u16()?;
        let name = c.read_string_u16()?;
        let kind = match c.read_u8()? {
            0 => RigControlKind::Position2D,
            1 => RigControlKind::Rotation,
            2 => RigControlKind::Slider,
            3 => RigControlKind::Toggle,
            _ => return Err(Error::Validation("invalid rig control kind")),
        };
        let target_node = read_optional_u16(c)?;
        let rest_x = c.read_f32()?;
        let rest_y = c.read_f32()?;
        let rest_value = c.read_f32()?;
        let min_value = c.read_f32()?;
        let max_value = c.read_f32()?;
        let public_in_simple = read_bool(c, "invalid rig control public flag")?;
        controls.push(RigControl {
            control_id,
            name,
            kind,
            target_node,
            rest_x,
            rest_y,
            rest_value,
            min_value,
            max_value,
            public_in_simple,
        });
    }

    let mut constraints = Vec::with_capacity(usize::from(constraint_count));
    for _ in 0..constraint_count {
        let kind = c.read_u8()?;
        let constraint_id = c.read_u16()?;
        constraints.push(match kind {
            0 => RigConstraint::RotationLimit {
                constraint_id,
                node_id: c.read_u16()?,
                min_radians: c.read_f32()?,
                max_radians: c.read_f32()?,
            },
            1 => RigConstraint::TwoBoneIk {
                constraint_id,
                root_node: c.read_u16()?,
                mid_node: c.read_u16()?,
                tip_node: c.read_u16()?,
                target_control: c.read_u16()?,
                pole_control: read_optional_u16(c)?,
                weight: c.read_f32()?,
                allow_stretch: read_bool(c, "invalid rig ik stretch flag")?,
                max_stretch: c.read_f32()?,
            },
            2 => RigConstraint::PositionLimit {
                constraint_id,
                node_id: c.read_u16()?,
                min_x: c.read_f32()?,
                max_x: c.read_f32()?,
                min_y: c.read_f32()?,
                max_y: c.read_f32()?,
            },
            3 => RigConstraint::Aim {
                constraint_id,
                node_id: c.read_u16()?,
                target_control: c.read_u16()?,
                angle_offset: c.read_f32()?,
                weight: c.read_f32()?,
            },
            4 => RigConstraint::Transform {
                constraint_id,
                node_id: c.read_u16()?,
                target_node: c.read_u16()?,
                position_weight: c.read_f32()?,
                rotation_weight: c.read_f32()?,
            },
            5 => RigConstraint::Distance {
                constraint_id,
                node_id: c.read_u16()?,
                target_control: c.read_u16()?,
                min_distance: c.read_f32()?,
                max_distance: c.read_f32()?,
                weight: c.read_f32()?,
            },
            _ => return Err(Error::Validation("invalid rig constraint kind")),
        });
    }

    let mut channels = Vec::with_capacity(usize::from(channel_count));
    for _ in 0..channel_count {
        let property = read_rig_property_ref(c)?;
        let key_count = c.read_u16()?;
        let mut keys = Vec::with_capacity(usize::from(key_count));
        for _ in 0..key_count {
            keys.push(RigKey {
                frame: c.read_u16()?,
                value: c.read_f32()?,
                easing: read_easing(c)?,
            });
        }
        channels.push(RigChannel { property, keys });
    }

    let mut drivers = Vec::with_capacity(usize::from(driver_count));
    for _ in 0..driver_count {
        drivers.push(RigDriver {
            driver_id: c.read_u16()?,
            source_control: c.read_u16()?,
            source_min: c.read_f32()?,
            source_max: c.read_f32()?,
            target: read_rig_property_ref(c)?,
            target_min: c.read_f32()?,
            target_max: c.read_f32()?,
        });
    }
    let mut poses = Vec::with_capacity(usize::from(pose_count));
    for _ in 0..pose_count {
        let pose_id = c.read_u16()?;
        let name = c.read_string_u16()?;
        let value_count = c.read_u16()?;
        let mut values = Vec::with_capacity(usize::from(value_count));
        for _ in 0..value_count {
            values.push(RigPoseValue {
                property: read_rig_property_ref(c)?,
                value: c.read_f32()?,
            });
        }
        poses.push(RigPosePreset {
            pose_id,
            name,
            values,
        });
    }

    let mut deformers = Vec::with_capacity(usize::from(deformer_count));
    for _ in 0..deformer_count {
        let kind = c.read_u8()?;
        let deformer_id = c.read_u16()?;
        let instance_id = c.read_u32()?;
        let target_asset_id = c.read_u16()?;
        let bind_transform = read_affine(c)?;
        deformers.push(match kind {
            0 => {
                let bone_count = c.read_u16()?;
                let mut bones = Vec::with_capacity(usize::from(bone_count));
                for _ in 0..bone_count {
                    bones.push(RigSkinBoneBind {
                        node_id: c.read_u16()?,
                        inverse_rest_world: read_affine(c)?,
                    });
                }
                let anchor_count = c.read_u16()?;
                let mut anchors = Vec::with_capacity(usize::from(anchor_count));
                for _ in 0..anchor_count {
                    let path_index = c.read_u16()?;
                    let anchor_index = c.read_u16()?;
                    let weight_count = c.read_u8()?;
                    let mut weights = Vec::with_capacity(usize::from(weight_count));
                    for _ in 0..weight_count {
                        weights.push(RigSkinWeight {
                            node_id: c.read_u16()?,
                            weight: c.read_f32()?,
                        });
                    }
                    anchors.push(RigSkinAnchorWeights {
                        path_index,
                        anchor_index,
                        weights,
                    });
                }
                RigDeformer::Skin {
                    deformer_id,
                    instance_id,
                    asset_id: target_asset_id,
                    bind_transform,
                    bones,
                    anchors,
                }
            }
            1 => RigDeformer::Bend {
                deformer_id,
                instance_id,
                asset_id: target_asset_id,
                bind_transform,
                axis_start: Vec2::new(c.read_f32()?, c.read_f32()?),
                axis_end: Vec2::new(c.read_f32()?, c.read_f32()?),
                start_control: c.read_u16()?,
                middle_control: c.read_u16()?,
                end_control: c.read_u16()?,
            },
            2 => RigDeformer::Cage {
                deformer_id,
                instance_id,
                asset_id: target_asset_id,
                bind_transform,
                rest_min: Vec2::new(c.read_f32()?, c.read_f32()?),
                rest_max: Vec2::new(c.read_f32()?, c.read_f32()?),
                controls: [c.read_u16()?, c.read_u16()?, c.read_u16()?, c.read_u16()?],
            },
            _ => return Err(Error::Validation("invalid rig deformer kind")),
        });
    }

    let mut pose_drivers = Vec::with_capacity(usize::from(pose_driver_count));
    for _ in 0..pose_driver_count {
        pose_drivers.push(RigPoseDriver {
            driver_id: c.read_u16()?,
            source_control: c.read_u16()?,
            pose_id: c.read_u16()?,
            source_min: c.read_f32()?,
            source_max: c.read_f32()?,
            weight_min: c.read_f32()?,
            weight_max: c.read_f32()?,
            mode: match c.read_u8()? {
                0 => RigPoseBlendMode::Override,
                1 => RigPoseBlendMode::Additive,
                _ => return Err(Error::Validation("invalid rig pose blend mode")),
            },
        });
    }
    let mut mirror_pairs = Vec::with_capacity(usize::from(mirror_pair_count));
    for _ in 0..mirror_pair_count {
        mirror_pairs.push(RigMirrorPair {
            left: read_rig_property_ref(c)?,
            right: read_rig_property_ref(c)?,
            multiplier: c.read_f32()?,
            offset: c.read_f32()?,
        });
    }
    let mut variants = Vec::with_capacity(usize::from(variant_count));
    for _ in 0..variant_count {
        let variant_id = c.read_u16()?;
        let name = c.read_string_u16()?;
        let instance_id = c.read_u32()?;
        let source_control = c.read_u16()?;
        let choice_count = c.read_u16()?;
        let mut choices = Vec::with_capacity(usize::from(choice_count));
        for _ in 0..choice_count {
            let choice_name = c.read_string_u16()?;
            let kind = c.read_u8()?;
            let id = c.read_u16()?;
            let target = match kind {
                0 => Target::Asset(id),
                1 => Target::Q0rg(id),
                _ => return Err(Error::Validation("invalid rig variant target kind")),
            };
            choices.push(RigVariantChoice {
                name: choice_name,
                target,
            });
        }
        variants.push(RigVariantSet {
            variant_id,
            name,
            instance_id,
            source_control,
            choices,
        });
    }

    Ok(RigAsset {
        asset_id,
        owner_q0rg_id,
        nodes,
        controls,
        constraints,
        channels,
        drivers,
        poses,
        deformers,
        pose_drivers,
        mirror_pairs,
        variants,
    })
}

fn read_rig_property_ref(c: &mut Cursor<'_>) -> Result<RigPropertyRef, Error> {
    let kind = c.read_u8()?;
    let id = c.read_u16()?;
    match kind {
        0 => Ok(RigPropertyRef::ControlX(id)),
        1 => Ok(RigPropertyRef::ControlY(id)),
        2 => Ok(RigPropertyRef::ControlValue(id)),
        3 => Ok(RigPropertyRef::NodeRotation(id)),
        4 => Ok(RigPropertyRef::ConstraintWeight(id)),
        5 => Ok(RigPropertyRef::NodeTx(id)),
        6 => Ok(RigPropertyRef::NodeTy(id)),
        7 => Ok(RigPropertyRef::NodeScaleX(id)),
        8 => Ok(RigPropertyRef::NodeScaleY(id)),
        _ => Err(Error::Validation("invalid rig property reference")),
    }
}

fn read_optional_u16(c: &mut Cursor<'_>) -> Result<Option<u16>, Error> {
    match c.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(c.read_u16()?)),
        _ => Err(Error::Validation("invalid optional u16 flag")),
    }
}

fn read_bool(c: &mut Cursor<'_>, message: &'static str) -> Result<bool, Error> {
    match c.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Validation(message)),
    }
}

fn read_transform2d(c: &mut Cursor<'_>) -> Result<Transform2D, Error> {
    Ok(Transform2D {
        tx: c.read_f32()?,
        ty: c.read_f32()?,
        sx: c.read_f32()?,
        sy: c.read_f32()?,
        rotation: c.read_f32()?,
        skew_x: c.read_f32()?,
        skew_y: c.read_f32()?,
    })
}

fn read_affine(c: &mut Cursor<'_>) -> Result<Affine, Error> {
    Ok(Affine {
        a11: c.read_f32()?,
        a12: c.read_f32()?,
        a21: c.read_f32()?,
        a22: c.read_f32()?,
        tx: c.read_f32()?,
        ty: c.read_f32()?,
    })
}
fn read_mask_path(c: &mut Cursor) -> Result<Path, Error> {
    let closed = match c.read_u8()? {
        0 => false,
        1 => true,
        _ => return Err(Error::Validation("invalid appearance mask closed flag")),
    };
    let anchor_count = c.read_u16()?;
    let mut anchors = Vec::with_capacity(usize::from(anchor_count));
    for _ in 0..anchor_count {
        anchors.push(Anchor {
            point: Vec2::new(c.read_f32()?, c.read_f32()?),
            in_handle: read_optional_vec2(c)?,
            out_handle: read_optional_vec2(c)?,
        });
    }
    Ok(Path { anchors, closed })
}

fn read_optional_vec2(c: &mut Cursor) -> Result<Option<Vec2>, Error> {
    match c.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(Vec2::new(c.read_f32()?, c.read_f32()?))),
        _ => Err(Error::Validation("invalid optional-vec2 flag")),
    }
}

fn read_easing(c: &mut Cursor) -> Result<Easing, Error> {
    match c.read_u8()? {
        EASING_KIND_LINEAR => Ok(Easing::Linear),
        EASING_KIND_PRESET => {
            let family = EasingFamily::from_u8(c.read_u8()?)
                .ok_or(Error::Validation("invalid easing family"))?;
            let mode = EasingMode::from_u8(c.read_u8()?)
                .ok_or(Error::Validation("invalid easing mode"))?;
            Ok(Easing::Preset { family, mode })
        }
        EASING_KIND_CUBIC_BEZIER => Ok(Easing::CubicBezier {
            x1: c.read_f32()?,
            y1: c.read_f32()?,
            x2: c.read_f32()?,
            y2: c.read_f32()?,
        }),
        _ => Err(Error::Validation("invalid easing kind")),
    }
}

fn parse_q0rg(c: &mut Cursor, version: u16) -> Result<Q0rg, Error> {
    let q0rg_id = c.read_u16()?;
    let name = c.read_string_u16()?;
    let frame_count = c.read_u16()?;
    let script = c.read_string_u16()?;
    let layer_count = c.read_u16()?;

    let mut layers = Vec::with_capacity(usize::from(layer_count));
    for _ in 0..layer_count {
        let layer_id = c.read_u16()?;
        let layer_name = c.read_string_u16()?;
        let placement_count = c.read_u16()?;
        let mut placements = Vec::with_capacity(usize::from(placement_count));
        for _ in 0..placement_count {
            let instance_id = if version >= Q1S_VERSION_RIGGING {
                c.read_u32()?
            } else {
                0
            };
            let frame = c.read_u16()?;
            let target_kind = c.read_u8()?;
            let target_id = c.read_u16()?;
            let target = match target_kind {
                TARGET_KIND_ASSET => Target::Asset(target_id),
                TARGET_KIND_Q0RG => Target::Q0rg(target_id),
                _ => return Err(Error::Validation("invalid target kind")),
            };
            let tx = c.read_f32()?;
            let ty = c.read_f32()?;
            let sx = c.read_f32()?;
            let sy = c.read_f32()?;
            let rotation = c.read_f32()?;
            let (skew_x, skew_y) = if version >= Q1S_VERSION_SKEW {
                (c.read_f32()?, c.read_f32()?)
            } else {
                (0.0, 0.0)
            };
            let transform = Transform2D {
                tx,
                ty,
                sx,
                sy,
                rotation,
                skew_x,
                skew_y,
            };
            let tween_kind = c.read_u8()?;
            let tween = match tween_kind {
                TWEEN_KIND_NONE => Tween::None,
                TWEEN_KIND_LINEAR => Tween::Linear {
                    to_frame: c.read_u16()?,
                },
                TWEEN_KIND_EASED if version >= Q1S_VERSION_EASING => Tween::Eased {
                    to_frame: c.read_u16()?,
                    easing: read_easing(c)?,
                },
                _ => return Err(Error::Validation("invalid tween kind")),
            };
            let fx = if version >= Q1S_VERSION_PLACEMENT_FX {
                read_placement_fx(c, version)?
            } else {
                PlacementFx::default()
            };
            placements.push(Placement {
                instance_id,
                frame,
                target,
                transform,
                tween,
                fx,
            });
        }
        let explicit_keyframes = if version >= Q1S_VERSION_KEYFRAMES {
            let count = c.read_u16()?;
            let mut frames = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                frames.push(c.read_u16()?);
            }
            frames
        } else {
            Vec::new()
        };
        layers.push(Layer {
            layer_id,
            name: layer_name,
            explicit_keyframes,
            placements,
        });
    }

    Ok(Q0rg {
        q0rg_id,
        name,
        frame_count,
        script,
        layers,
    })
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    fn test_q0v_bytes() -> Vec<u8> {
        let spec = q0video::q0v::Q0vSpec {
            width: 2,
            height: 2,
            fps: 24,
            timeline_frames: 1,
            video: true,
            audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(std::io::Cursor::new(Vec::new()), spec)
            .expect("q0v writer");
        writer
            .write_video_frame(0, b"test-png-payload")
            .expect("q0v frame");
        writer.finish().expect("finish q0v").into_inner()
    }

    fn test_audio_q0v_bytes() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 4_800;
        let spec = q0video::q0v::Q0vSpec {
            width: 0,
            height: 0,
            fps: 48_000,
            timeline_frames: SAMPLE_FRAMES as u32,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(std::io::Cursor::new(Vec::new()), spec)
            .expect("audio q0v writer");
        writer
            .write_audio_pcm_i16(&vec![0; SAMPLE_FRAMES * 2])
            .expect("audio q0v pcm");
        writer.finish().expect("finish audio q0v").into_inner()
    }

    fn legacy_project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "legacy-v3".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 2,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            skew_x: 0.25,
                            skew_y: -0.5,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        }
    }

    #[test]
    fn current_parser_still_reads_q1s_v3() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_SKEW).expect("write v3 fixture");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), Q1S_VERSION_SKEW);
        assert_eq!(parse(&bytes).expect("parse v3 fixture"), project);
    }

    #[test]
    fn q1s_v13_nested_folders_remain_readable_with_identity_fx() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_NESTED_LAYER_FOLDERS)
            .expect("write q1s v13 fixture");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_NESTED_LAYER_FOLDERS
        );
        let decoded = parse(&bytes).expect("parse q1s v13 fixture");
        assert_eq!(decoded, project);
        assert!(decoded.q0rgs[0].layers[0].placements[0].fx.is_identity());
    }

    #[test]
    fn wire_canonicalization_matches_current_q1s_roundtrip() {
        let mut project = legacy_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 2,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        project.assets.swap(0, 1);
        project.q0rgs[0].frame_count = 4;
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements.push(Placement {
            instance_id: 0,
            frame: 3,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        });
        layer.placements.swap(0, 1);
        layer.explicit_keyframes = vec![2, 1];

        let bytes = write(&project).expect("write current q1s");
        let parsed = parse(&bytes).expect("parse current q1s");
        assert_eq!(parsed, canonicalized_for_wire(&project));
    }

    #[test]
    fn wire_canonicalization_preserves_same_frame_display_order() {
        let mut project = legacy_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 2,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements = vec![
            Placement {
                instance_id: 0,
                frame: 2,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        ];
        project.q0rgs[0].frame_count = 3;

        let canonical = canonicalized_for_wire(&project);
        assert!(wire_equivalent(&project, &canonical));
        let targets = canonical.q0rgs[0].layers[0]
            .placements
            .iter()
            .map(|placement| placement.target)
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            vec![Target::Asset(2), Target::Asset(1), Target::Asset(1)]
        );

        let mut reordered_same_frame = canonical.clone();
        reordered_same_frame.q0rgs[0].layers[0]
            .placements
            .swap(0, 1);
        assert!(!wire_equivalent(&canonical, &reordered_same_frame));
    }

    #[test]
    fn current_parser_still_reads_q1s_v4_blank_keyframes() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].explicit_keyframes.push(1);
        let bytes = write_version(&project, Q1S_VERSION_KEYFRAMES).expect("write v4 fixture");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_KEYFRAMES
        );
        assert_eq!(parse(&bytes).expect("parse v4 fixture"), project);
    }

    #[test]
    fn legacy_writer_rejects_unrepresentable_blank_keyframes() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].explicit_keyframes.push(1);
        assert_eq!(
            write_version(&project, Q1S_VERSION_SKEW).expect_err("v3 cannot store blank keys"),
            Error::Validation("legacy q1s versions cannot store explicit keyframes")
        );
    }

    #[test]
    fn legacy_writer_rejects_unrepresentable_layer_folders() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].placements.clear();
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: None,
                collapsed: true,
                hidden: false,
                locked: false,
            },
        );
        assert_eq!(
            write_version(&project, Q1S_VERSION_ASSET_NAMES)
                .expect_err("v5 cannot store layer folders"),
            Error::Validation("legacy q1s versions cannot store layer folders")
        );
    }

    #[test]
    fn current_parser_still_reads_q1s_v5_asset_names_without_layer_metadata() {
        let mut project = legacy_project();
        project
            .asset_names
            .insert(1, "legacy vector name".to_string());
        let bytes = write_version(&project, Q1S_VERSION_ASSET_NAMES).expect("write v5 fixture");
        let parsed = parse(&bytes).expect("parse v5 fixture");
        assert_eq!(parsed, project);
        assert!(parsed.layer_metadata.is_empty());
    }

    #[test]
    fn legacy_writer_rejects_unrepresentable_asset_names() {
        let mut project = legacy_project();
        project.asset_names.insert(1, "named vector".to_string());
        assert_eq!(
            write_version(&project, Q1S_VERSION_KEYFRAMES)
                .expect_err("v4 cannot store asset names"),
            Error::Validation("legacy q1s versions cannot store asset names")
        );
    }

    #[test]
    fn current_parser_still_reads_q1s_v7_q0v_assets() {
        let mut project = legacy_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project
            .asset_names
            .insert(7, "legacy embedded video".to_string());
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        let bytes = write_version(&project, Q1S_VERSION_Q0V_ASSETS).expect("write q1s v7 body");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_Q0V_ASSETS
        );
        assert_eq!(parse(&bytes).expect("parse q1s v7"), project);
    }

    #[test]
    fn q1s_v7_writer_rejects_easing_instead_of_dropping_it() {
        let mut project = legacy_project();
        project.q0rgs[0].frame_count = 3;
        project.q0rgs[0].layers[0].placements[0].tween = Tween::Eased {
            to_frame: 2,
            easing: Easing::Preset {
                family: EasingFamily::Cubic,
                mode: EasingMode::InOut,
            },
        };
        let mut target = project.q0rgs[0].layers[0].placements[0].clone();
        target.frame = 2;
        target.tween = Tween::None;
        project.q0rgs[0].layers[0].placements.push(target);

        assert_eq!(
            write_version(&project, Q1S_VERSION_Q0V_ASSETS)
                .expect_err("q1s v7 cannot store easing"),
            Error::Validation("legacy q1s versions cannot store easing curves")
        );
    }

    #[test]
    fn current_q1s_roundtrip_preserves_embedded_q0v_asset() {
        let mut project = legacy_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project.asset_names.insert(
            7,
            "reference / Р В Р’В Р В РІР‚В Р В Р’В Р РЋРІР‚ВР В Р’В Р СћРІР‚ВР В Р’В Р вЂ™Р’ВµР В Р’В Р РЋРІР‚Сћ".to_string(),
        );
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        let bytes = write(&project).expect("write q0v project");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(parse(&bytes).expect("parse q0v project"), project);
    }

    fn appearance_project() -> ProjectV2 {
        let mut project = legacy_project();
        let Asset::Vector(vector) = &mut project.assets[0] else {
            unreachable!();
        };
        vector.paths = vec![Path {
            closed: true,
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(20.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(20.0, 20.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(0.0, 20.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
        }];
        vector.fill = Some(Rgba {
            r: 240,
            g: 120,
            b: 20,
            a: 230,
        });
        vector.stroke = None;
        project.asset_appearances.insert(
            1,
            VectorAppearance {
                material: VectorMaterial::SoftHalo {
                    radius: 9.5,
                    opacity: 0.42,
                },
                erase_mask: vec![Path {
                    closed: true,
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(7.0, 7.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(13.0, 7.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(13.0, 13.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(7.0, 13.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                }],
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: crate::transform::Affine::IDENTITY,
            },
        );
        project
    }

    #[test]
    fn current_q1s_roundtrip_preserves_vector_appearance_and_mask() {
        let project = appearance_project();
        let bytes = write(&project).expect("write appearance q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse appearance q1s");
        assert_eq!(parsed, project);
        assert_eq!(parsed.asset_appearances[&1], project.asset_appearances[&1]);
    }

    #[test]
    fn q1s_v9_appearance_without_fragments_remains_readable() {
        let project = appearance_project();
        let bytes = write_version(&project, Q1S_VERSION_APPEARANCE_MASKS)
            .expect("write q1s v9 appearance fixture");
        let parsed = parse(&bytes).expect("parse q1s v9 appearance fixture");
        assert_eq!(parsed, project);
        assert!(parsed.asset_appearances[&1].material_source.is_empty());
        assert!(parsed.asset_appearances[&1].clip_mask.is_empty());
    }

    #[test]
    fn current_q1s_roundtrip_preserves_post_material_fragments() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source;
        appearance.clip_mask = vec![Path {
            closed: true,
            anchors: vec![
                Anchor {
                    point: Vec2::new(10.0, -10.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(30.0, -10.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(30.0, 30.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(10.0, 30.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
        }];
        let bytes = write(&project).expect("write q1s v10 fragments");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(parse(&bytes).expect("parse current q1s fragments"), project);
    }

    #[test]
    fn q1s_v10_fragment_body_remains_readable_with_identity_field() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source.clone();
        appearance.clip_mask = source;
        appearance.field_transform = Affine::IDENTITY;
        let bytes = write_version(&project, Q1S_VERSION_APPEARANCE_FRAGMENTS)
            .expect("write q1s v10 fragment fixture");
        let parsed = parse(&bytes).expect("parse q1s v10 fragment fixture");
        assert_eq!(parsed, project);
        assert_eq!(
            parsed.asset_appearances[&1].field_transform,
            Affine::IDENTITY
        );
    }

    #[test]
    fn current_q1s_roundtrip_preserves_appearance_field_affine() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let field_transform = Affine {
            a11: 0.75,
            a12: 0.5,
            a21: -0.25,
            a22: 1.2,
            tx: 13.0,
            ty: -7.0,
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source.clone();
        appearance.clip_mask = source;
        appearance.field_transform = field_transform;
        let bytes = write(&project).expect("write affine q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse affine q1s");
        assert_eq!(
            parsed.asset_appearances[&1].field_transform,
            field_transform
        );
        assert_eq!(parsed, project);
    }

    #[test]
    fn q1s_v10_writer_rejects_transformed_field_instead_of_dropping_it() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source;
        appearance.field_transform = Affine {
            a11: 1.0,
            a12: 0.4,
            a21: 0.0,
            a22: 1.0,
            tx: 0.0,
            ty: 0.0,
        };
        assert_eq!(
            write_version(&project, Q1S_VERSION_APPEARANCE_FRAGMENTS)
                .expect_err("q1s v10 cannot store field affine"),
            Error::Validation("q1s v10 cannot store transformed appearance fields")
        );
    }

    #[test]
    fn q1s_v9_writer_rejects_post_material_fragments_instead_of_dropping_them() {
        let mut project = appearance_project();
        project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .material_source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            write_version(&project, Q1S_VERSION_APPEARANCE_MASKS)
                .expect_err("q1s v9 cannot store appearance fragments"),
            Error::Validation("q1s v9 cannot store post-material appearance fragments")
        );
    }

    #[test]
    fn validation_rejects_invalid_appearance_field_affine() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source.clone();
        appearance.field_transform.a12 = f32::NAN;
        assert_eq!(
            validate(&project).expect_err("non-finite field affine must fail"),
            Error::Validation("appearance field transform must be finite")
        );

        let mut project = appearance_project();
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = source.clone();
        appearance.field_transform = Affine {
            a11: 1.0,
            a12: 2.0,
            a21: 0.5,
            a22: 1.0,
            tx: 0.0,
            ty: 0.0,
        };
        assert_eq!(
            validate(&project).expect_err("singular field affine must fail"),
            Error::Validation("appearance field transform must be invertible")
        );

        let mut project = appearance_project();
        project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .field_transform = Affine {
            a11: 1.0,
            a12: 0.25,
            a21: 0.0,
            a22: 1.0,
            tx: 0.0,
            ty: 0.0,
        };
        assert_eq!(
            validate(&project).expect_err("field affine without frozen source must fail"),
            Error::Validation("transformed appearance field requires a frozen material source")
        );
    }

    #[test]
    fn validation_rejects_invalid_post_material_fragment_paths() {
        let mut project = appearance_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .material_source = source.clone();
        project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .material_source[0]
            .anchors[0]
            .point
            .x = f32::NAN;
        assert_eq!(
            validate(&project).expect_err("non-finite material source must fail"),
            Error::Validation("appearance anchors must be finite")
        );

        let mut project = appearance_project();
        let mut clip = source[0].clone();
        clip.closed = false;
        project.asset_appearances.get_mut(&1).unwrap().clip_mask = vec![clip];
        assert_eq!(
            validate(&project).expect_err("open post-material clip must fail"),
            Error::Validation("appearance paths must be closed with >= 3 anchors")
        );
    }

    #[test]
    fn q1s_v8_remains_readable_and_has_no_appearance_state() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_EASING).expect("write q1s v8 fixture");
        let parsed = parse(&bytes).expect("parse q1s v8 fixture");
        assert_eq!(parsed, project);
        assert!(parsed.asset_appearances.is_empty());
    }

    #[test]
    fn q1s_v8_writer_rejects_appearance_instead_of_dropping_it() {
        let project = appearance_project();
        assert_eq!(
            write_version(&project, Q1S_VERSION_EASING)
                .expect_err("q1s v8 cannot store appearance masks"),
            Error::Validation("legacy q1s versions cannot store vector appearance masks")
        );
    }

    #[test]
    fn q1s_v6_writer_rejects_q0v_assets_instead_of_losing_them() {
        let mut project = legacy_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        assert_eq!(
            write_version(&project, Q1S_VERSION_LAYER_FOLDERS).expect_err("v6 cannot store q0v"),
            Error::Validation("legacy q1s versions cannot store q0v assets")
        );
    }
    #[test]
    fn layer_visibility_and_lock_round_trip_and_inherit_from_folder() {
        let mut project = legacy_project();
        project.q0rgs[0].layers.push(Layer {
            layer_id: 9,
            name: "Folder".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                parent_folder_id: Some(9),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 9),
            LayerMetadata {
                kind: LayerKind::Folder,
                hidden: true,
                locked: true,
                ..Default::default()
            },
        );

        assert!(!project.layer_is_visible(1, 1));
        assert!(project.layer_is_locked(1, 1));

        let bytes = write(&project).expect("write layer state");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse layer state");
        assert_eq!(parsed, project);
        assert!(!parsed.layer_is_visible(1, 1));
        assert!(parsed.layer_is_locked(1, 1));
    }

    #[test]
    fn nested_layer_folders_round_trip_and_inherit_state_recursively() {
        let mut project = legacy_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                name: "leaf".into(),
                explicit_keyframes: vec![0],
                placements: project.q0rgs[0].layers[0].placements.clone(),
            },
            Layer {
                layer_id: 8,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 9,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                parent_folder_id: Some(8),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 8),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: Some(9),
                locked: true,
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 9),
            LayerMetadata {
                kind: LayerKind::Folder,
                hidden: true,
                ..Default::default()
            },
        );

        validate(&project).expect("nested folders validate");
        assert_eq!(project.layer_folder_depth(1, 1), 2);
        assert!(project.layer_is_descendant_of(1, 1, 8));
        assert!(project.layer_is_descendant_of(1, 1, 9));
        assert!(!project.layer_is_visible(1, 1));
        assert!(project.layer_is_locked(1, 1));

        let bytes = write(&project).expect("write nested q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse nested q1s");
        assert_eq!(parsed, project);

        assert_eq!(
            write_version(&project, Q1S_VERSION_LAYER_STATE)
                .expect_err("q1s v12 cannot represent nested folders"),
            Error::Validation("legacy q1s versions cannot store nested layer folders")
        );
    }

    #[test]
    fn nested_layer_folder_validation_rejects_cycles_and_noncontiguous_subtrees() {
        let mut project = legacy_project();
        project.q0rgs[0].layers.push(Layer {
            layer_id: 8,
            name: "folder a".into(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.q0rgs[0].layers.push(Layer {
            layer_id: 9,
            name: "folder b".into(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.layer_metadata.insert(
            LayerKey::new(1, 8),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: Some(9),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 9),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: Some(8),
                ..Default::default()
            },
        );
        assert_eq!(
            validate(&project).expect_err("folder cycle must fail"),
            Error::Validation("layer folder parent cycle")
        );

        let mut project = legacy_project();
        project.q0rgs[0].layers = vec![
            project.q0rgs[0].layers[0].clone(),
            Layer {
                layer_id: 7,
                name: "unrelated".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
            Layer {
                layer_id: 8,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 9,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                parent_folder_id: Some(8),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 8),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: Some(9),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 9),
            LayerMetadata {
                kind: LayerKind::Folder,
                ..Default::default()
            },
        );
        assert_eq!(
            validate(&project).expect_err("unrelated row splits folder subtree"),
            Error::Validation("folder child layers must immediately precede their folder")
        );
    }

    #[test]
    fn q1s_v12_layer_state_remains_readable_without_nested_folders() {
        let mut project = legacy_project();
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                hidden: true,
                locked: true,
                ..Default::default()
            },
        );
        let bytes = write_version(&project, Q1S_VERSION_LAYER_STATE).expect("write q1s v12");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_LAYER_STATE
        );
        let parsed = parse(&bytes).expect("parse q1s v12");
        assert_eq!(parsed, project);
        assert!(!parsed.layer_is_visible(1, 1));
        assert!(parsed.layer_is_locked(1, 1));
    }

    #[test]
    fn q1s_v11_defaults_layers_to_visible_unlocked_and_rejects_new_state_on_write() {
        let project = legacy_project();
        let bytes =
            write_version(&project, Q1S_VERSION_APPEARANCE_AFFINE).expect("write q1s v11 fixture");
        let parsed = parse(&bytes).expect("parse q1s v11 fixture");
        assert!(parsed.layer_is_visible(1, 1));
        assert!(!parsed.layer_is_locked(1, 1));

        let mut with_state = project;
        with_state.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                hidden: true,
                ..Default::default()
            },
        );
        assert_eq!(
            write_version(&with_state, Q1S_VERSION_APPEARANCE_AFFINE)
                .expect_err("q1s v11 must not silently drop visibility"),
            Error::Validation("legacy q1s versions cannot store layer visibility or locks")
        );
    }
    fn add_test_rig(project: &mut ProjectV2) {
        project.q0rgs[0].layers[0].placements[0].instance_id = 1;
        let mut rig = RigAsset {
            asset_id: 400,
            owner_q0rg_id: 1,
            nodes: vec![
                RigNode {
                    node_id: 1,
                    name: "root".into(),
                    parent: None,
                    rest: Transform2D::IDENTITY,
                    length: 20.0,
                    binding: Some(RigBinding {
                        instance_id: 1,
                        bind_offset: Affine::IDENTITY,
                    }),
                },
                RigNode {
                    node_id: 2,
                    name: "tip".into(),
                    parent: Some(1),
                    rest: Transform2D {
                        tx: 20.0,
                        ..Transform2D::IDENTITY
                    },
                    length: 0.0,
                    binding: None,
                },
            ],
            controls: vec![RigControl {
                control_id: 1,
                name: "root rotate".into(),
                kind: RigControlKind::Rotation,
                target_node: Some(1),
                rest_x: 0.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -std::f32::consts::PI,
                max_value: std::f32::consts::PI,
                public_in_simple: true,
            }],
            constraints: vec![RigConstraint::RotationLimit {
                constraint_id: 1,
                node_id: 1,
                min_radians: -1.5,
                max_radians: 1.5,
            }],
            channels: vec![RigChannel {
                property: RigPropertyRef::ControlValue(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 0.25,
                    easing: Easing::Linear,
                }],
            }],
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            pose_drivers: Vec::new(),
            mirror_pairs: Vec::new(),
            variants: Vec::new(),
        };
        rig.controls.push(RigControl {
            control_id: 2,
            name: "master".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.35,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.drivers.push(RigDriver {
            driver_id: 1,
            source_control: 2,
            source_min: 0.0,
            source_max: 1.0,
            target: RigPropertyRef::NodeRotation(2),
            target_min: -0.5,
            target_max: 0.75,
        });
        rig.poses.push(RigPosePreset {
            pose_id: 1,
            name: "tilt".into(),
            values: vec![
                RigPoseValue {
                    property: RigPropertyRef::ControlValue(2),
                    value: 0.8,
                },
                RigPoseValue {
                    property: RigPropertyRef::NodeRotation(2),
                    value: 0.3,
                },
            ],
        });
        project.assets.push(Asset::Rig(rig));
    }

    #[test]
    fn current_q1s_roundtrip_preserves_rigging() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let bytes = write(&project).expect("write rigged q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse rigged q1s");
        assert_eq!(parsed, project);
        let rig = parsed
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig asset");
        assert_eq!(rig.nodes.len(), 2);
        assert_eq!(rig.controls.len(), 2);
        assert_eq!(rig.channels[0].keys[0].value, 0.25);
        assert_eq!(rig.drivers.len(), 1);
        assert_eq!(rig.poses.len(), 1);
        assert_eq!(project.q0rgs[0].layers[0].placements[0].instance_id, 1);
        assert_eq!(rig.nodes[0].binding.unwrap().instance_id, 1);
    }

    #[test]
    fn q1s_v14_remains_readable_without_rigging() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_PLACEMENT_FX).expect("write q1s v14");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_PLACEMENT_FX
        );
        assert_eq!(parse(&bytes).expect("parse q1s v14"), project);
    }

    #[test]
    fn q1s_v14_writer_rejects_rigging_instead_of_dropping_it() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        assert_eq!(
            write_version(&project, Q1S_VERSION_PLACEMENT_FX)
                .expect_err("q1s v14 cannot store rigs"),
            Error::Validation("legacy q1s versions cannot store rigging")
        );
    }

    #[test]
    fn validation_rejects_rig_key_past_owner_timeline() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .unwrap();
        rig.channels[0].keys[0].frame = project.q0rgs[0].frame_count;
        assert_eq!(
            validate(&project).expect_err("rig key must stay in q0rg"),
            Error::Validation("rig key is out of owner q0rg frame bounds")
        );
    }

    #[test]
    fn legacy_identity_bootstrap_is_deterministic_and_tracks_target_occurrence() {
        let mut project = legacy_project();
        project.q0rgs[0].frame_count = 4;
        project.q0rgs[0].layers[0].placements = vec![
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D {
                    tx: 20.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 2,
                target: Target::Asset(1),
                transform: Transform2D {
                    tx: 5.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 2,
                target: Target::Asset(1),
                transform: Transform2D {
                    tx: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            },
        ];
        let mut a = project.clone();
        let mut b = project;
        assign_missing_instance_ids(&mut a).expect("bootstrap a");
        assign_missing_instance_ids(&mut b).expect("bootstrap b");
        assert_eq!(a, b, "legacy identity migration must be deterministic");
        let ids = a.q0rgs[0].layers[0]
            .placements
            .iter()
            .map(|placement| placement.instance_id)
            .collect::<Vec<_>>();
        assert_ne!(ids[0], 0);
        assert_ne!(ids[1], 0);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(ids[0], ids[2]);
        assert_eq!(ids[1], ids[3]);
    }

    #[test]
    fn q1s_v15_basic_rig_roundtrip_remains_readable() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let bytes = write_version(&project, Q1S_VERSION_RIGGING).expect("write basic rig v15");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_RIGGING
        );
        assert_eq!(parse(&bytes).expect("parse basic rig v15"), project);
    }

    #[test]
    fn q1s_v15_rejects_pro_rig_extensions_instead_of_dropping_them() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        rig.channels.push(RigChannel {
            property: RigPropertyRef::NodeTx(1),
            keys: vec![RigKey {
                frame: 0,
                value: 2.0,
                easing: Easing::Linear,
            }],
        });
        assert_eq!(
            write_version(&project, Q1S_VERSION_RIGGING).expect_err("v15 must reject pro rig"),
            Error::Validation("q1s v15 cannot store pro rig extensions")
        );
    }

    #[test]
    fn current_q1s_roundtrips_pro_controls_constraints_and_node_channels() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        rig.controls.push(RigControl {
            control_id: 3,
            name: "aim target".into(),
            kind: RigControlKind::Position2D,
            target_node: None,
            rest_x: 40.0,
            rest_y: 10.0,
            rest_value: 0.0,
            min_value: -1000.0,
            max_value: 1000.0,
            public_in_simple: true,
        });
        rig.controls.push(RigControl {
            control_id: 4,
            name: "toggle".into(),
            kind: RigControlKind::Toggle,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.constraints.extend([
            RigConstraint::PositionLimit {
                constraint_id: 2,
                node_id: 1,
                min_x: -10.0,
                max_x: 10.0,
                min_y: -20.0,
                max_y: 20.0,
            },
            RigConstraint::Aim {
                constraint_id: 3,
                node_id: 1,
                target_control: 3,
                angle_offset: 0.1,
                weight: 0.8,
            },
            RigConstraint::Transform {
                constraint_id: 4,
                node_id: 2,
                target_node: 1,
                position_weight: 0.25,
                rotation_weight: 0.5,
            },
            RigConstraint::Distance {
                constraint_id: 5,
                node_id: 2,
                target_control: 3,
                min_distance: 5.0,
                max_distance: 80.0,
                weight: 0.7,
            },
        ]);
        rig.channels.extend([
            RigChannel {
                property: RigPropertyRef::NodeTx(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 2.0,
                    easing: Easing::Linear,
                }],
            },
            RigChannel {
                property: RigPropertyRef::NodeScaleX(2),
                keys: vec![RigKey {
                    frame: 0,
                    value: 1.2,
                    easing: Easing::Linear,
                }],
            },
        ]);
        rig.drivers.push(RigDriver {
            driver_id: 2,
            source_control: 4,
            source_min: 0.0,
            source_max: 1.0,
            target: RigPropertyRef::NodeScaleY(1),
            target_min: 1.0,
            target_max: 1.5,
        });
        let bytes = write(&project).expect("write pro rig current q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(parse(&bytes).expect("parse pro rig current q1s"), project);
    }

    #[test]
    fn validation_rejects_transform_constraint_cycle_with_parent_graph() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        rig.constraints.push(RigConstraint::Transform {
            constraint_id: 2,
            node_id: 1,
            target_node: 2,
            position_weight: 1.0,
            rotation_weight: 1.0,
        });
        assert_eq!(
            validate(&project).expect_err("transform dependency cycle must fail"),
            Error::Validation("rig constraint dependency cycle")
        );
    }

    fn add_test_bend_deformer(project: &mut ProjectV2) {
        project.q0rgs[0].layers[0].placements.push(Placement {
            instance_id: 2,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 8.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
        });
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        for (control_id, x, y) in [(3, 8.0, 0.0), (4, 13.0, 4.0), (5, 18.0, 0.0)] {
            rig.controls.push(RigControl {
                control_id,
                name: format!("bend {control_id}"),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: x,
                rest_y: y,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            });
        }
        rig.deformers.push(RigDeformer::Bend {
            deformer_id: 1,
            instance_id: 2,
            asset_id: 1,
            bind_transform: Affine {
                tx: 8.0,
                ..Affine::IDENTITY
            },
            axis_start: Vec2::new(0.0, 0.0),
            axis_end: Vec2::new(10.0, 0.0),
            start_control: 3,
            middle_control: 4,
            end_control: 5,
        });
    }

    #[test]
    fn current_q1s_roundtrip_preserves_nonempty_rig_deformer() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        add_test_bend_deformer(&mut project);
        let bytes = write(&project).expect("write deformer q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(parse(&bytes).expect("parse deformer q1s"), project);
    }

    #[test]
    fn q1s_v16_rejects_deformers_instead_of_dropping_them() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        add_test_bend_deformer(&mut project);
        assert_eq!(
            write_version(&project, Q1S_VERSION_RIG_PRO).expect_err("v16 cannot store deformers"),
            Error::Validation("q1s v16 cannot store rig deformers")
        );
    }

    fn add_test_pose_variant_features(project: &mut ProjectV2) {
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        rig.controls.push(RigControl {
            control_id: 3,
            name: "pose source".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.pose_drivers.push(RigPoseDriver {
            driver_id: 1,
            source_control: 3,
            pose_id: 1,
            source_min: 0.0,
            source_max: 1.0,
            weight_min: 0.1,
            weight_max: 0.9,
            mode: RigPoseBlendMode::Additive,
        });
        rig.mirror_pairs.push(RigMirrorPair {
            left: RigPropertyRef::NodeRotation(1),
            right: RigPropertyRef::NodeRotation(2),
            multiplier: -1.0,
            offset: 0.25,
        });
        rig.variants.push(RigVariantSet {
            variant_id: 1,
            name: "mouth".into(),
            instance_id: 1,
            source_control: 3,
            choices: vec![
                RigVariantChoice {
                    name: "rest".into(),
                    target: Target::Asset(1),
                },
                RigVariantChoice {
                    name: "alt".into(),
                    target: Target::Asset(1),
                },
            ],
        });
    }

    #[test]
    fn current_q1s_roundtrip_preserves_pose_drivers_mirror_pairs_and_variants() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        add_test_pose_variant_features(&mut project);
        let bytes = write(&project).expect("write v18 rig extensions");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(
            parse(&bytes).expect("parse current rig extensions"),
            project
        );
    }

    #[test]
    fn current_q1s_roundtrip_preserves_audio_clip_gain_and_mute() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].placements[0].fx.audio_gain = 0.375;
        project.q0rgs[0].layers[0].placements[0].fx.audio_muted = true;
        let bytes = write(&project).expect("write audio clip fx");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        assert_eq!(parse(&bytes).expect("parse audio clip fx"), project);
    }

    #[test]
    fn current_q1s_roundtrip_preserves_timeline_audio_clip() {
        let mut project = legacy_project();
        project.meta.fps = 24;
        project.assets = vec![Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: test_audio_q0v_bytes(),
        })];
        project.q0rgs[0].frame_count = 20;
        project.q0rgs[0].layers[0].placements.clear();
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.audio_clips.push(AudioClip {
            q0rg_id: 1,
            layer_id: 1,
            start_frame: 3,
            asset_id: 77,
            gain: 0.375,
            muted: true,
        });

        let bytes = write(&project).expect("write timeline audio q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_CURRENT
        );
        let parsed = parse(&bytes).expect("parse timeline audio q1s");
        assert_eq!(parsed, project);
        assert_eq!(parsed.audio_clips, project.audio_clips);
    }

    #[test]
    fn q1s_v19_remains_readable_without_timeline_audio_section() {
        let mut project = legacy_project();
        let bytes =
            write_version(&project, Q1S_VERSION_AUDIO_CLIP_FX).expect("write q1s v19 fixture");
        let parsed = parse(&bytes).expect("parse q1s v19 fixture");
        assert_eq!(parsed, project);
        assert!(parsed.audio_clips.is_empty());

        project.assets = vec![Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: test_audio_q0v_bytes(),
        })];
        project.q0rgs[0].frame_count = 20;
        project.q0rgs[0].layers[0].placements.clear();
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.audio_clips.push(AudioClip {
            q0rg_id: 1,
            layer_id: 1,
            start_frame: 0,
            asset_id: 77,
            gain: 1.0,
            muted: false,
        });
        assert_eq!(
            write_version(&project, Q1S_VERSION_AUDIO_CLIP_FX)
                .expect_err("q1s v19 must reject timeline audio clips"),
            Error::Validation("q1s v19 cannot store timeline audio clips")
        );
    }

    #[test]
    fn q1s_v18_rejects_audio_clip_fx_instead_of_dropping_them() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].placements[0].fx.audio_gain = 0.5;
        assert_eq!(
            write_version(&project, Q1S_VERSION_RIG_POSE_VARIANTS)
                .expect_err("v18 must reject audio clip fx"),
            Error::Validation("q1s v18 cannot store audio clip gain or mute")
        );
    }

    #[test]
    fn q1s_v18_remains_readable_with_default_audio_clip_fx() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_RIG_POSE_VARIANTS).expect("write q1s v18");
        let parsed = parse(&bytes).expect("parse q1s v18");
        assert_eq!(parsed, project);
        let fx = parsed.q0rgs[0].layers[0].placements[0].fx;
        assert_eq!(fx.audio_gain, 1.0);
        assert!(!fx.audio_muted);
    }

    #[test]
    fn q1s_v17_rejects_pose_variant_features_instead_of_dropping_them() {
        let mut project = legacy_project();
        add_test_rig(&mut project);
        add_test_pose_variant_features(&mut project);
        assert_eq!(
            write_version(&project, Q1S_VERSION_RIG_DEFORMERS)
                .expect_err("v17 must reject phase h"),
            Error::Validation("q1s v17 cannot store pose drivers, mirror pairs or variants")
        );
    }

    #[test]
    fn variant_q0rg_choice_participates_in_cycle_validation() {
        let mut project = legacy_project();
        project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "child".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 2,
                name: "child".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            }],
        });
        project.q0rgs[1].layers[0].placements.push(Placement {
            instance_id: 99,
            frame: 0,
            target: Target::Q0rg(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        });
        add_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .unwrap();
        rig.variants.push(RigVariantSet {
            variant_id: 1,
            name: "cycle".into(),
            instance_id: 1,
            source_control: 2,
            choices: vec![RigVariantChoice {
                name: "child".into(),
                target: Target::Q0rg(2),
            }],
        });
        assert_eq!(
            validate(&project).expect_err("variant cycle must reject"),
            Error::Validation("q0rg cycle detected")
        );
    }

    #[test]
    fn current_q1s_roundtrip_preserves_project_runtime_metadata() {
        let mut project = legacy_project();
        project.q0rgs[0].layers[0].placements[0].instance_id = 42;
        project.runtime.project_graph.nodes = vec![
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "logic".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("scripts/logic.q0l".into()),
            },
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: Some(1),
                alias: "ui".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::Embedded(vec![1, 2, 3, 4]),
            },
        ];
        project.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 0,
            source: "gostop! 0\n".into(),
        });
        project
            .runtime
            .instance_names
            .insert(InstanceKey::new(1, 42), "hero".into());

        let bytes = write(&project).expect("write project runtime q1s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_PROJECT_RUNTIME
        );
        assert_eq!(parse(&bytes).expect("parse project runtime q1s"), project);
    }

    #[test]
    fn q1s_v20_remains_readable_with_empty_project_runtime_metadata() {
        let project = legacy_project();
        let bytes = write_version(&project, Q1S_VERSION_AUDIO_TIMELINE_CLIPS)
            .expect("write q1s v20 fixture");
        let parsed = parse(&bytes).expect("parse q1s v20 fixture");
        assert_eq!(parsed, project);
        assert_eq!(parsed.runtime, ProjectRuntimeData::default());
    }

    #[test]
    fn project_runtime_validation_rejects_dependency_cycles_and_duplicate_aliases() {
        let mut project = legacy_project();
        project.runtime.project_graph.nodes = vec![
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: Some(2),
                alias: "a".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("a.q0l".into()),
            },
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: Some(1),
                alias: "b".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::External("b.q0s".into()),
            },
        ];
        assert_eq!(
            validate(&project).expect_err("dependency cycle must fail"),
            Error::Validation("project dependency cycle detected")
        );

        project.runtime.project_graph.nodes = vec![
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "same".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("a.q0l".into()),
            },
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: None,
                alias: "same".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::External("b.q0s".into()),
            },
        ];
        assert_eq!(
            validate(&project).expect_err("duplicate sibling alias must fail"),
            Error::Validation("project dependency aliases must be unique among siblings")
        );
    }
}
