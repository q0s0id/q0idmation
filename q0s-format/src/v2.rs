//! .q1s format v2/v3 — vector shapes + recursive q0rg (MovieClip) symbols.
//!
//! Binary layout (little-endian):
//!
//!   Header:
//!     [4]   magic "Q1S\0"
//!     [2]   version = 2 (legacy) | 3 (skew) | 4 (blank keyframes) | 5 (asset names) | 6 (layer folders) | 7 (q0v assets)
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
//!         v2: transform [20]: tx ty sx sy rot (5 × f32)
//!         v3+: transform [28]: tx ty sx sy rot skew_x skew_y (7 × f32)
//!         tween: [1] kind (0=none, 1=linear), [2 if linear] to_frame
//!       v4+: [2] explicit_keyframe_count, explicit_keyframes[]: [2] frame
//!   v6 layer metadata: [2] entry_count, then entries:
//!     [2] q0rg_id, [2] layer_id, [1] kind, [1] parent flag, [2 if present] parent id, [1] collapsed
//!
//! Reading: v2 through v7 are accepted; v2 placements get skew_x/y = 0,
//! v2/v3 layers get no explicit blank-keyframe markers, v2-v4 assets
//! keep deterministic default labels, and v2-v5 projects have ordinary
//! top-level layers without folders.
//! Writing: always v7 (see `Q1S_VERSION_CURRENT`).

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
pub const Q1S_VERSION_CURRENT: u16 = Q1S_VERSION_Q0V_ASSETS;
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
const LAYER_KIND_NORMAL: u8 = 0;
const LAYER_KIND_FOLDER: u8 = 1;

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
    /// load — see the `flag == 1` vs `flag == 2` branches in the
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tween {
    None,
    Linear { to_frame: u16 },
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
    /// Sparse structural metadata for timeline layers. Missing entries are
    /// ordinary top-level layers, preserving legacy project behaviour.
    pub layer_metadata: HashMap<LayerKey, LayerMetadata>,
    pub q0rgs: Vec<Q0rg>,
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
                if let Tween::Linear { to_frame } = p.tween {
                    if to_frame <= p.frame {
                        return Err(Error::Validation("tween to_frame must be > frame"));
                    }
                    if to_frame >= q0rg.frame_count {
                        return Err(Error::Validation(
                            "tween to_frame is out of q0rg frame_count bounds",
                        ));
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
                // Legacy (pre-cap) stroke — implicit round cap.
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

fn read_optional_vec2(c: &mut Cursor) -> Result<Option<Vec2>, Error> {
    match c.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(Vec2::new(c.read_f32()?, c.read_f32()?))),
        _ => Err(Error::Validation("invalid optional-vec2 flag")),
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
    fn current_q1s_roundtrip_preserves_embedded_q0v_asset() {
        let mut project = legacy_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project
            .asset_names
            .insert(7, "reference / видео".to_string());
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        let bytes = write(&project).expect("write q0v project");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q1S_VERSION_Q0V_ASSETS
        );
        assert_eq!(parse(&bytes).expect("parse q0v project"), project);
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
