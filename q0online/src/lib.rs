//! Shared room protocol. The server owns the document, permissions and operation order.
//! Clients submit conditional entity changes; they never declare their own authorization scope.
use q0s_format::v2::{
    self, Asset, Layer, LayerKey, LayerMetadata, LibraryItemReference, ProjectMeta,
    ProjectRuntimeData, ProjectV2, VectorAppearance,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod merge;
pub use merge::{merge_projects, merge_projects_trusted};

pub const PROTOCOL_VERSION: u32 = 1;
pub const STATE_VERSION: u32 = 1;
pub type UserId = i64;
pub type Revision = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rank {
    Viewer,
    Editor,
    Admin,
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    Project,
    Layer { q0rg_id: u16, layer_id: u16 },
    LayerFolder { q0rg_id: u16, layer_id: u16 },
    LibraryFolder { path: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub scope: Scope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub user_id: UserId,
    pub rank: Rank,
    #[serde(default)]
    pub grants: Vec<Grant>,
    #[serde(default)]
    pub banned: bool,
}
impl Member {
    pub fn viewer(user_id: UserId) -> Self {
        Self {
            user_id,
            rank: Rank::Viewer,
            grants: Vec::new(),
            banned: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoomPolicy {
    pub editor_grants: Vec<Grant>,
    pub admin_grants: Vec<Grant>,
    pub members_can_invite: bool,
    pub max_members: usize,
    /// The lowest rank that may keep a copy of the project: save it, export
    /// it or copy parts of it out of the room. Owners always may. Everyone
    /// in a room receives the document to draw it, so this is what honest
    /// editors enforce, not a lock a modified client could not pick.
    #[serde(skip_serializing_if = "is_default_save_rank")]
    pub save_rank: Rank,
}
fn is_default_save_rank(rank: &Rank) -> bool {
    *rank == Rank::Editor
}
/// Whether this member may save, export or copy the room's project.
pub fn may_keep_copy(policy: &RoomPolicy, member: &Member) -> bool {
    !member.banned && (member.rank == Rank::Owner || member.rank >= policy.save_rank)
}
impl Default for RoomPolicy {
    /// New rooms let editors and admins edit everything. Owners narrow this to
    /// assigned layers and folders when they want scoped work; a rank that
    /// cannot draw anything out of the box is not a useful default.
    fn default() -> Self {
        let everything = || {
            vec![Grant {
                scope: Scope::Project,
            }]
        };
        Self {
            editor_grants: everything(),
            admin_grants: everything(),
            members_can_invite: false,
            max_members: 64,
            // viewers watch; they don't walk off with the project
            save_rank: Rank::Editor,
        }
    }
}
impl RoomPolicy {
    /// Editing defaults that apply to a rank. Owners always edit the whole
    /// project and viewers never edit, so neither has configurable defaults.
    pub fn rank_defaults(&self, rank: Rank) -> &[Grant] {
        match rank {
            Rank::Editor => &self.editor_grants,
            Rank::Admin => &self.admin_grants,
            Rank::Viewer | Rank::Owner => &[],
        }
    }
}

/// Why a member may edit one resource. This is an explanation for interfaces;
/// the server still authorizes every submitted change against canonical data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access {
    pub owner: bool,
    pub rank_default: bool,
    pub assigned: bool,
}
impl Access {
    pub fn allowed(&self) -> bool {
        self.owner || self.rank_default || self.assigned
    }
}
fn member_access(
    policy: &RoomPolicy,
    member: &Member,
    allows: impl Fn(&[&Scope]) -> bool,
) -> Access {
    if member.banned || member.rank == Rank::Viewer {
        return Access::default();
    }
    if member.rank == Rank::Owner {
        return Access {
            owner: true,
            ..Access::default()
        };
    }
    let defaults: Vec<&Scope> = policy
        .rank_defaults(member.rank)
        .iter()
        .map(|grant| &grant.scope)
        .collect();
    let assigned: Vec<&Scope> = member.grants.iter().map(|grant| &grant.scope).collect();
    Access {
        owner: false,
        rank_default: allows(&defaults),
        assigned: allows(&assigned),
    }
}
/// Project metadata, runtime settings and audio arrangement need a project grant.
pub fn project_access(policy: &RoomPolicy, member: &Member) -> Access {
    member_access(policy, member, grants_project)
}
pub fn layer_access(
    project: &ProjectV2,
    policy: &RoomPolicy,
    member: &Member,
    q0rg_id: u16,
    layer_id: u16,
) -> Access {
    member_access(policy, member, |grants| {
        grants_layer(grants, project, q0rg_id, layer_id)
    })
}
pub fn library_access(policy: &RoomPolicy, member: &Member, path: &str) -> Access {
    member_access(policy, member, |grants| grants_library(grants, path))
}
/// Human-readable resource name for messages; ids remain only as a fallback.
pub fn resource_label(project: &ProjectV2, key: &ResourceKey) -> String {
    let q0rg_name = |id: u16| {
        project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == id)
            .map(|q| format!("“{}”", q.name))
            .unwrap_or_else(|| format!("symbol {id}"))
    };
    match key {
        ResourceKey::Meta => "the project settings".into(),
        ResourceKey::Runtime => "project-wide settings (camera, scripts, physics, scenes)".into(),
        ResourceKey::AudioClips => "the audio arrangement".into(),
        ResourceKey::Q0rg { q0rg_id } => format!("the symbol {}", q0rg_name(*q0rg_id)),
        ResourceKey::Layer { q0rg_id, layer_id } => {
            match existing_layer(project, *q0rg_id, *layer_id) {
                Some(layer) => format!("the layer “{}” in {}", layer.name, q0rg_name(*q0rg_id)),
                None => format!("a new layer in {}", q0rg_name(*q0rg_id)),
            }
        }
        ResourceKey::Asset { asset_id } => match project.asset_names.get(asset_id) {
            Some(name) => format!("the shared artwork “{name}”"),
            None => format!("shared artwork #{asset_id}"),
        },
        ResourceKey::LibraryFolder { path } if path.is_empty() => "the library root".into(),
        ResourceKey::LibraryFolder { path } => format!("the library folder “{path}”"),
    }
}
/// Library folder of a symbol, as used by library-folder grants.
pub fn q0rg_library_folder(project: &ProjectV2, q0rg_id: u16) -> &str {
    item_folder(project, LibraryItemReference::Q0rg(q0rg_id))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    Conflict,
    InvalidRequest,
    InvalidProject,
    NotFound,
    RevisionAhead,
    HistoryUnavailable,
    UnsupportedVersion,
    TooLarge,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineError {
    pub code: ErrorCode,
    pub message: String,
}
impl std::fmt::Display for OnlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for OnlineError {}
pub type Result<T> = std::result::Result<T, OnlineError>;
fn error(code: ErrorCode, message: impl Into<String>) -> OnlineError {
    OnlineError {
        code,
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceKey {
    Meta,
    Runtime,
    AudioClips,
    Q0rg { q0rg_id: u16 },
    Layer { q0rg_id: u16, layer_id: u16 },
    Asset { asset_id: u16 },
    LibraryFolder { path: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q0rgRecord {
    pub q0rg_id: u16,
    pub name: String,
    pub frame_count: u16,
    pub script: String,
    pub position: usize,
    pub library_folder: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerRecord {
    pub layer: Layer,
    pub metadata: Option<LayerMetadata>,
    pub position: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRecord {
    pub asset: Asset,
    pub name: Option<String>,
    pub appearance: Option<VectorAppearance>,
    pub position: usize,
    pub library_folder: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ResourceValue {
    Meta(ProjectMeta),
    Runtime(ProjectRuntimeData),
    AudioClips(Vec<v2::AudioClip>),
    Q0rg(Q0rgRecord),
    Layer(LayerRecord),
    Asset(AssetRecord),
    LibraryFolder { position: usize },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub key: ResourceKey,
    pub before: Option<ResourceValue>,
    pub after: Option<ResourceValue>,
}
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPatch {
    pub changes: Vec<Change>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRequest {
    pub operation_id: String,
    pub base_revision: Revision,
    pub patch: ProjectPatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Edit,
    Undo,
    Redo,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomEvent {
    pub revision: Revision,
    pub actor: UserId,
    pub operation_id: String,
    pub kind: EventKind,
    pub target_operation_id: Option<String>,
    pub patch: ProjectPatch,
    #[serde(default)]
    pub generations: Vec<ResourceGeneration>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceGeneration {
    pub key: ResourceKey,
    pub before: Option<Revision>,
    pub after: Option<Revision>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub protocol_version: u32,
    pub revision: Revision,
    pub project: ProjectV2,
    pub members: BTreeMap<UserId, Member>,
    pub policy: RoomPolicy,
}
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    pub q0rg_id: Option<u16>,
    pub layer_id: Option<u16>,
    pub frame: Option<u16>,
    pub x: Option<f32>,
    pub y: Option<f32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
        after_revision: Revision,
    },
    Submit {
        request: OperationRequest,
    },
    Undo {
        operation_id: String,
    },
    Redo {
        operation_id: String,
    },
    Presence {
        presence: Presence,
    },
    Ping,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        // boxed: a whole project dwarfs every other message; serde is unchanged
        snapshot: Box<RoomSnapshot>,
    },
    Event {
        event: RoomEvent,
    },
    Presence {
        user_id: UserId,
        presence: Presence,
    },
    MemberChanged {
        member: Member,
    },
    PolicyChanged {
        policy: RoomPolicy,
    },
    Error {
        operation_id: Option<String>,
        error: OnlineError,
    },
    ResyncRequired {
        revision: Revision,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomState {
    #[serde(default = "state_version")]
    pub state_version: u32,
    pub owner_id: UserId,
    pub project: ProjectV2,
    pub revision: Revision,
    pub members: BTreeMap<UserId, Member>,
    #[serde(default)]
    pub policy: RoomPolicy,
    #[serde(default)]
    pub events: Vec<RoomEvent>,
    #[serde(default)]
    undone: BTreeSet<String>,
    #[serde(default)]
    redo: BTreeMap<UserId, Vec<String>>,
    #[serde(default)]
    receipts: BTreeMap<String, Receipt>,
    #[serde(default)]
    resource_epochs: Vec<(ResourceKey, Revision)>,
    #[serde(default)]
    resource_lifecycle: Vec<(ResourceKey, Revision)>,
}
fn state_version() -> u32 {
    STATE_VERSION
}

pub const MAX_PATCH_CHANGES: usize = 4096;
pub const MAX_PATCH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROJECT_BYTES: usize = 64 * 1024 * 1024;

/// Validate references/geometry with the same format validator as the editor.
/// A JSON round trip additionally rejects non-finite numbers from in-process callers.
pub fn validate_project(project: &ProjectV2) -> Result<()> {
    if project
        .runtime
        .project_graph
        .nodes
        .iter()
        .any(|node| matches!(node.source, v2::ProjectDependencySource::External(_)))
    {
        return Err(error(
            ErrorCode::InvalidProject,
            "embed external project dependencies before sharing",
        ));
    }
    v2::validate(project).map_err(|e| error(ErrorCode::InvalidProject, e.to_string()))?;
    let bytes =
        serde_json::to_vec(project).map_err(|e| error(ErrorCode::InvalidProject, e.to_string()))?;
    if bytes.len() > MAX_PROJECT_BYTES {
        return Err(error(
            ErrorCode::TooLarge,
            "project exceeds the room document limit",
        ));
    }
    serde_json::from_slice::<ProjectV2>(&bytes).map_err(|e| {
        error(
            ErrorCode::InvalidProject,
            format!("project contains invalid numeric data: {e}"),
        )
    })?;
    Ok(())
}

type Entities = BTreeMap<ResourceKey, ResourceValue>;
fn entities(project: &ProjectV2) -> Entities {
    let mut result = BTreeMap::new();
    result.insert(ResourceKey::Meta, ResourceValue::Meta(project.meta.clone()));
    let mut runtime = project.runtime.clone();
    runtime.library_folders.clear();
    runtime.library_item_folders.clear();
    result.insert(ResourceKey::Runtime, ResourceValue::Runtime(runtime));
    result.insert(
        ResourceKey::AudioClips,
        ResourceValue::AudioClips(project.audio_clips.clone()),
    );
    for (position, path) in project.runtime.library_folders.iter().enumerate() {
        result.insert(
            ResourceKey::LibraryFolder { path: path.clone() },
            ResourceValue::LibraryFolder { position },
        );
    }
    for (position, q) in project.q0rgs.iter().enumerate() {
        result.insert(
            ResourceKey::Q0rg { q0rg_id: q.q0rg_id },
            ResourceValue::Q0rg(Q0rgRecord {
                q0rg_id: q.q0rg_id,
                name: q.name.clone(),
                frame_count: q.frame_count,
                script: q.script.clone(),
                position,
                library_folder: project
                    .runtime
                    .library_item_folders
                    .get(&LibraryItemReference::Q0rg(q.q0rg_id))
                    .cloned(),
            }),
        );
        for (position, layer) in q.layers.iter().enumerate() {
            result.insert(
                ResourceKey::Layer {
                    q0rg_id: q.q0rg_id,
                    layer_id: layer.layer_id,
                },
                ResourceValue::Layer(LayerRecord {
                    layer: layer.clone(),
                    metadata: project
                        .layer_metadata
                        .get(&LayerKey::new(q.q0rg_id, layer.layer_id))
                        .copied(),
                    position,
                }),
            );
        }
    }
    for (position, asset) in project.assets.iter().enumerate() {
        let id = asset.id();
        result.insert(
            ResourceKey::Asset { asset_id: id },
            ResourceValue::Asset(AssetRecord {
                asset: asset.clone(),
                name: project.asset_names.get(&id).cloned(),
                appearance: project.asset_appearances.get(&id).cloned(),
                position,
                library_folder: project
                    .runtime
                    .library_item_folders
                    .get(&LibraryItemReference::Asset(id))
                    .cloned(),
            }),
        );
    }
    result
}

fn project_from_entities(entities: &Entities) -> Result<ProjectV2> {
    let Some(ResourceValue::Meta(meta)) = entities.get(&ResourceKey::Meta) else {
        return Err(error(
            ErrorCode::InvalidRequest,
            "project metadata cannot be removed",
        ));
    };
    let Some(ResourceValue::Runtime(runtime)) = entities.get(&ResourceKey::Runtime) else {
        return Err(error(
            ErrorCode::InvalidRequest,
            "project runtime cannot be removed",
        ));
    };
    if !runtime.library_folders.is_empty() || !runtime.library_item_folders.is_empty() {
        return Err(error(
            ErrorCode::InvalidRequest,
            "library ownership must use its typed resources",
        ));
    }
    let Some(ResourceValue::AudioClips(audio_clips)) = entities.get(&ResourceKey::AudioClips)
    else {
        return Err(error(
            ErrorCode::InvalidRequest,
            "audio clip resource cannot be removed",
        ));
    };
    let mut project = ProjectV2 {
        meta: meta.clone(),
        runtime: runtime.clone(),
        assets: Vec::new(),
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: audio_clips.clone(),
        q0rgs: Vec::new(),
    };
    let mut q0rgs = Vec::new();
    let mut layers = BTreeMap::<u16, Vec<(usize, LayerRecord)>>::new();
    let mut assets = Vec::new();
    let mut folders = Vec::new();
    for (key, value) in entities {
        match (key, value) {
            (ResourceKey::Meta, ResourceValue::Meta(_))
            | (ResourceKey::Runtime, ResourceValue::Runtime(_))
            | (ResourceKey::AudioClips, ResourceValue::AudioClips(_)) => {}
            (ResourceKey::Q0rg { q0rg_id }, ResourceValue::Q0rg(q)) if *q0rg_id == q.q0rg_id => {
                if let Some(path) = &q.library_folder {
                    project
                        .runtime
                        .library_item_folders
                        .insert(LibraryItemReference::Q0rg(*q0rg_id), path.clone());
                }
                q0rgs.push((q.position, q));
            }
            (ResourceKey::Layer { q0rg_id, layer_id }, ResourceValue::Layer(layer))
                if *layer_id == layer.layer.layer_id =>
            {
                if let Some(metadata) = layer.metadata {
                    project
                        .layer_metadata
                        .insert(LayerKey::new(*q0rg_id, *layer_id), metadata);
                }
                layers
                    .entry(*q0rg_id)
                    .or_default()
                    .push((layer.position, layer.clone()));
            }
            (ResourceKey::Asset { asset_id }, ResourceValue::Asset(asset))
                if *asset_id == asset.asset.id() =>
            {
                if let Some(path) = &asset.library_folder {
                    project
                        .runtime
                        .library_item_folders
                        .insert(LibraryItemReference::Asset(*asset_id), path.clone());
                }
                if let Some(name) = &asset.name {
                    project.asset_names.insert(*asset_id, name.clone());
                }
                if let Some(appearance) = &asset.appearance {
                    project
                        .asset_appearances
                        .insert(*asset_id, appearance.clone());
                }
                assets.push((asset.position, asset.asset.clone()));
            }
            (ResourceKey::LibraryFolder { path }, ResourceValue::LibraryFolder { position }) => {
                folders.push((*position, path.clone()));
            }
            _ => {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "resource key and value do not match",
                ))
            }
        }
    }
    q0rgs.sort_by_key(|(position, q)| (*position, q.q0rg_id));
    for (_, q) in q0rgs {
        let mut q_layers = layers.remove(&q.q0rg_id).unwrap_or_default();
        q_layers.sort_by_key(|(position, l)| (*position, l.layer.layer_id));
        project.q0rgs.push(v2::Q0rg {
            q0rg_id: q.q0rg_id,
            name: q.name.clone(),
            frame_count: q.frame_count,
            script: q.script.clone(),
            layers: q_layers.into_iter().map(|(_, l)| l.layer).collect(),
        });
    }
    if !layers.is_empty() {
        return Err(error(
            ErrorCode::InvalidRequest,
            "layer references a missing symbol",
        ));
    }
    assets.sort_by_key(|(position, asset)| (*position, asset.id()));
    project.assets = assets.into_iter().map(|(_, a)| a).collect();
    folders.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    project.runtime.library_folders = folders.into_iter().map(|(_, path)| path).collect();
    Ok(project)
}

impl ProjectPatch {
    pub fn between(before: &ProjectV2, after: &ProjectV2) -> Result<Self> {
        validate_project(after)?;
        let a = entities(before);
        let b = entities(after);
        let keys = a.keys().chain(b.keys()).cloned().collect::<BTreeSet<_>>();
        let changes = keys
            .into_iter()
            .filter_map(|key| {
                let before = a.get(&key).cloned();
                let after = b.get(&key).cloned();
                (before != after).then_some(Change { key, before, after })
            })
            .collect();
        let patch = Self { changes };
        patch.validate()?;
        Ok(patch)
    }
    /// `between` without validating the whole resulting document. Clients use
    /// it for every local edit; the server validates what it accepts.
    pub fn diff(before: &ProjectV2, after: &ProjectV2) -> Result<Self> {
        let a = entities(before);
        let b = entities(after);
        let keys = a.keys().chain(b.keys()).collect::<BTreeSet<_>>();
        let changes = keys
            .into_iter()
            .filter_map(|key| {
                let before = a.get(key);
                let after = b.get(key);
                (before != after).then(|| Change {
                    key: key.clone(),
                    before: before.cloned(),
                    after: after.cloned(),
                })
            })
            .collect();
        Ok(Self { changes })
    }
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
    pub fn inverse(&self) -> Self {
        Self {
            changes: self
                .changes
                .iter()
                .map(|change| Change {
                    key: change.key.clone(),
                    before: change.after.clone(),
                    after: change.before.clone(),
                })
                .collect(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.changes.len() > MAX_PATCH_CHANGES {
            return Err(error(
                ErrorCode::TooLarge,
                "operation changes too many resources",
            ));
        }
        let mut keys = BTreeSet::new();
        for change in &self.changes {
            if !keys.insert(&change.key) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "operation repeats a resource",
                ));
            }
            if change.before == change.after {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "operation contains an unchanged resource",
                ));
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?;
        if bytes.len() > MAX_PATCH_BYTES {
            return Err(error(
                ErrorCode::TooLarge,
                "operation exceeds the message size limit",
            ));
        }
        // Serde encodes NaN as null; decoding the typed shape rejects that value.
        serde_json::from_slice::<Self>(&bytes)
            .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?;
        Ok(())
    }
    /// Apply a server event or local conditional patch atomically. Permission
    /// checks belong to RoomState::submit; this helper does not authorize edits.
    pub fn apply(&self, project: &mut ProjectV2) -> Result<()> {
        self.apply_checked(project, true)
    }
    /// Apply an event the server has already validated and accepted. Skips
    /// re-validating and re-serializing the whole document, which dominates
    /// the cost of applying a small edit to a large project.
    pub fn apply_trusted(&self, project: &mut ProjectV2) -> Result<()> {
        self.apply_checked(project, false)
    }
    fn apply_checked(&self, project: &mut ProjectV2, full: bool) -> Result<()> {
        if full {
            self.validate()?;
        }
        let mut records = entities(project);
        for change in &self.changes {
            if records.get(&change.key) != change.before.as_ref() {
                return Err(error(
                    ErrorCode::Conflict,
                    format!(
                        "{} changed since this edit was prepared",
                        resource_label(project, &change.key)
                    ),
                ));
            }
        }
        for change in &self.changes {
            if let Some(value) = &change.after {
                records.insert(change.key.clone(), value.clone());
            } else {
                records.remove(&change.key);
            }
        }
        let next = project_from_entities(&records)?;
        if full {
            if entities(&next) != records {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "resource positions must describe the canonical contiguous document order",
                ));
            }
            validate_project(&next)?;
        }
        *project = next;
        Ok(())
    }
}

fn valid_folder(path: &str, allow_root: bool) -> bool {
    (allow_root && path.is_empty())
        || (!path.is_empty()
            && path.len() <= 1024
            && !path.contains('\\')
            && path.split('/').all(|part| {
                !part.is_empty()
                    && part.trim() == part
                    && part != "."
                    && part != ".."
                    && !part.chars().any(char::is_control)
            }))
}
fn path_contains(root: &str, path: &str) -> bool {
    root.is_empty()
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn item_folder(project: &ProjectV2, item: LibraryItemReference) -> &str {
    project
        .runtime
        .library_item_folders
        .get(&item)
        .map(String::as_str)
        .unwrap_or("")
}
fn grants_project(grants: &[&Scope]) -> bool {
    grants.iter().any(|scope| matches!(scope, Scope::Project))
}
fn grants_library(grants: &[&Scope], path: &str) -> bool {
    grants_project(grants)
        || grants.iter().any(
            |scope| matches!(scope,Scope::LibraryFolder{path:root} if path_contains(root,path)),
        )
}
fn grants_layer(grants: &[&Scope], project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    if grants_project(grants)
        || grants_library(
            grants,
            item_folder(project, LibraryItemReference::Q0rg(q0rg_id)),
        )
    {
        return true;
    }
    grants.iter().any(|scope| match scope {
        Scope::Layer {
            q0rg_id: q,
            layer_id: l,
        } => *q == q0rg_id && *l == layer_id,
        Scope::LayerFolder {
            q0rg_id: q,
            layer_id: l,
        } => {
            *q == q0rg_id
                && project.layer_is_folder(q0rg_id, *l)
                && (*l == layer_id || project.layer_is_descendant_of(q0rg_id, layer_id, *l))
        }
        _ => false,
    })
}
fn existing_layer(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<&Layer> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == layer_id)
}
fn existing_q0rg(project: &ProjectV2, q0rg_id: u16) -> bool {
    project.q0rgs.iter().any(|q| q.q0rg_id == q0rg_id)
}
fn asset_referencing_layers(project: &ProjectV2, asset_id: u16) -> Vec<(u16, u16)> {
    project
        .q0rgs
        .iter()
        .flat_map(|q| {
            q.layers
                .iter()
                .filter(move |layer| {
                    layer
                        .placements
                        .iter()
                        .any(|p| p.target == v2::Target::Asset(asset_id))
                })
                .map(move |layer| (q.q0rg_id, layer.layer_id))
        })
        .collect()
}
fn grants_asset(grants: &[&Scope], project: &ProjectV2, asset_id: u16) -> bool {
    if grants_library(
        grants,
        item_folder(project, LibraryItemReference::Asset(asset_id)),
    ) {
        return true;
    }
    let uses = asset_referencing_layers(project, asset_id);
    !uses.is_empty()
        && uses
            .into_iter()
            .all(|(q, l)| grants_layer(grants, project, q, l))
}
fn position_only(change: &Change) -> bool {
    match (&change.before, &change.after) {
        (Some(ResourceValue::Layer(a)), Some(ResourceValue::Layer(b))) => {
            a.layer == b.layer && a.metadata == b.metadata
        }
        (Some(ResourceValue::Asset(a)), Some(ResourceValue::Asset(b))) => {
            a.asset == b.asset
                && a.name == b.name
                && a.appearance == b.appearance
                && a.library_folder == b.library_folder
        }
        (Some(ResourceValue::Q0rg(a)), Some(ResourceValue::Q0rg(b))) => {
            a.q0rg_id == b.q0rg_id
                && a.name == b.name
                && a.frame_count == b.frame_count
                && a.script == b.script
                && a.library_folder == b.library_folder
        }
        (Some(ResourceValue::LibraryFolder { .. }), Some(ResourceValue::LibraryFolder { .. })) => {
            true
        }
        _ => false,
    }
}
fn protected_order_preserved(
    grants: &[&Scope],
    before: &ProjectV2,
    after: &ProjectV2,
    key: &ResourceKey,
) -> bool {
    match key {
        ResourceKey::Layer { q0rg_id, .. } => {
            let Some(a) = before.q0rgs.iter().find(|q| q.q0rg_id == *q0rg_id) else {
                return false;
            };
            let Some(b) = after.q0rgs.iter().find(|q| q.q0rg_id == *q0rg_id) else {
                return false;
            };
            let protected = a
                .layers
                .iter()
                .filter(|layer| !grants_layer(grants, before, *q0rg_id, layer.layer_id))
                .map(|layer| layer.layer_id)
                .collect::<BTreeSet<_>>();
            a.layers
                .iter()
                .filter(|layer| protected.contains(&layer.layer_id))
                .map(|layer| layer.layer_id)
                .eq(b
                    .layers
                    .iter()
                    .filter(|layer| protected.contains(&layer.layer_id))
                    .map(|layer| layer.layer_id))
        }
        ResourceKey::Asset { .. } => {
            let protected = before
                .assets
                .iter()
                .filter(|asset| !grants_asset(grants, before, asset.id()))
                .map(Asset::id)
                .collect::<BTreeSet<_>>();
            before
                .assets
                .iter()
                .filter(|asset| protected.contains(&asset.id()))
                .map(Asset::id)
                .eq(after
                    .assets
                    .iter()
                    .filter(|asset| protected.contains(&asset.id()))
                    .map(Asset::id))
        }
        ResourceKey::Q0rg { .. } => {
            let protected = before
                .q0rgs
                .iter()
                .filter(|q| {
                    !grants_library(
                        grants,
                        item_folder(before, LibraryItemReference::Q0rg(q.q0rg_id)),
                    )
                })
                .map(|q| q.q0rg_id)
                .collect::<BTreeSet<_>>();
            before
                .q0rgs
                .iter()
                .filter(|q| protected.contains(&q.q0rg_id))
                .map(|q| q.q0rg_id)
                .eq(after
                    .q0rgs
                    .iter()
                    .filter(|q| protected.contains(&q.q0rg_id))
                    .map(|q| q.q0rg_id))
        }
        ResourceKey::LibraryFolder { .. } => {
            let protected = before
                .runtime
                .library_folders
                .iter()
                .filter(|path| !grants_library(grants, path))
                .collect::<BTreeSet<_>>();
            before
                .runtime
                .library_folders
                .iter()
                .filter(|path| protected.contains(path))
                .eq(after
                    .runtime
                    .library_folders
                    .iter()
                    .filter(|path| protected.contains(path)))
        }
        _ => false,
    }
}

fn validate_grants(project: &ProjectV2, grants: &[Grant]) -> Result<()> {
    if grants.len() > 128 {
        return Err(error(ErrorCode::TooLarge, "too many permission grants"));
    }
    for grant in grants {
        let exists = match &grant.scope {
            Scope::Project => true,
            Scope::Layer { q0rg_id, layer_id } => {
                existing_layer(project, *q0rg_id, *layer_id).is_some()
            }
            Scope::LayerFolder { q0rg_id, layer_id } => {
                existing_layer(project, *q0rg_id, *layer_id).is_some()
                    && project.layer_is_folder(*q0rg_id, *layer_id)
            }
            Scope::LibraryFolder { path } => {
                valid_folder(path, true)
                    && (path.is_empty() || project.runtime.library_folders.contains(path))
            }
        };
        if !exists {
            return Err(error(
                ErrorCode::InvalidRequest,
                "permission scope does not identify an existing resource",
            ));
        }
    }
    Ok(())
}
fn validate_policy(project: &ProjectV2, policy: &RoomPolicy) -> Result<()> {
    if !(1..=256).contains(&policy.max_members) {
        return Err(error(
            ErrorCode::InvalidRequest,
            "max_members must be between 1 and 256",
        ));
    }
    validate_grants(project, &policy.editor_grants)?;
    validate_grants(project, &policy.admin_grants)
}

impl Presence {
    pub fn validate(&self) -> Result<()> {
        if self
            .x
            .into_iter()
            .chain(self.y)
            .any(|value| !value.is_finite() || value.abs() > 1e9)
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "presence coordinates must be finite stage coordinates",
            ));
        }
        Ok(())
    }
}

impl RoomState {
    pub fn new(owner_id: UserId, project: ProjectV2) -> Result<Self> {
        if owner_id <= 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "owner must identify an account",
            ));
        }
        validate_project(&project)?;
        let owner = Member {
            user_id: owner_id,
            rank: Rank::Owner,
            grants: Vec::new(),
            banned: false,
        };
        Ok(Self {
            state_version: STATE_VERSION,
            owner_id,
            project,
            revision: 0,
            members: BTreeMap::from([(owner_id, owner)]),
            policy: RoomPolicy::default(),
            events: Vec::new(),
            undone: BTreeSet::new(),
            redo: BTreeMap::new(),
            receipts: BTreeMap::new(),
            resource_epochs: Vec::new(),
            resource_lifecycle: Vec::new(),
        })
    }
    pub fn can_view(&self, user_id: UserId) -> bool {
        self.members
            .get(&user_id)
            .is_some_and(|member| !member.banned)
    }
    pub fn member(&self, user_id: UserId) -> Result<&Member> {
        let member = self
            .members
            .get(&user_id)
            .ok_or_else(|| error(ErrorCode::Forbidden, "this account is not a room member"))?;
        if member.banned {
            return Err(error(
                ErrorCode::Forbidden,
                "this account is banned from the room",
            ));
        }
        Ok(member)
    }
    pub fn can_invite(&self, user_id: UserId) -> bool {
        self.member(user_id)
            .is_ok_and(|member| member.rank >= Rank::Admin || self.policy.members_can_invite)
    }
    pub fn snapshot(&self, user_id: UserId) -> Result<RoomSnapshot> {
        self.member(user_id)?;
        Ok(RoomSnapshot {
            protocol_version: PROTOCOL_VERSION,
            revision: self.revision,
            project: self.project.clone(),
            members: self.members.clone(),
            policy: self.policy.clone(),
        })
    }
    fn owner(&self, actor: UserId) -> Result<()> {
        self.member(actor)?;
        if actor != self.owner_id {
            return Err(error(
                ErrorCode::Forbidden,
                "only the room owner can change ranks and permissions",
            ));
        }
        Ok(())
    }
    fn write_grants(&self, actor: UserId) -> Result<Vec<&Scope>> {
        let member = self.member(actor)?;
        if member.rank == Rank::Viewer {
            return Err(error(
                ErrorCode::Forbidden,
                "viewers cannot modify the project",
            ));
        }
        if member.rank == Rank::Owner {
            return Ok(vec![&Scope::Project]);
        }
        Ok(self
            .policy
            .rank_defaults(member.rank)
            .iter()
            .chain(&member.grants)
            .map(|grant| &grant.scope)
            .collect())
    }
    pub fn set_member(&mut self, actor: UserId, member: Member) -> Result<()> {
        self.owner(actor)?;
        if member.user_id <= 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "member must identify an account",
            ));
        }
        if member.user_id == self.owner_id {
            if member.rank != Rank::Owner || member.banned {
                return Err(error(
                    ErrorCode::Forbidden,
                    "the room owner cannot be demoted or banned",
                ));
            }
        } else if member.rank == Rank::Owner {
            return Err(error(
                ErrorCode::Forbidden,
                "ownership cannot be granted through member permissions",
            ));
        }
        validate_grants(&self.project, &member.grants)?;
        if !self.members.contains_key(&member.user_id)
            && self.members.len() >= self.policy.max_members
        {
            return Err(error(
                ErrorCode::TooLarge,
                "the room member limit has been reached",
            ));
        }
        self.members.insert(member.user_id, member);
        Ok(())
    }
    /// Invites can only admit a viewer; editing grants are always the owner's
    /// separate explicit decision. A ban cannot be bypassed with another invite.
    pub fn admit_viewer(&mut self, actor: UserId, user_id: UserId) -> Result<()> {
        if !self.can_invite(actor) {
            return Err(error(
                ErrorCode::Forbidden,
                "this member cannot invite people",
            ));
        }
        if let Some(member) = self.members.get(&user_id) {
            if member.banned {
                return Err(error(
                    ErrorCode::Forbidden,
                    "this account is banned from the room",
                ));
            }
            return Ok(());
        }
        if user_id <= 0 || self.members.len() >= self.policy.max_members {
            return Err(error(
                ErrorCode::TooLarge,
                "the room member limit has been reached",
            ));
        }
        self.members.insert(user_id, Member::viewer(user_id));
        Ok(())
    }
    pub fn ban(&mut self, actor: UserId, target: UserId, banned: bool) -> Result<()> {
        let actor_rank = self.member(actor)?.rank;
        let target_rank = self
            .members
            .get(&target)
            .ok_or_else(|| error(ErrorCode::NotFound, "room member was not found"))?
            .rank;
        if target == self.owner_id
            || actor == target
            || actor_rank < Rank::Admin
            || actor_rank <= target_rank
        {
            return Err(error(
                ErrorCode::Forbidden,
                "moderators can only ban lower-ranked participants",
            ));
        }
        self.members.get_mut(&target).unwrap().banned = banned;
        Ok(())
    }
    pub fn set_policy(&mut self, actor: UserId, policy: RoomPolicy) -> Result<()> {
        self.owner(actor)?;
        validate_policy(&self.project, &policy)?;
        if self.members.len() > policy.max_members {
            return Err(error(
                ErrorCode::InvalidRequest,
                "max_members is smaller than current membership",
            ));
        }
        self.policy = policy;
        Ok(())
    }
    pub fn can_edit(&self, actor: UserId, key: &ResourceKey) -> bool {
        let Ok(grants) = self.write_grants(actor) else {
            return false;
        };
        match key {
            ResourceKey::Meta | ResourceKey::Runtime | ResourceKey::AudioClips => {
                grants_project(&grants)
            }
            ResourceKey::Layer { q0rg_id, layer_id } => {
                grants_layer(&grants, &self.project, *q0rg_id, *layer_id)
            }
            ResourceKey::Q0rg { q0rg_id } => grants_library(
                &grants,
                item_folder(&self.project, LibraryItemReference::Q0rg(*q0rg_id)),
            ),
            ResourceKey::Asset { asset_id } => grants_asset(&grants, &self.project, *asset_id),
            ResourceKey::LibraryFolder { path } => grants_library(&grants, path),
        }
    }
    fn authorize(&self, actor: UserId, patch: &ProjectPatch, next: &ProjectV2) -> Result<()> {
        let grants = self.write_grants(actor)?;
        if grants_project(&grants) {
            return Ok(());
        }
        for change in &patch.changes {
            if position_only(change)
                && protected_order_preserved(&grants, &self.project, next, &change.key)
            {
                continue;
            }
            let allowed = match &change.key {
                ResourceKey::Meta | ResourceKey::Runtime | ResourceKey::AudioClips => false,
                ResourceKey::Asset { asset_id } => {
                    let old = self
                        .project
                        .assets
                        .iter()
                        .any(|asset| asset.id() == *asset_id);
                    let new = next.assets.iter().any(|asset| asset.id() == *asset_id);
                    (!old || grants_asset(&grants,&self.project,*asset_id)) && (!new || grants_asset(&grants,next,*asset_id)) &&
                    // Moving an asset cannot turn a layer grant into library ownership.
                    (item_folder(&self.project,LibraryItemReference::Asset(*asset_id))==item_folder(next,LibraryItemReference::Asset(*asset_id)) ||
                        (grants_library(&grants,item_folder(&self.project,LibraryItemReference::Asset(*asset_id))) && grants_library(&grants,item_folder(next,LibraryItemReference::Asset(*asset_id)))))
                }
                ResourceKey::Q0rg { q0rg_id } => {
                    let old = existing_q0rg(&self.project, *q0rg_id);
                    let new = existing_q0rg(next, *q0rg_id);
                    (!old
                        || grants_library(
                            &grants,
                            item_folder(&self.project, LibraryItemReference::Q0rg(*q0rg_id)),
                        ))
                        && (!new
                            || grants_library(
                                &grants,
                                item_folder(next, LibraryItemReference::Q0rg(*q0rg_id)),
                            ))
                }
                ResourceKey::LibraryFolder { path } => grants_library(&grants, path),
                ResourceKey::Layer { q0rg_id, layer_id } => {
                    let old = existing_layer(&self.project, *q0rg_id, *layer_id).is_some();
                    let new = existing_layer(next, *q0rg_id, *layer_id).is_some();
                    let old_parent = self.project.layer_parent_folder(*q0rg_id, *layer_id);
                    let new_parent = next.layer_parent_folder(*q0rg_id, *layer_id);
                    let destination = !new
                        || old_parent == new_parent
                        || match new_parent {
                            Some(parent) => grants_layer(&grants, next, *q0rg_id, parent),
                            None => grants_library(
                                &grants,
                                item_folder(next, LibraryItemReference::Q0rg(*q0rg_id)),
                            ),
                        };
                    let descendants = self
                        .project
                        .q0rgs
                        .iter()
                        .find(|q| q.q0rg_id == *q0rg_id)
                        .is_none_or(|q| {
                            q.layers.iter().all(|layer| {
                                let affected = self.project.layer_is_descendant_of(
                                    *q0rg_id,
                                    layer.layer_id,
                                    *layer_id,
                                ) || self
                                    .project
                                    .layer_clipping_masks(*q0rg_id, layer.layer_id)
                                    .contains(layer_id);
                                !affected
                                    || grants_layer(
                                        &grants,
                                        &self.project,
                                        *q0rg_id,
                                        layer.layer_id,
                                    )
                            })
                        });
                    (!old || grants_layer(&grants, &self.project, *q0rg_id, *layer_id))
                        && (!new || grants_layer(&grants, next, *q0rg_id, *layer_id))
                        && destination
                        && descendants
                }
            };
            if !allowed {
                return Err(error(
                    ErrorCode::Forbidden,
                    format!(
                        "your room permissions do not cover {}",
                        resource_label(&self.project, &change.key)
                    ),
                ));
            }
        }
        Ok(())
    }
}

const MAX_EVENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECEIPTS: usize = 65_536;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    actor: UserId,
    kind: EventKind,
    fingerprint: String,
    revision: Revision,
}
fn fingerprint(kind: EventKind, patch: Option<&ProjectPatch>) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut value = serde_json::to_value((kind, patch))
        .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?;
    canonicalize_wire_maps(&mut value);
    let bytes =
        serde_json::to_vec(&value).map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn operation_id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
impl RoomState {
    fn retry(
        &self,
        actor: UserId,
        id: &str,
        kind: EventKind,
        fingerprint: &str,
    ) -> Result<Option<RoomEvent>> {
        self.member(actor)?;
        if !operation_id_valid(id) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "operation_id must contain 1 to 128 ascii identifier characters",
            ));
        }
        if let Some(receipt) = self.receipts.get(id) {
            if receipt.actor != actor || receipt.kind != kind || receipt.fingerprint != fingerprint
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "operation_id was already used for another request",
                ));
            }
            return self.events.iter().find(|event|event.operation_id==id).cloned().map(Some).ok_or_else(||error(ErrorCode::HistoryUnavailable,format!("operation was already applied at revision {}; reload the room snapshot",receipt.revision)));
        }
        if let Some(event) = self.events.iter().find(|event| event.operation_id == id) {
            let original = fingerprint_for_event(event)?;
            if event.actor != actor || event.kind != kind || original != fingerprint {
                return Err(error(
                    ErrorCode::Conflict,
                    "operation_id was already used for another request",
                ));
            }
            return Ok(Some(event.clone()));
        }
        Ok(None)
    }
    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        actor: UserId,
        id: String,
        kind: EventKind,
        target_operation_id: Option<String>,
        patch: ProjectPatch,
        request_fingerprint: String,
        generation_source: Option<&RoomEvent>,
    ) -> Result<RoomEvent> {
        let records = entities(&self.project);
        let current_epoch = |key: &ResourceKey| {
            records.contains_key(key).then(|| {
                self.resource_epochs
                    .iter()
                    .find(|(candidate, _)| candidate == key)
                    .map(|(_, epoch)| *epoch)
                    .unwrap_or(0)
            })
        };
        if let Some(source) = generation_source {
            for change in &patch.changes {
                // Changes a merge added (order bookkeeping) are not part of the
                // personal edit; their identity is unchanged by this commit.
                let Some(generation) = source
                    .generations
                    .iter()
                    .find(|generation| generation.key == change.key)
                else {
                    continue;
                };
                let expected = if kind == EventKind::Undo {
                    generation.after
                } else {
                    generation.before
                };
                if current_epoch(&change.key) != expected {
                    return Err(error(
                        ErrorCode::Conflict,
                        "resource was replaced after this personal edit",
                    ));
                }
            }
        }
        let mut next = self.project.clone();
        patch.apply(&mut next)?;
        self.authorize(actor, &patch, &next)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| error(ErrorCode::TooLarge, "room revision limit reached"))?;
        let generations = patch
            .changes
            .iter()
            .map(|change| {
                let before = current_epoch(&change.key);
                let restored = generation_source.and_then(|source| {
                    source
                        .generations
                        .iter()
                        .find(|generation| generation.key == change.key)
                });
                let after = if change.after.is_none() {
                    None
                } else if let Some(generation) = restored {
                    if kind == EventKind::Undo {
                        generation.before
                    } else {
                        generation.after
                    }
                } else {
                    Some(before.unwrap_or(revision))
                };
                ResourceGeneration {
                    key: change.key.clone(),
                    before,
                    after,
                }
            })
            .collect();
        let event = RoomEvent {
            revision,
            actor,
            operation_id: id.clone(),
            kind,
            target_operation_id,
            patch,
            generations,
        };
        let event_size = serde_json::to_vec(&event)
            .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?
            .len();
        if event_size > MAX_EVENT_BYTES {
            return Err(error(
                ErrorCode::TooLarge,
                "event exceeds retained history size limit",
            ));
        }
        self.project = next;
        self.revision = revision;
        for generation in &event.generations {
            self.resource_epochs
                .retain(|(key, _)| key != &generation.key);
            if let Some(epoch) = generation.after {
                self.resource_epochs.push((generation.key.clone(), epoch));
            }
            if generation.before.is_none() != generation.after.is_none() {
                self.resource_lifecycle
                    .retain(|(key, _)| key != &generation.key);
                self.resource_lifecycle
                    .push((generation.key.clone(), revision));
            }
        }
        let deleted = event
            .patch
            .changes
            .iter()
            .filter(|change| change.after.is_none())
            .map(|change| change.key.clone())
            .collect::<Vec<_>>();
        let still_valid = |grant: &Grant| {
            !deleted.iter().any(|key| match (key, &grant.scope) {
                (
                    ResourceKey::Layer {
                        q0rg_id: q,
                        layer_id: l,
                    },
                    Scope::Layer { q0rg_id, layer_id } | Scope::LayerFolder { q0rg_id, layer_id },
                ) => q == q0rg_id && l == layer_id,
                (
                    ResourceKey::Q0rg { q0rg_id: q },
                    Scope::Layer { q0rg_id, .. } | Scope::LayerFolder { q0rg_id, .. },
                ) => q == q0rg_id,
                (ResourceKey::LibraryFolder { path: deleted }, Scope::LibraryFolder { path }) => {
                    path_contains(deleted, path)
                }
                _ => false,
            })
        };
        for member in self.members.values_mut() {
            member.grants.retain(&still_valid);
        }
        self.policy.editor_grants.retain(&still_valid);
        self.policy.admin_grants.retain(&still_valid);
        self.receipts.insert(
            id,
            Receipt {
                actor,
                kind,
                fingerprint: request_fingerprint,
                revision,
            },
        );
        self.prune_receipts();
        self.events.push(event.clone());
        let mut bytes = serde_json::to_vec(&self.events)
            .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?
            .len();
        while self.events.len() > MAX_EVENTS || bytes > MAX_EVENT_BYTES {
            let removed = self.events.remove(0);
            bytes = bytes.saturating_sub(
                serde_json::to_vec(&removed)
                    .map(|v| v.len() + 1)
                    .unwrap_or(0),
            );
        }
        let retained = self
            .events
            .iter()
            .filter(|event| event.kind == EventKind::Edit)
            .map(|event| event.operation_id.clone())
            .collect::<BTreeSet<_>>();
        self.undone.retain(|id| retained.contains(id));
        for stack in self.redo.values_mut() {
            stack.retain(|id| retained.contains(id));
        }
        Ok(event)
    }
    pub fn submit(&mut self, actor: UserId, request: OperationRequest) -> Result<RoomEvent> {
        let hash = fingerprint(EventKind::Edit, Some(&request.patch))?;
        if let Some(event) = self.retry(actor, &request.operation_id, EventKind::Edit, &hash)? {
            return Ok(event);
        }
        if request.base_revision > self.revision {
            return Err(error(
                ErrorCode::RevisionAhead,
                "operation revision is newer than the room",
            ));
        }
        if request.patch.is_empty() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "operation must change the project",
            ));
        }
        // Editing something that was deleted and recreated since is a conflict
        // even if the replacement looks identical. New resources whose id was
        // taken meanwhile are renumbered by the merge instead.
        if request.patch.changes.iter().any(|change| {
            change.before.is_some()
                && self
                    .resource_lifecycle
                    .iter()
                    .any(|(key, revision)| key == &change.key && *revision > request.base_revision)
        }) {
            return Err(error(
                ErrorCode::Conflict,
                "something you edited was deleted and replaced since you started; your change was kept",
            ));
        }
        // Edits prepared against an older revision are merged with everything
        // accepted since, so people drawing at the same time don't collide.
        let patch = if request.base_revision < self.revision {
            self.rebase(request.base_revision, &request.patch, true)?
        } else {
            request.patch
        };
        let event = self.commit(
            actor,
            request.operation_id,
            EventKind::Edit,
            None,
            patch,
            hash,
            None,
        )?;
        self.redo.remove(&actor);
        Ok(event)
    }
    pub fn undo(&mut self, actor: UserId, operation_id: String) -> Result<RoomEvent> {
        let hash = fingerprint(EventKind::Undo, None)?;
        if let Some(event) = self.retry(actor, &operation_id, EventKind::Undo, &hash)? {
            return Ok(event);
        }
        self.write_grants(actor)?;
        let target = self
            .events
            .iter()
            .rev()
            .find(|event| {
                event.actor == actor
                    && event.kind == EventKind::Edit
                    && !self.undone.contains(&event.operation_id)
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    ErrorCode::HistoryUnavailable,
                    "no retained personal edit can be undone",
                )
            })?;
        let patch = self.rebase(target.revision, &target.patch.inverse(), false)?;
        let event = self.commit(
            actor,
            operation_id,
            EventKind::Undo,
            Some(target.operation_id.clone()),
            patch,
            hash,
            Some(&target),
        )?;
        if self
            .events
            .iter()
            .any(|event| event.operation_id == target.operation_id)
        {
            self.undone.insert(target.operation_id.clone());
            self.redo
                .entry(actor)
                .or_default()
                .push(target.operation_id);
        }
        Ok(event)
    }
    pub fn redo(&mut self, actor: UserId, operation_id: String) -> Result<RoomEvent> {
        let hash = fingerprint(EventKind::Redo, None)?;
        if let Some(event) = self.retry(actor, &operation_id, EventKind::Redo, &hash)? {
            return Ok(event);
        }
        self.write_grants(actor)?;
        let target_id = self
            .redo
            .get(&actor)
            .and_then(|stack| stack.last())
            .cloned()
            .ok_or_else(|| {
                error(
                    ErrorCode::HistoryUnavailable,
                    "no retained personal edit can be redone",
                )
            })?;
        let target = self
            .events
            .iter()
            .find(|event| {
                event.operation_id == target_id
                    && event.actor == actor
                    && event.kind == EventKind::Edit
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    ErrorCode::HistoryUnavailable,
                    "the redo edit is outside retained history",
                )
            })?;
        // Redo reverses the undo, which matches the document exactly at the
        // undo's revision; the original patch may carry positions since shifted.
        let undo = self
            .events
            .iter()
            .rev()
            .find(|event| {
                event.kind == EventKind::Undo
                    && event.target_operation_id.as_deref() == Some(target_id.as_str())
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    ErrorCode::HistoryUnavailable,
                    "the undo being redone is outside retained history",
                )
            })?;
        let patch = self.rebase(undo.revision, &undo.patch.inverse(), false)?;
        let event = self.commit(
            actor,
            operation_id,
            EventKind::Redo,
            Some(target_id.clone()),
            patch,
            hash,
            Some(&target),
        )?;
        self.undone.remove(&target_id);
        if let Some(stack) = self.redo.get_mut(&actor) {
            if stack.last() == Some(&target_id) {
                stack.pop();
            }
        }
        Ok(event)
    }
    /// The project as it was right after `revision`, rebuilt from retained events.
    fn project_at(&self, revision: Revision) -> Result<ProjectV2> {
        let later = self.events_since(revision)?;
        let mut project = self.project.clone();
        for event in later.iter().rev() {
            event.patch.inverse().apply_trusted(&mut project)?;
        }
        Ok(project)
    }
    /// Re-express `patch`, prepared against `base_revision`, against the
    /// current project by merging it with everything accepted since.
    fn rebase(
        &self,
        base_revision: Revision,
        patch: &ProjectPatch,
        renumber: bool,
    ) -> Result<ProjectPatch> {
        let base = self.project_at(base_revision)?;
        let mut ours = base.clone();
        patch.apply_trusted(&mut ours).map_err(|e| {
            if e.code == ErrorCode::Conflict {
                error(
                    ErrorCode::InvalidRequest,
                    "the edit does not match the revision it claims to be based on",
                )
            } else {
                e
            }
        })?;
        let merged = merge_projects(&base, &ours, &self.project, renumber)?;
        ProjectPatch::diff(&self.project, &merged)
    }
    /// Receipts make retries idempotent. A client only retries an operation for
    /// as long as it is in flight, so receipts are kept for retained events and
    /// a generous window of recent operations instead of for the room's life.
    fn prune_receipts(&mut self) {
        if self.receipts.len() <= MAX_RECEIPTS + MAX_RECEIPTS / 8 {
            return;
        }
        let oldest_event = self.events.first().map_or(Revision::MAX, |e| e.revision);
        let mut newest: Vec<(Revision, String)> = self
            .receipts
            .iter()
            .map(|(id, receipt)| (receipt.revision, id.clone()))
            .collect();
        newest.sort_unstable_by(|a, b| b.cmp(a));
        let keep: BTreeSet<String> = newest
            .into_iter()
            .enumerate()
            .filter(|(index, (revision, _))| *index < MAX_RECEIPTS || *revision >= oldest_event)
            .map(|(_, (_, id))| id)
            .collect();
        self.receipts.retain(|id, _| keep.contains(id));
    }
    pub fn events_since(&self, revision: Revision) -> Result<Vec<RoomEvent>> {
        if revision > self.revision {
            return Err(error(
                ErrorCode::RevisionAhead,
                "requested revision is newer than the room",
            ));
        }
        if revision < self.revision
            && self
                .events
                .first()
                .is_none_or(|event| revision.saturating_add(1) < event.revision)
        {
            return Err(error(
                ErrorCode::HistoryUnavailable,
                "requested revision is outside retained history; reload the room snapshot",
            ));
        }
        Ok(self
            .events
            .iter()
            .filter(|event| event.revision > revision)
            .cloned()
            .collect())
    }
    pub fn validate_loaded(&self) -> Result<()> {
        if self.state_version != STATE_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "room checkpoint has an unsupported schema version",
            ));
        }
        validate_project(&self.project)?;
        if !(1..=256).contains(&self.policy.max_members)
            || self.members.len() > self.policy.max_members
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "room checkpoint has invalid member limits",
            ));
        }
        let owner = self
            .members
            .get(&self.owner_id)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "room checkpoint has no owner"))?;
        if owner.rank != Rank::Owner || owner.banned || self.owner_id <= 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "room checkpoint has an invalid owner",
            ));
        }
        for (id, member) in &self.members {
            if *id != member.user_id
                || *id <= 0
                || (*id != self.owner_id && member.rank == Rank::Owner)
            {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "room checkpoint has invalid membership",
                ));
            }
        }
        if self.events.len() > MAX_EVENTS
            || self.receipts.len() > MAX_RECEIPTS
            || serde_json::to_vec(&self.events)
                .map_err(|e| error(ErrorCode::InvalidRequest, e.to_string()))?
                .len()
                > MAX_EVENT_BYTES
        {
            return Err(error(
                ErrorCode::TooLarge,
                "room checkpoint history exceeds supported limits",
            ));
        }
        if let Some(last) = self.events.last() {
            if last.revision != self.revision {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "room checkpoint revision disagrees with history",
                ));
            }
        }
        if self
            .events
            .windows(2)
            .any(|pair| pair[1].revision != pair[0].revision + 1)
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "room checkpoint history is not contiguous",
            ));
        }
        Ok(())
    }
}
fn fingerprint_for_event(event: &RoomEvent) -> Result<String> {
    fingerprint(
        event.kind,
        (event.kind == EventKind::Edit).then_some(&event.patch),
    )
}

fn canonicalize_wire_maps(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                canonicalize_wire_maps(value);
                if matches!(
                    key.as_str(),
                    "instance_names" | "frame_labels" | "library_item_folders" | "layer_metadata"
                ) {
                    if let serde_json::Value::Array(pairs) = value {
                        pairs.sort_by_cached_key(|pair| {
                            serde_json::to_string(pair).unwrap_or_default()
                        });
                    }
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                canonicalize_wire_maps(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use v2::LayerKind;
    use v2::{Anchor, Path, Placement, Q0rg, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset};
    fn project() -> ProjectV2 {
        let asset = |id: u16| {
            Asset::Vector(VectorAsset {
                asset_id: id,
                fill: Some(Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                }),
                stroke: None,
                paths: vec![Path {
                    closed: true,
                    anchors: [[0., 0.], [8., 0.], [8., 8.], [0., 8.]]
                        .into_iter()
                        .map(|[x, y]| Anchor {
                            point: Vec2::new(x, y),
                            in_handle: None,
                            out_handle: None,
                        })
                        .collect(),
                }],
            })
        };
        let layer = |id: u16, asset: Option<u16>| Layer {
            layer_id: id,
            timeline_frame_count: None,
            name: format!("layer {id}"),
            explicit_keyframes: if asset.is_some() { vec![0] } else { Vec::new() },
            placements: asset
                .map(|id| Placement {
                    instance_id: u32::from(id),
                    frame: 0,
                    target: Target::Asset(id),
                    transform: Transform2D::IDENTITY,
                    pivot: None,
                    tween: Tween::None,
                    fx: Default::default(),
                })
                .into_iter()
                .collect(),
        };
        ProjectV2 {
            meta: ProjectMeta {
                name: "room".into(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                background_color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![asset(1), asset(2), asset(3)],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            audio_clips: Vec::new(),
            layer_metadata: std::collections::HashMap::from([
                (
                    LayerKey::new(1, 10),
                    LayerMetadata {
                        kind: LayerKind::Folder,
                        ..Default::default()
                    },
                ),
                (
                    LayerKey::new(1, 20),
                    LayerMetadata {
                        kind: LayerKind::Folder,
                        ..Default::default()
                    },
                ),
                (
                    LayerKey::new(1, 1),
                    LayerMetadata {
                        parent_folder_id: Some(10),
                        ..Default::default()
                    },
                ),
            ])
            .into_iter()
            .collect(),
            runtime: ProjectRuntimeData {
                library_folders: vec!["bob".into(), "alice".into()],
                library_item_folders: std::collections::HashMap::from([(
                    LibraryItemReference::Asset(3),
                    "alice".into(),
                )]),
                ..Default::default()
            },
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 12,
                script: String::new(),
                layers: vec![
                    layer(1, Some(1)),
                    layer(10, None),
                    layer(2, Some(2)),
                    layer(20, None),
                ],
            }],
        }
    }
    /// A room where ranks edit only what they are assigned, which is what
    /// the permission tests exercise. New rooms let editors edit everything.
    fn scoped_room() -> RoomState {
        let mut room = RoomState::new(1, project()).unwrap();
        room.policy.editor_grants.clear();
        room.policy.admin_grants.clear();
        room
    }
    fn grant(scope: Scope) -> Grant {
        Grant { scope }
    }
    fn editor(room: &mut RoomState, user: UserId, scope: Scope) {
        room.set_member(
            1,
            Member {
                user_id: user,
                rank: Rank::Editor,
                grants: vec![grant(scope)],
                banned: false,
            },
        )
        .unwrap();
    }
    fn own_layer() -> Scope {
        Scope::Layer {
            q0rg_id: 1,
            layer_id: 1,
        }
    }
    fn rename(project: &ProjectV2, layer_id: u16, name: &str) -> ProjectV2 {
        let mut p = project.clone();
        p.q0rgs[0]
            .layers
            .iter_mut()
            .find(|l| l.layer_id == layer_id)
            .unwrap()
            .name = name.into();
        p
    }
    fn request(id: &str, revision: u64, before: &ProjectV2, after: &ProjectV2) -> OperationRequest {
        OperationRequest {
            operation_id: id.into(),
            base_revision: revision,
            patch: ProjectPatch::between(before, after).unwrap(),
        }
    }
    fn assert_code<T: std::fmt::Debug>(result: Result<T>, code: ErrorCode) {
        assert_eq!(result.unwrap_err().code, code);
    }

    #[test]
    fn patch_roundtrip_preserves_every_project_resource_and_folder_order() {
        let a = project();
        let mut b = rename(&a, 1, "new name");
        b.asset_names.insert(1, "one".into());
        b.runtime.library_folders.reverse();
        let patch = ProjectPatch::between(&a, &b).unwrap();
        let mut applied = a.clone();
        patch.apply(&mut applied).unwrap();
        assert_eq!(applied, b);
        patch.inverse().apply(&mut applied).unwrap();
        assert_eq!(applied, a);
    }
    #[test]
    fn viewer_is_read_only_even_with_a_forged_project_grant() {
        let mut room = scoped_room();
        let mut viewer = Member::viewer(2);
        viewer.grants = vec![grant(Scope::Project)];
        room.set_member(1, viewer).unwrap();
        let next = rename(&room.project, 1, "forged");
        assert_code(
            room.submit(2, request("viewer", 0, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        assert_eq!(room.revision, 0);
    }
    #[test]
    fn layer_and_recursive_folder_grants_are_enforced_on_the_server() {
        let mut room = scoped_room();
        editor(
            &mut room,
            2,
            Scope::LayerFolder {
                q0rg_id: 1,
                layer_id: 10,
            },
        );
        let next = rename(&room.project, 1, "allowed");
        room.submit(2, request("own", 0, &room.project, &next))
            .unwrap();
        let next = rename(&room.project, 2, "not allowed");
        assert_code(
            room.submit(2, request("foreign", 1, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        let mut moved = room.project.clone();
        moved
            .layer_metadata
            .get_mut(&LayerKey::new(1, 1))
            .unwrap()
            .parent_folder_id = Some(20);
        let index = moved.q0rgs[0]
            .layers
            .iter()
            .position(|layer| layer.layer_id == 1)
            .unwrap();
        let layer = moved.q0rgs[0].layers.remove(index);
        let index = moved.q0rgs[0]
            .layers
            .iter()
            .position(|layer| layer.layer_id == 20)
            .unwrap();
        moved.q0rgs[0].layers.insert(index, layer);
        assert_code(
            room.submit(2, request("escape", 1, &room.project, &moved)),
            ErrorCode::Forbidden,
        );
        assert_eq!(room.project.layer_parent_folder(1, 1), Some(10));
    }
    #[test]
    fn folder_assignment_cannot_capture_someone_elses_layer() {
        let mut room = scoped_room();
        editor(
            &mut room,
            2,
            Scope::LayerFolder {
                q0rg_id: 1,
                layer_id: 10,
            },
        );
        let mut p = room.project.clone();
        p.layer_metadata.insert(
            LayerKey::new(1, 2),
            LayerMetadata {
                parent_folder_id: Some(10),
                ..Default::default()
            },
        );
        let index = p.q0rgs[0]
            .layers
            .iter()
            .position(|layer| layer.layer_id == 2)
            .unwrap();
        let layer = p.q0rgs[0].layers.remove(index);
        p.q0rgs[0].layers.insert(0, layer);
        assert_code(
            room.submit(2, request("capture", 0, &room.project, &p)),
            ErrorCode::Forbidden,
        );
    }
    #[test]
    fn shared_asset_needs_all_referencing_layers_or_an_explicit_library_grant() {
        let mut p = project();
        for (layer_id, id) in [(1, 31), (2, 32)] {
            let layer = p.q0rgs[0]
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
                .unwrap();
            let mut placement = layer.placements[0].clone();
            placement.target = Target::Asset(3);
            placement.instance_id = id;
            layer.placements.push(placement);
        }
        let mut room = RoomState::new(1, p).unwrap();
        room.policy.editor_grants.clear();
        editor(&mut room, 2, own_layer());
        let mut next = room.project.clone();
        next.asset_names.insert(3, "shared".into());
        assert_code(
            room.submit(2, request("shared-denied", 0, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        editor(
            &mut room,
            2,
            Scope::LibraryFolder {
                path: "alice".into(),
            },
        );
        room.submit(2, request("shared-allowed", 0, &room.project, &next))
            .unwrap();
        let mut moved = room.project.clone();
        moved
            .runtime
            .library_item_folders
            .insert(LibraryItemReference::Asset(3), "bob".into());
        assert_code(
            room.submit(2, request("library-escape", 1, &room.project, &moved)),
            ErrorCode::Forbidden,
        );
    }
    #[test]
    fn scoped_editor_can_create_artwork_only_inside_their_layer() {
        let mut room = scoped_room();
        editor(&mut room, 2, own_layer());
        let mut next = room.project.clone();
        let mut asset = next.assets[0].clone();
        if let Asset::Vector(v) = &mut asset {
            v.asset_id = 4;
        }
        next.assets.push(asset);
        let layer = next.q0rgs[0]
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == 1)
            .unwrap();
        let mut placement = layer.placements[0].clone();
        placement.instance_id = 4;
        placement.target = Target::Asset(4);
        layer.placements.push(placement);
        room.submit(2, request("new-art", 0, &room.project, &next))
            .unwrap();
        assert!(room.project.assets.iter().any(|a| a.id() == 4));
    }
    #[test]
    fn moderation_does_not_grant_rank_management_or_owner_access() {
        let mut room = scoped_room();
        for id in [2, 3] {
            room.set_member(
                1,
                Member {
                    user_id: id,
                    rank: Rank::Admin,
                    grants: Vec::new(),
                    banned: false,
                },
            )
            .unwrap();
        }
        room.set_member(1, Member::viewer(4)).unwrap();
        assert_code(room.set_member(2, Member::viewer(1)), ErrorCode::Forbidden);
        assert_code(
            room.set_policy(2, RoomPolicy::default()),
            ErrorCode::Forbidden,
        );
        assert_code(room.ban(2, 1, true), ErrorCode::Forbidden);
        assert_code(room.ban(2, 3, true), ErrorCode::Forbidden);
        room.ban(2, 4, true).unwrap();
        assert!(!room.can_view(4));
        assert_code(room.admit_viewer(1, 4), ErrorCode::Forbidden);
        room.ban(1, 4, false).unwrap();
        assert!(room.can_view(4));
    }
    #[test]
    fn owner_policy_changes_are_bounded_and_nonexistent_grants_fail() {
        let mut room = scoped_room();
        for count in [0, 257, usize::MAX] {
            let policy = RoomPolicy {
                max_members: count,
                ..RoomPolicy::default()
            };
            assert_code(room.set_policy(1, policy), ErrorCode::InvalidRequest);
        }
        assert_code(
            room.set_member(
                1,
                Member {
                    user_id: 2,
                    rank: Rank::Editor,
                    grants: vec![grant(Scope::Layer {
                        q0rg_id: 1,
                        layer_id: 999,
                    })],
                    banned: false,
                },
            ),
            ErrorCode::InvalidRequest,
        );
        assert_code(
            room.set_member(
                1,
                Member {
                    user_id: 2,
                    rank: Rank::Owner,
                    grants: Vec::new(),
                    banned: false,
                },
            ),
            ErrorCode::Forbidden,
        );
        let policy = RoomPolicy {
            editor_grants: vec![grant(own_layer())],
            ..RoomPolicy::default()
        };
        room.set_policy(1, policy).unwrap();
        room.set_member(
            1,
            Member {
                user_id: 2,
                rank: Rank::Editor,
                grants: Vec::new(),
                banned: false,
            },
        )
        .unwrap();
        assert!(room.can_edit(
            2,
            &ResourceKey::Layer {
                q0rg_id: 1,
                layer_id: 1
            }
        ));
    }
    #[test]
    fn stale_disjoint_edits_and_personal_undo_preserve_other_users_work() {
        let mut room = scoped_room();
        editor(&mut room, 2, own_layer());
        editor(
            &mut room,
            3,
            Scope::Layer {
                q0rg_id: 1,
                layer_id: 2,
            },
        );
        let before = room.project.clone();
        room.submit(
            2,
            request("alice", 0, &before, &rename(&before, 1, "alice")),
        )
        .unwrap();
        room.submit(3, request("bob", 0, &before, &rename(&before, 2, "bob")))
            .unwrap();
        room.undo(2, "undo-alice".into()).unwrap();
        assert_eq!(
            room.project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 1)
                .unwrap()
                .name,
            "layer 1"
        );
        assert_eq!(
            room.project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 2)
                .unwrap()
                .name,
            "bob"
        );
        room.redo(2, "redo-alice".into()).unwrap();
        assert_eq!(
            room.project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 1)
                .unwrap()
                .name,
            "alice"
        );
        assert_eq!(
            room.project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 2)
                .unwrap()
                .name,
            "bob"
        );
    }
    #[test]
    fn undo_conflicts_atomically_instead_of_erasing_a_foreign_change() {
        let mut room = scoped_room();
        editor(&mut room, 2, own_layer());
        let next = rename(&room.project, 1, "alice");
        room.submit(2, request("alice", 0, &room.project, &next))
            .unwrap();
        let next = rename(&room.project, 1, "owner");
        room.submit(1, request("owner", 1, &room.project, &next))
            .unwrap();
        let before = room.project.clone();
        assert_code(room.undo(2, "undo".into()), ErrorCode::Conflict);
        assert_eq!(room.project, before);
        assert_eq!(room.revision, 2);
    }
    #[test]
    fn operation_ids_are_idempotent_and_atomic_batches_cannot_smuggle_edits() {
        let mut room = scoped_room();
        editor(&mut room, 2, own_layer());
        let next = rename(&room.project, 1, "alice");
        let req = request("op", 0, &room.project, &next);
        let event = room.submit(2, req.clone()).unwrap();
        assert_eq!(room.submit(2, req).unwrap(), event);
        assert_eq!(room.revision, 1);
        let next = rename(&room.project, 1, "changed");
        assert_code(
            room.submit(2, request("op", 1, &room.project, &next)),
            ErrorCode::Conflict,
        );
        let next = rename(&rename(&room.project, 1, "partial"), 2, "smuggled");
        let before = room.project.clone();
        assert_code(
            room.submit(2, request("batch", 1, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        assert_eq!(room.project, before);
        let mut patch = ProjectPatch::between(&before, &rename(&before, 1, "wrong key")).unwrap();
        patch.changes[0].key = ResourceKey::Meta;
        assert_code(patch.apply(&mut room.project), ErrorCode::Conflict);
    }
    #[test]
    fn serialized_state_keeps_personal_history_and_rejects_future_schemas() {
        let mut room = scoped_room();
        let next = rename(&room.project, 1, "edit");
        room.submit(1, request("op", 0, &room.project, &next))
            .unwrap();
        let bytes = serde_json::to_vec(&room).unwrap();
        let mut restored: RoomState = serde_json::from_slice(&bytes).unwrap();
        restored.validate_loaded().unwrap();
        restored.undo(1, "undo".into()).unwrap();
        assert_eq!(
            restored.project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 1)
                .unwrap()
                .name,
            "layer 1"
        );
        restored.state_version = 99;
        assert_code(restored.validate_loaded(), ErrorCode::UnsupportedVersion);
    }
    #[test]
    fn evicted_event_ids_remain_tombstoned_and_old_reconnects_resync() {
        let mut room = scoped_room();
        let first = request(
            "first",
            0,
            &room.project,
            &rename(&room.project, 1, "first"),
        );
        room.submit(1, first.clone()).unwrap();
        for n in 1..260 {
            let next = rename(&room.project, 1, &format!("edit {n}"));
            room.submit(
                1,
                request(&format!("op-{n}"), room.revision, &room.project, &next),
            )
            .unwrap();
        }
        assert!(room.events.len() <= 256);
        let revision = room.revision;
        assert_code(room.submit(1, first), ErrorCode::HistoryUnavailable);
        assert_eq!(room.revision, revision);
        assert_code(room.events_since(0), ErrorCode::HistoryUnavailable);
        assert_eq!(room.events_since(revision).unwrap().len(), 0);
        let restored: RoomState =
            serde_json::from_slice(&serde_json::to_vec(&room).unwrap()).unwrap();
        restored.validate_loaded().unwrap();
    }
    #[test]
    fn nonfinite_presence_and_invalid_projects_are_rejected() {
        assert_code(
            Presence {
                x: Some(f32::NAN),
                ..Default::default()
            }
            .validate(),
            ErrorCode::InvalidRequest,
        );
        let mut p = project();
        p.meta.fps = 0;
        assert_code(RoomState::new(1, p), ErrorCode::InvalidProject);
    }
    #[test]
    fn deleted_highest_ids_can_be_reused_but_stale_grants_and_requests_cannot_capture_them() {
        let mut room = scoped_room();
        editor(
            &mut room,
            2,
            Scope::LayerFolder {
                q0rg_id: 1,
                layer_id: 20,
            },
        );
        let original = room.project.clone();
        let stale = request("stale", 0, &original, &rename(&original, 20, "old edit"));
        let mut deleted = original.clone();
        deleted.q0rgs[0].layers.retain(|layer| layer.layer_id != 20);
        deleted.layer_metadata.remove(&LayerKey::new(1, 20));
        room.submit(1, request("delete", 0, &room.project, &deleted))
            .unwrap();
        assert!(room.members[&2].grants.is_empty());
        room.submit(1, request("recreate", 1, &room.project, &original))
            .unwrap();
        assert!(!room.can_edit(
            2,
            &ResourceKey::Layer {
                q0rg_id: 1,
                layer_id: 20
            }
        ));
        assert_code(room.submit(1, stale), ErrorCode::Conflict);
        assert_eq!(room.revision, 2);
        room.validate_loaded().unwrap();
    }
    #[test]
    fn undo_distinguishes_a_same_looking_replacement_from_the_original_layer() {
        let mut room = scoped_room();
        editor(&mut room, 2, own_layer());
        let edited = rename(&room.project, 1, "mine");
        room.submit(2, request("mine", 0, &room.project, &edited))
            .unwrap();
        let mut deleted = edited.clone();
        deleted.q0rgs[0].layers.retain(|layer| layer.layer_id != 1);
        deleted.layer_metadata.remove(&LayerKey::new(1, 1));
        room.submit(1, request("delete", 1, &room.project, &deleted))
            .unwrap();
        room.submit(1, request("replacement", 2, &room.project, &edited))
            .unwrap();
        editor(&mut room, 2, own_layer());
        assert_code(
            room.undo(2, "undo-old-identity".into()),
            ErrorCode::Conflict,
        );
        assert_eq!(room.project, edited);
    }
    #[test]
    fn undoing_deletion_restores_original_identity_for_earlier_personal_undo() {
        let mut room = scoped_room();
        let renamed = rename(&room.project, 20, "edited");
        room.submit(1, request("rename", 0, &room.project, &renamed))
            .unwrap();
        let mut deleted = room.project.clone();
        deleted.q0rgs[0].layers.retain(|layer| layer.layer_id != 20);
        deleted.layer_metadata.remove(&LayerKey::new(1, 20));
        room.submit(1, request("delete", 1, &room.project, &deleted))
            .unwrap();
        room.undo(1, "undo-delete".into()).unwrap();
        room.undo(1, "undo-rename".into()).unwrap();
        assert_eq!(
            room.project.q0rgs[0].layers.last().unwrap().name,
            "layer 20"
        );
        room.redo(1, "redo-rename".into()).unwrap();
        assert_eq!(room.project.q0rgs[0].layers.last().unwrap().name, "edited");
    }
    #[test]
    fn admin_moderation_does_not_implicitly_authorize_artwork_changes() {
        let mut room = scoped_room();
        room.set_member(
            1,
            Member {
                user_id: 2,
                rank: Rank::Admin,
                grants: Vec::new(),
                banned: false,
            },
        )
        .unwrap();
        room.set_member(1, Member::viewer(3)).unwrap();
        assert!(!room.can_edit(2, &ResourceKey::Meta));
        let next = rename(&room.project, 1, "admin edit");
        assert_code(
            room.submit(2, request("admin-edit", 0, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        room.ban(2, 3, true).unwrap();
        assert!(!room.can_view(3));
    }
    #[test]
    fn room_projects_require_embedded_dependencies() {
        let mut p = project();
        p.runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "linked".into(),
                kind: v2::ProjectDependencyKind::Movie,
                source: v2::ProjectDependencySource::External("private/project.q1s".into()),
            });
        let error = RoomState::new(1, p).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidProject);
        assert!(error.message.contains("embed external"));
    }
    #[test]
    fn scoped_inserts_allow_index_bookkeeping_without_reordering_protected_siblings() {
        let mut room = scoped_room();
        editor(
            &mut room,
            2,
            Scope::LayerFolder {
                q0rg_id: 1,
                layer_id: 10,
            },
        );
        let mut next = room.project.clone();
        let mut layer = next.q0rgs[0].layers[0].clone();
        layer.layer_id = 4;
        layer.name = "new".into();
        layer.placements.clear();
        next.layer_metadata.insert(
            LayerKey::new(1, 4),
            LayerMetadata {
                parent_folder_id: Some(10),
                ..Default::default()
            },
        );
        next.q0rgs[0].layers.insert(1, layer);
        room.submit(2, request("insert-child", 0, &room.project, &next))
            .unwrap();
        editor(
            &mut room,
            3,
            Scope::LibraryFolder {
                path: "alice".into(),
            },
        );
        let mut next = room.project.clone();
        next.runtime.library_folders.push("alice/props".into());
        next.runtime.library_folders.sort();
        room.submit(3, request("insert-library-folder", 1, &room.project, &next))
            .unwrap();
        let mut next = room.project.clone();
        let index = next.q0rgs[0]
            .layers
            .iter()
            .position(|l| l.layer_id == 2)
            .unwrap();
        let layer = next.q0rgs[0].layers.remove(index);
        next.q0rgs[0].layers.push(layer);
        assert_code(
            room.submit(3, request("reorder-protected", 2, &room.project, &next)),
            ErrorCode::Forbidden,
        );
    }
    #[test]
    fn crafted_noncanonical_positions_are_rejected_before_accepting_events() {
        let mut room = scoped_room();
        let mut patch =
            ProjectPatch::between(&room.project, &rename(&room.project, 1, "name")).unwrap();
        if let Some(ResourceValue::Layer(layer)) = &mut patch.changes[0].after {
            layer.position = 999;
        }
        assert_code(
            room.submit(
                1,
                OperationRequest {
                    operation_id: "bad-order".into(),
                    base_revision: 0,
                    patch,
                },
            ),
            ErrorCode::InvalidRequest,
        );
        assert_eq!(room.revision, 0);
    }
    #[test]
    fn advisory_access_matches_server_authorization_and_names_its_source() {
        let mut room = scoped_room();
        room.set_member(
            1,
            Member {
                user_id: 2,
                rank: Rank::Editor,
                grants: vec![grant(Scope::LayerFolder {
                    q0rg_id: 1,
                    layer_id: 10,
                })],
                banned: false,
            },
        )
        .unwrap();
        room.set_member(1, Member::viewer(3)).unwrap();
        let mut admin = Member::viewer(4);
        admin.rank = Rank::Admin;
        room.set_member(1, admin).unwrap();
        let mut policy = room.policy.clone();
        policy.editor_grants = vec![grant(Scope::Layer {
            q0rg_id: 1,
            layer_id: 2,
        })];
        room.set_policy(1, policy).unwrap();
        for user in 1..=4 {
            let member = &room.members[&user];
            for layer in &room.project.q0rgs[0].layers {
                let access = layer_access(&room.project, &room.policy, member, 1, layer.layer_id);
                let key = ResourceKey::Layer {
                    q0rg_id: 1,
                    layer_id: layer.layer_id,
                };
                assert_eq!(
                    access.allowed(),
                    room.can_edit(user, &key),
                    "{user} {key:?}"
                );
            }
            assert_eq!(
                project_access(&room.policy, member).allowed(),
                room.can_edit(user, &ResourceKey::Meta)
            );
        }
        let editor = &room.members[&2];
        let folder_child = layer_access(&room.project, &room.policy, editor, 1, 1);
        assert!(folder_child.assigned && !folder_child.rank_default);
        let default_layer = layer_access(&room.project, &room.policy, editor, 1, 2);
        assert!(default_layer.rank_default && !default_layer.assigned);
        assert!(layer_access(&room.project, &room.policy, &room.members[&1], 1, 20).owner);
        assert!(!library_access(&room.policy, &room.members[&4], "alice").allowed());
        let mut banned = editor.clone();
        banned.banned = true;
        assert!(!layer_access(&room.project, &room.policy, &banned, 1, 1).allowed());
    }

    /// What the editor does for a stroke: a new vector asset with the next id
    /// at the end of the asset list, placed on a layer at a frame.
    fn draw(project: &ProjectV2, layer_id: u16, frame: u16, size: f32) -> ProjectV2 {
        let mut next = project.clone();
        let id = next.assets.iter().map(Asset::id).max().unwrap_or(0) + 1;
        let mut asset = next.assets[0].clone();
        if let Asset::Vector(vector) = &mut asset {
            vector.asset_id = id;
            vector.paths[0].anchors[2].point = Vec2::new(size, size);
        }
        next.assets.push(asset);
        let layer = next.q0rgs[0]
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        if !layer.explicit_keyframes.contains(&frame) {
            layer.explicit_keyframes.push(frame);
            layer.explicit_keyframes.sort_unstable();
        }
        layer.placements.push(Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(id),
            transform: Transform2D::IDENTITY,
            pivot: None,
            tween: Tween::None,
            fx: Default::default(),
        });
        next
    }
    fn strokes(project: &ProjectV2, layer_id: u16) -> Vec<(u16, f32)> {
        let layer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap();
        layer
            .placements
            .iter()
            .filter_map(|placement| match placement.target {
                Target::Asset(id) => project.assets.iter().find(|a| a.id() == id).map(|asset| {
                    let Asset::Vector(vector) = asset else {
                        unreachable!()
                    };
                    (placement.frame, vector.paths[0].anchors[2].point.x)
                }),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn two_people_drawing_at_once_both_keep_their_strokes() {
        let mut room = RoomState::new(1, project()).unwrap();
        room.set_member(
            1,
            Member {
                rank: Rank::Editor,
                ..Member::viewer(2)
            },
        )
        .unwrap();
        room.set_member(
            1,
            Member {
                rank: Rank::Admin,
                ..Member::viewer(3)
            },
        )
        .unwrap();
        let base = room.project.clone();
        // the same layer, the same frame, the same new asset id, prepared at once
        let a = draw(&base, 2, 0, 11.0);
        let b = draw(&base, 2, 0, 22.0);
        let c = draw(&base, 1, 5, 33.0);
        room.submit(2, request("a", 0, &base, &a)).unwrap();
        room.submit(3, request("b", 0, &base, &b)).unwrap();
        room.submit(1, request("c", 0, &base, &c)).unwrap();
        let mut sizes: Vec<f32> = strokes(&room.project, 2).iter().map(|s| s.1).collect();
        sizes.sort_by(f32::total_cmp);
        assert_eq!(
            sizes,
            vec![8.0, 11.0, 22.0],
            "both strokes on the shared layer survive"
        );
        assert!(strokes(&room.project, 1).contains(&(5, 33.0)));
        let ids: BTreeSet<u16> = room.project.assets.iter().map(Asset::id).collect();
        assert_eq!(
            ids.len(),
            room.project.assets.len(),
            "new strokes got distinct ids"
        );
        assert_eq!(room.revision, 3);
        // every client replaying the events reaches the server's document
        let mut replay = base;
        for event in room.events_since(0).unwrap() {
            event.patch.apply(&mut replay).unwrap();
        }
        assert_eq!(replay, room.project);
    }
    #[test]
    fn personal_undo_survives_later_work_and_only_removes_your_own() {
        let mut room = RoomState::new(1, project()).unwrap();
        room.set_member(
            1,
            Member {
                rank: Rank::Editor,
                ..Member::viewer(2)
            },
        )
        .unwrap();
        let mine = draw(&room.project, 2, 0, 11.0);
        room.submit(2, request("mine", 0, &room.project, &mine))
            .unwrap();
        let theirs = draw(&room.project, 2, 0, 22.0);
        room.submit(1, request("theirs", 1, &room.project, &theirs))
            .unwrap();
        let assets = room.project.assets.len();
        room.undo(2, "undo-mine".into()).unwrap();
        let sizes: Vec<f32> = strokes(&room.project, 2).iter().map(|s| s.1).collect();
        assert_eq!(
            sizes,
            vec![8.0, 22.0],
            "their later stroke stays, mine is gone"
        );
        assert_eq!(room.project.assets.len(), assets - 1);
        room.redo(2, "redo-mine".into()).unwrap();
        let mut sizes: Vec<f32> = strokes(&room.project, 2).iter().map(|s| s.1).collect();
        sizes.sort_by(f32::total_cmp);
        assert_eq!(sizes, vec![8.0, 11.0, 22.0]);
    }
    #[test]
    fn same_field_still_conflicts_but_different_fields_merge() {
        let mut room = RoomState::new(1, project()).unwrap();
        room.set_member(
            1,
            Member {
                rank: Rank::Editor,
                ..Member::viewer(2)
            },
        )
        .unwrap();
        let base = room.project.clone();
        room.submit(
            1,
            request("rename-1", 0, &base, &rename(&base, 2, "owner name")),
        )
        .unwrap();
        assert_code(
            room.submit(
                2,
                request("rename-2", 0, &base, &rename(&base, 2, "editor name")),
            ),
            ErrorCode::Conflict,
        );
        // drawing on the renamed layer is a different field of the same layer
        room.submit(2, request("draw", 0, &base, &draw(&base, 2, 3, 44.0)))
            .unwrap();
        let layer = room.project.q0rgs[0]
            .layers
            .iter()
            .find(|l| l.layer_id == 2)
            .unwrap();
        assert_eq!(layer.name, "owner name");
        assert!(strokes(&room.project, 2).contains(&(3, 44.0)));
        // reordering layers while someone draws keeps both
        let mut moved = room.project.clone();
        let index = moved.q0rgs[0]
            .layers
            .iter()
            .position(|l| l.layer_id == 20)
            .unwrap();
        let layer = moved.q0rgs[0].layers.remove(index);
        moved.q0rgs[0].layers.insert(0, layer);
        let before_move = room.project.clone();
        let revision = room.revision;
        room.submit(1, request("move", revision, &before_move, &moved))
            .unwrap();
        room.submit(
            2,
            request(
                "draw-2",
                revision,
                &before_move,
                &draw(&before_move, 2, 6, 55.0),
            ),
        )
        .unwrap();
        assert_eq!(room.project.q0rgs[0].layers[0].layer_id, 20);
        assert!(strokes(&room.project, 2).contains(&(6, 55.0)));
    }
    #[test]
    fn new_rooms_let_editors_and_admins_draw_and_receipts_never_end_a_room() {
        let mut room = RoomState::new(1, project()).unwrap();
        room.set_member(
            1,
            Member {
                rank: Rank::Admin,
                ..Member::viewer(3)
            },
        )
        .unwrap();
        room.set_member(1, Member::viewer(4)).unwrap();
        let next = draw(&room.project, 1, 0, 9.0);
        assert_code(
            room.submit(4, request("viewer", 0, &room.project, &next)),
            ErrorCode::Forbidden,
        );
        room.submit(3, request("admin", 0, &room.project, &next))
            .unwrap();
        for index in 0..(MAX_RECEIPTS + MAX_RECEIPTS / 8 + 1) {
            room.receipts.insert(
                format!("old-{index}"),
                Receipt {
                    actor: 3,
                    kind: EventKind::Edit,
                    fingerprint: String::new(),
                    revision: 0,
                },
            );
        }
        let again = rename(&room.project, 1, "still alive");
        room.submit(3, request("after-many", 1, &room.project, &again))
            .unwrap();
        assert!(room.receipts.len() <= MAX_RECEIPTS + 1);
        assert!(room.receipts.contains_key("after-many"));
    }
}
