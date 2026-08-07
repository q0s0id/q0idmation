//! .q1s format v2/v3 Р Р†Р вЂљРІР‚Сњ vector shapes + recursive q0rg (MovieClip) symbols.
//!
//! Binary layout (little-endian):
//!
//!   Header:
//!     [4]   magic "Q1S\0"
//!     [2]   version = 2 (legacy) | 3 (skew) | 4 (blank keyframes) | 5 (asset names) | 6 (layer folders) | 7 (q0v assets) | 8 (easing) | 9 (vector appearance masks) | 10 (post-material appearance fragments) | 11 (appearance field affine)
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
//!         v2: transform [20]: tx ty sx sy rot (5 Р вЂњРІР‚вЂќ f32)
//!         v3+: transform [28]: tx ty sx sy rot skew_x skew_y (7 Р вЂњРІР‚вЂќ f32)
//!         tween: [1] kind (0=none, 1=linear, 2=eased), [2 if motion] to_frame,
//!           v8 eased: [1] easing kind + payload
//!       v4+: [2] explicit_keyframe_count, explicit_keyframes[]: [2] frame
//!   v6 layer metadata: [2] entry_count, then entries:
//!     [2] q0rg_id, [2] layer_id, [1] kind, [1] parent flag, [2 if present] parent id, [1] collapsed
//!   v9 vector appearance metadata: [2] entry_count, then entries:
//!     [2] asset_id, [1] material kind, material payload, [2] erase path count, erase paths[]
//!   v10 post-material fragments append per appearance:
//!     [2] material source path count, source paths[], [2] clip path count, clip paths[]
//!
//! Reading: v2 through v11 are accepted; v2 placements get skew_x/y = 0,
//! v2/v3 layers get no explicit blank-keyframe markers, v2-v4 assets
//! keep deterministic default labels, and v2-v5 projects have ordinary
//! top-level layers without folders.
//!   v11 transformed appearance fields append per appearance:
//!     [24] field affine a11 a12 a21 a22 tx ty (6 x f32)
//! Writing: always v11 (see `Q1S_VERSION_CURRENT`).

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
pub const Q1S_VERSION_CURRENT: u16 = Q1S_VERSION_APPEARANCE_AFFINE;
/// Kept as an alias so external code that imported the v2-era constant keeps
/// compiling. It now means "the current write-out version".
pub const Q1S_V2_VERSION: u16 = Q1S_VERSION_CURRENT;

/// Maximum number of q0rg-to-q0rg edges below a render root.
///
/// A root q0rg is at depth 0, so a chain containing nine q0rgs (eight nested
/// edges) is valid. Validation and every renderer must use this same limit so
/// a project can never validate successfully and then lose deeper content.
pub const MAX_Q0RG_NESTING_DEPTH: u8 = 8;

const ASSET_KIND_BITMAP: u8 = 1;
const ASSET_KIND_VECTOR: u8 = 2;
const ASSET_KIND_Q0V: u8 = 3;
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
    /// load Р Р†Р вЂљРІР‚Сњ see the `flag == 1` vs `flag == 2` branches in the
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

#[derive(Debug, Clone, PartialEq)]
pub enum Asset {
    Bitmap(BitmapAsset),
    Vector(VectorAsset),
    Q0v(Q0vAsset),
}

impl Asset {
    pub fn id(&self) -> u16 {
        match self {
            Asset::Bitmap(b) => b.asset_id,
            Asset::Vector(v) => v.asset_id,
            Asset::Q0v(v) => v.asset_id,
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

#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub frame: u16,
    pub target: Target,
    pub transform: Transform2D,
    pub tween: Tween,
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
    pub q0rgs: Vec<Q0rg>,
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
pub fn wire_equivalent(left: &ProjectV2, right: &ProjectV2) -> bool {
    if left.meta != right.meta
        || left.asset_names != right.asset_names
        || left.asset_appearances != right.asset_appearances
        || left.layer_metadata != right.layer_metadata
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
                q0video::q0v::Q0vFile::parse(v.bytes.clone())
                    .map_err(|_| Error::Validation("q0v asset payload is invalid"))?;
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
            for p in &layer.placements {
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
                match p.target {
                    Target::Asset(id) => {
                        if !asset_ids.contains(&id) {
                            return Err(Error::Validation("placement references unknown asset_id"));
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
                if metadata.parent_folder_id.is_some() {
                    return Err(Error::Validation("layer folders cannot be nested"));
                }
                if !layer.explicit_keyframes.is_empty() || !layer.placements.is_empty() {
                    return Err(Error::Validation("layer folder must not contain keyframes"));
                }
            }
            LayerKind::Normal => {
                if metadata.collapsed {
                    return Err(Error::Validation("ordinary layer cannot be collapsed"));
                }
                if let Some(parent_id) = metadata.parent_folder_id {
                    let parent = project.layer_metadata(key.q0rg_id, parent_id);
                    if parent.kind != LayerKind::Folder {
                        return Err(Error::Validation("layer parent must reference a folder"));
                    }
                }
            }
        }
    }

    for q0rg in &project.q0rgs {
        let mut pending_folder = None;
        for layer in &q0rg.layers {
            let metadata = project.layer_metadata(q0rg.q0rg_id, layer.layer_id);
            match (metadata.kind, metadata.parent_folder_id) {
                (LayerKind::Normal, Some(parent_id)) => match pending_folder {
                    Some(expected) if expected != parent_id => {
                        return Err(Error::Validation("folder child layers must be contiguous"));
                    }
                    None => pending_folder = Some(parent_id),
                    _ => {}
                },
                (LayerKind::Normal, None) => {
                    if pending_folder.is_some() {
                        return Err(Error::Validation(
                            "folder child layers must immediately precede their folder",
                        ));
                    }
                }
                (LayerKind::Folder, None) => {
                    if pending_folder.is_some_and(|expected| expected != layer.layer_id) {
                        return Err(Error::Validation(
                            "folder child layers must immediately precede their folder",
                        ));
                    }
                    pending_folder = None;
                }
                (LayerKind::Folder, Some(_)) => unreachable!("validated folder parent"),
            }
        }
        if pending_folder.is_some() {
            return Err(Error::Validation(
                "folder child layers must immediately precede their folder",
            ));
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
    validate_q0rg_graph(&project.q0rgs, &q0rg_index)?;

    Ok(())
}

fn validate_q0rg_graph(q0rgs: &[Q0rg], index: &HashMap<u16, usize>) -> Result<(), Error> {
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
        && version != Q1S_VERSION_CURRENT
    {
        return Err(Error::UnsupportedVersion(version));
    }
    if version < Q1S_VERSION_LAYER_FOLDERS && !project.layer_metadata.is_empty() {
        return Err(Error::Validation(
            "legacy q1s versions cannot store layer folders",
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
    }
    Ok(())
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
            if layer_metadata
                .insert(
                    key,
                    LayerMetadata {
                        kind,
                        parent_folder_id,
                        collapsed,
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
        q0rgs,
    };
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
                // Legacy (pre-cap) stroke Р Р†Р вЂљРІР‚Сњ implicit round cap.
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
        other => return Err(Error::InvalidAssetKind(other)),
    };
    Ok((asset, asset_name))
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
            placements.push(Placement {
                frame,
                target,
                transform,
                tween,
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
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            skew_x: 0.25,
                            skew_y: -0.5,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
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
            frame: 3,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
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
                frame: 2,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
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
            "reference / Р В Р вЂ Р В РЎвЂР В РўвЂР В Р’ВµР В РЎвЂў".to_string(),
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
}
