use std::path::{Path, PathBuf};

use q0s_format::{
    is_q0s_v2, parse_q0s, parse_q0s_v2, parse_q1s,
    v2::{self, Asset, ProjectDependencySource, ProjectV2},
    Q1Project,
};
use q0video::q0v::Q0vFile;

use crate::swf::{find_projector_swf, looks_like_swf, parse_swf, SwfMovie};
use crate::swf_assets::{self, Category};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Q1s,
    Q0s,
    Q0v,
    Swf,
    FlashProjector,
}

impl FormatKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Q1s => "q1s project",
            Self::Q0s => "q0s movie",
            Self::Q0v => "q0v media",
            Self::Swf => "swf movie",
            Self::FlashProjector => "flash projector exe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EditRef {
    QProjectQ0rgScript {
        path: Vec<u16>,
        q0rg_id: u16,
    },
    QProjectFrameScript {
        path: Vec<u16>,
        q0rg_id: u16,
        layer_id: u16,
        frame: u16,
    },
    QProjectQ0lang {
        path: Vec<u16>,
        node_id: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetVisualKind {
    Bitmap,
    Vector,
    Media,
    Rig,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PreviewRef {
    QProjectAsset {
        path: Vec<u16>,
        asset_id: u16,
        kind: AssetVisualKind,
    },
    QProjectQ0rg {
        path: Vec<u16>,
        q0rg_id: u16,
    },
    QProjectEmbedded {
        path: Vec<u16>,
        node_id: u16,
    },
    Q1LegacyBitmap(u16),
    Q0LegacyBitmap(u16),
    Q0vStandaloneVideo,
    Q0vStandaloneAudio,
    SwfTag(usize),
    /// The main timeline, playable frame by frame.
    SwfTimeline,
    /// One frame of the main timeline, from zero.
    SwfFrame(u32),
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub id: usize,
    pub label: String,
    pub detail: String,
    pub preview: Option<PreviewRef>,
    pub edit: Option<EditRef>,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone)]
pub enum DocumentData {
    Q1Legacy(Q1Project),
    QProject(Box<ProjectV2>),
    Q0Legacy(q0s_format::Movie),
    Q0v(Q0vFile),
    Swf(SwfMovie),
    Projector { offset: usize, movie: SwfMovie },
}

#[derive(Debug, Clone)]
pub struct Document {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub format: FormatKind,
    pub summary: String,
    pub roots: Vec<TreeNode>,
    pub data: DocumentData,
    /// Where each node sits in `roots`, by id; built when first asked.
    lookup: std::sync::OnceLock<std::collections::HashMap<usize, Vec<usize>>>,
}

impl Document {
    pub fn open(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::from_bytes(path.to_path_buf(), bytes)
    }

    pub fn from_bytes(path: PathBuf, bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() < 4 {
            return Err("file is too short to identify".to_string());
        }

        match &bytes[0..4] {
            b"Q1S\0" => parse_q1_document(path, bytes),
            b"Q0S\0" => parse_q0_document(path, bytes),
            b"Q0V\0" => parse_q0v_document(path, bytes),
            _ if looks_like_swf(&bytes) => parse_swf_document(path, bytes),
            _ if bytes.starts_with(b"MZ") => parse_projector_document(path, bytes),
            _ => Err(format!(
                "unknown format: magic {:02x} {:02x} {:02x} {:02x}",
                bytes[0], bytes[1], bytes[2], bytes[3]
            )),
        }
    }

    pub fn node_detail(&self, id: usize) -> Option<&str> {
        self.node(id).map(|node| node.detail.as_str())
    }

    pub fn node_preview(&self, id: usize) -> Option<&PreviewRef> {
        self.node(id).and_then(|node| node.preview.as_ref())
    }

    pub fn node_edit(&self, id: usize) -> Option<&EditRef> {
        self.node(id).and_then(|node| node.edit.as_ref())
    }

    fn new(
        path: PathBuf,
        bytes: Vec<u8>,
        format: FormatKind,
        summary: String,
        roots: Vec<TreeNode>,
        data: DocumentData,
    ) -> Self {
        Self {
            path,
            bytes,
            format,
            summary,
            roots,
            data,
            lookup: std::sync::OnceLock::new(),
        }
    }

    /// Forgets where nodes were, after `roots` changed.
    pub fn rebuild_lookup(&mut self) {
        self.lookup = std::sync::OnceLock::new();
    }

    pub fn rebuild_tree(&mut self) {
        let roots = match &self.data {
            DocumentData::Q1Legacy(project) => {
                let mut ids = Ids::default();
                q1_legacy_tree(project, &mut ids)
            }
            DocumentData::QProject(project) => {
                let mut ids = Ids::default();
                let mut embedded_budget = 0usize;
                q_project_tree(project, &mut ids, &[], 0, &mut embedded_budget)
            }
            DocumentData::Q0Legacy(movie) => {
                let mut ids = Ids::default();
                q0_legacy_tree(movie, &mut ids)
            }
            DocumentData::Q0v(_) | DocumentData::Swf(_) | DocumentData::Projector { .. } => {
                return;
            }
        };
        self.roots = roots;
        self.lookup = std::sync::OnceLock::new();
    }

    pub fn node(&self, id: usize) -> Option<&TreeNode> {
        let lookup = self.lookup.get_or_init(|| {
            fn index(
                nodes: &[TreeNode],
                trail: &mut Vec<usize>,
                out: &mut std::collections::HashMap<usize, Vec<usize>>,
            ) {
                for (position, node) in nodes.iter().enumerate() {
                    trail.push(position);
                    out.insert(node.id, trail.clone());
                    index(&node.children, trail, out);
                    trail.pop();
                }
            }
            let mut out = std::collections::HashMap::new();
            index(&self.roots, &mut Vec::new(), &mut out);
            out
        });
        let trail = lookup.get(&id)?;
        let (first, rest) = trail.split_first()?;
        let mut node = self.roots.get(*first)?;
        for position in rest {
            node = node.children.get(*position)?;
        }
        Some(node)
    }
}

fn parse_q1_document(path: PathBuf, bytes: Vec<u8>) -> Result<Document, String> {
    if bytes.len() < 6 {
        return Err("q1s header is truncated".to_string());
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version == 1 {
        let project = parse_q1s(&bytes).map_err(|error| format!("parse q1s v1: {error}"))?;
        let mut ids = Ids::default();
        let roots = q1_legacy_tree(&project, &mut ids);
        let summary = format!(
            "{}\nformat: q1s v1\nproject: {}\nstage: {} x {}\nfps: {}\nassets: {}\nscenes: {}\nsize: {} bytes",
            path.display(),
            project.meta.name,
            project.meta.stage_width,
            project.meta.stage_height,
            project.meta.fps,
            project.assets.len(),
            project.scenes.len(),
            bytes.len()
        );
        Ok(Document::new(
            path,
            bytes,
            FormatKind::Q1s,
            summary,
            roots,
            DocumentData::Q1Legacy(project),
        ))
    } else {
        let project =
            v2::parse(&bytes).map_err(|error| format!("parse q1s v{version}: {error}"))?;
        q_project_document(path, bytes, FormatKind::Q1s, project, version)
    }
}

fn parse_q0_document(path: PathBuf, bytes: Vec<u8>) -> Result<Document, String> {
    if bytes.len() < 6 {
        return Err("q0s header is truncated".to_string());
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if is_q0s_v2(&bytes) {
        let project =
            parse_q0s_v2(&bytes).map_err(|error| format!("parse q0s v{version}: {error}"))?;
        q_project_document(path, bytes, FormatKind::Q0s, project, version)
    } else {
        let movie = parse_q0s(&bytes).map_err(|error| format!("parse q0s v{version}: {error}"))?;
        let mut ids = Ids::default();
        let roots = q0_legacy_tree(&movie, &mut ids);
        let placement_count: usize = movie.placements_by_frame.iter().map(Vec::len).sum();
        let summary = format!(
            "{}\nformat: q0s v{} (legacy bitmap movie)\nframes: {}\nfps: {}\nbitmaps: {}\nplacements: {}\nsize: {} bytes",
            path.display(),
            movie.header.version,
            movie.header.frame_count,
            movie.header.fps,
            movie.bitmaps.len(),
            placement_count,
            bytes.len()
        );
        Ok(Document::new(
            path,
            bytes,
            FormatKind::Q0s,
            summary,
            roots,
            DocumentData::Q0Legacy(movie),
        ))
    }
}

fn q_project_document(
    path: PathBuf,
    bytes: Vec<u8>,
    format: FormatKind,
    project: ProjectV2,
    version: u16,
) -> Result<Document, String> {
    let mut ids = Ids::default();
    let mut embedded_budget = 0usize;
    let roots = q_project_tree(&project, &mut ids, &[], 0, &mut embedded_budget);
    let layers: usize = project.q0rgs.iter().map(|q0rg| q0rg.layers.len()).sum();
    let placements: usize = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .map(|layer| layer.placements.len())
        .sum();
    let summary = format!(
        "{}\nformat: {} v{}\nproject: {}\nstage: {} x {}\nfps: {}\nassets: {}\nq0rgs: {}\nlayers: {}\nplacements: {}\naudio clips: {}\nproject links: {}\nframe scripts: {}\nsize: {} bytes",
        path.display(),
        format.label(),
        version,
        project.meta.name,
        project.meta.stage_width,
        project.meta.stage_height,
        project.meta.fps,
        project.assets.len(),
        project.q0rgs.len(),
        layers,
        placements,
        project.audio_clips.len(),
        project.runtime.project_graph.nodes.len(),
        project.runtime.frame_scripts.len(),
        bytes.len()
    );
    Ok(Document::new(
        path,
        bytes,
        format,
        summary,
        roots,
        DocumentData::QProject(Box::new(project)),
    ))
}

fn parse_q0v_document(path: PathBuf, bytes: Vec<u8>) -> Result<Document, String> {
    let media = Q0vFile::parse(bytes.clone()).map_err(|error| format!("parse q0v: {error}"))?;
    let mut ids = Ids::default();
    let mut streams = Vec::new();
    if media.spec.video {
        streams.push(TreeNode {
            id: ids.next(),
            label: format!("video • {} frames", media.frames.len()),
            detail: format!(
                "video stream\nresolution: {} x {}\nfps: {}\nframes: {}\ncodec: png frames",
                media.spec.width,
                media.spec.height,
                media.spec.fps,
                media.frames.len()
            ),
            preview: Some(PreviewRef::Q0vStandaloneVideo),
            edit: None,
            children: Vec::new(),
        });
    }
    if media.spec.audio {
        streams.push(TreeNode {
            id: ids.next(),
            label: "audio • pcm16".to_string(),
            detail: format!(
                "audio stream\nsample rate: {} hz\nchannels: {}\nsamples/channel: {}\nformat: signed 16-bit pcm little-endian",
                media.spec.audio_sample_rate,
                media.spec.audio_channels,
                media.audio_samples_per_channel
            ),
            preview: Some(PreviewRef::Q0vStandaloneAudio),
            edit: None,
            children: Vec::new(),
        });
    }
    let roots = vec![TreeNode {
        id: ids.next(),
        label: "streams".to_string(),
        detail: format!("{} stream(s)", streams.len()),
        preview: None,
        edit: None,
        children: streams,
    }];
    let summary = format!(
        "{}\nformat: q0v\ntimeline frames: {}\nfps/timescale: {}\nvideo: {}\naudio: {}\nsize: {} bytes",
        path.display(),
        media.spec.timeline_frames,
        media.spec.fps,
        if media.spec.video {
            format!("{} x {}, {} frames", media.spec.width, media.spec.height, media.frames.len())
        } else {
            "none".to_string()
        },
        if media.spec.audio {
            format!("{} hz, {} ch", media.spec.audio_sample_rate, media.spec.audio_channels)
        } else {
            "none".to_string()
        },
        bytes.len()
    );
    Ok(Document::new(
        path,
        bytes,
        FormatKind::Q0v,
        summary,
        roots,
        DocumentData::Q0v(media),
    ))
}

fn parse_swf_document(path: PathBuf, bytes: Vec<u8>) -> Result<Document, String> {
    let movie = parse_swf(&bytes)?;
    let roots = swf_tree(&movie);
    let summary = swf_summary(&path, &movie, bytes.len(), None);
    Ok(Document::new(
        path,
        bytes,
        FormatKind::Swf,
        summary,
        roots,
        DocumentData::Swf(movie),
    ))
}

fn parse_projector_document(path: PathBuf, bytes: Vec<u8>) -> Result<Document, String> {
    let embedded = find_projector_swf(&bytes)?;
    let roots = swf_tree(&embedded.movie);
    let summary = swf_summary(&path, &embedded.movie, bytes.len(), Some(embedded.offset));
    Ok(Document::new(
        path,
        bytes,
        FormatKind::FlashProjector,
        summary,
        roots,
        DocumentData::Projector {
            offset: embedded.offset,
            movie: embedded.movie,
        },
    ))
}

fn swf_summary(path: &Path, movie: &SwfMovie, source_size: usize, offset: Option<usize>) -> String {
    let mut text = format!(
        "{}\nformat: swf v{}\ncompression: {}\nstage: {:.1} x {:.1}\nfps: {:.3}\nframes: {}\ntags: {}\ndeclared swf size: {} bytes\nsource size: {} bytes",
        path.display(),
        movie.version,
        movie.compression.label(),
        movie.width_px,
        movie.height_px,
        movie.frame_rate,
        movie.frame_count,
        movie.tags.len(),
        movie.declared_file_length,
        source_size
    );
    if let Some(offset) = offset {
        text.push_str(&format!("\nembedded swf offset: 0x{offset:x} ({offset})"));
    }
    text
}

fn q1_legacy_tree(project: &Q1Project, ids: &mut Ids) -> Vec<TreeNode> {
    let assets = project
        .assets
        .iter()
        .map(|asset| TreeNode {
            id: ids.next(),
            label: format!(
                "bitmap {} • {}x{}",
                asset.asset_id, asset.width, asset.height
            ),
            detail: format!(
                "bitmap asset {}\nresolution: {} x {}\nrgba bytes: {}",
                asset.asset_id,
                asset.width,
                asset.height,
                asset.rgba.len()
            ),
            preview: Some(PreviewRef::Q1LegacyBitmap(asset.asset_id)),
            edit: None,
            children: Vec::new(),
        })
        .collect::<Vec<_>>();
    let scenes = project
        .scenes
        .iter()
        .map(|scene| {
            let layers = scene
                .layers
                .iter()
                .map(|layer| TreeNode {
                    id: ids.next(),
                    label: format!("{} • {} placements", layer.name, layer.placements.len()),
                    detail: format!(
                        "layer {}\nid: {}\nplacements: {}",
                        layer.name,
                        layer.layer_id,
                        layer.placements.len()
                    ),
                    preview: None,
                    edit: None,
                    children: Vec::new(),
                })
                .collect();
            TreeNode {
                id: ids.next(),
                label: format!("{} • {} frames", scene.name, scene.frame_count),
                detail: format!(
                    "scene {}\nid: {}\nframes: {}\nlayers: {}",
                    scene.name,
                    scene.scene_id,
                    scene.frame_count,
                    scene.layers.len()
                ),
                preview: None,
                edit: None,
                children: layers,
            }
        })
        .collect::<Vec<_>>();
    vec![
        TreeNode {
            id: ids.next(),
            label: format!("assets ({})", assets.len()),
            detail: "legacy q1s bitmap assets".to_string(),
            preview: None,
            edit: None,
            children: assets,
        },
        TreeNode {
            id: ids.next(),
            label: format!("scenes ({})", scenes.len()),
            detail: "legacy q1s scenes".to_string(),
            preview: None,
            edit: None,
            children: scenes,
        },
    ]
}

fn q0_legacy_tree(movie: &q0s_format::Movie, ids: &mut Ids) -> Vec<TreeNode> {
    let mut bitmaps = movie.bitmaps.values().collect::<Vec<_>>();
    bitmaps.sort_by_key(|bitmap| bitmap.id);
    let assets = bitmaps
        .into_iter()
        .map(|bitmap| TreeNode {
            id: ids.next(),
            label: format!("bitmap {} • {}x{}", bitmap.id, bitmap.width, bitmap.height),
            detail: format!(
                "bitmap {}\nresolution: {} x {}\nrgba bytes: {}",
                bitmap.id,
                bitmap.width,
                bitmap.height,
                bitmap.rgba.len()
            ),
            preview: Some(PreviewRef::Q0LegacyBitmap(bitmap.id)),
            edit: None,
            children: Vec::new(),
        })
        .collect::<Vec<_>>();
    vec![TreeNode {
        id: ids.next(),
        label: format!("bitmaps ({})", assets.len()),
        detail: "legacy q0s bitmap dictionary".to_string(),
        preview: None,
        edit: None,
        children: assets,
    }]
}

const MAX_EMBEDDED_TREE_DEPTH: usize = 32;
const MAX_EMBEDDED_TREE_BYTES: usize = 512 * 1024 * 1024;

fn q_project_tree(
    project: &ProjectV2,
    ids: &mut Ids,
    project_path: &[u16],
    depth: usize,
    embedded_budget: &mut usize,
) -> Vec<TreeNode> {
    let assets = project
        .assets
        .iter()
        .map(|asset| {
            let id = asset.id();
            let name = project
                .asset_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("asset {id}"));
            let (kind, visual_kind, detail) = match asset {
                Asset::Bitmap(bitmap) => (
                    "bitmap",
                    AssetVisualKind::Bitmap,
                    format!(
                        "bitmap asset {id}\nname: {name}\nresolution: {} x {}\nrgba bytes: {}",
                        bitmap.width,
                        bitmap.height,
                        bitmap.rgba.len()
                    ),
                ),
                Asset::Vector(vector) => {
                    let anchors: usize = vector.paths.iter().map(|path| path.anchors.len()).sum();
                    (
                        "vector",
                        AssetVisualKind::Vector,
                        format!(
                            "vector asset {id}\nname: {name}\npaths: {}\nanchors: {}\nfill: {}\nstroke: {}",
                            vector.paths.len(),
                            anchors,
                            vector.fill.is_some(),
                            vector.stroke.is_some()
                        ),
                    )
                }
                Asset::Q0v(media) => {
                    let probe = q0video::q0v::probe_header(&media.bytes)
                        .map(|header| {
                            format!(
                                "video: {}\naudio: {}\ntimeline frames: {}",
                                header.spec.video, header.spec.audio, header.spec.timeline_frames
                            )
                        })
                        .unwrap_or_else(|error| format!("invalid embedded q0v: {error}"));
                    (
                        "q0v",
                        AssetVisualKind::Media,
                        format!(
                            "q0v asset {id}\nname: {name}\nbytes: {}\n{}",
                            media.bytes.len(),
                            probe
                        ),
                    )
                }
                Asset::Model(model) => (
                    "model",
                    AssetVisualKind::Rig,
                    format!(
                        "3D model {id}\nname: {name}\nmeshes: {}\nvertices: {}\ntriangles: {}\nnodes: {}\nclips: {}",
                        model.data.meshes.len(),
                        model.data.vertex_count(),
                        model.data.triangle_count(),
                        model.data.nodes.len(),
                        model.data.clips.len()
                    ),
                ),
                Asset::Rig(rig) => (
                    "rig",
                    AssetVisualKind::Rig,
                    format!(
                        "rig asset {id}\nname: {name}\nowner q0rg: {}\nnodes: {}\ncontrols: {}\nconstraints: {}\nchannels: {}\ndrivers: {}\nposes: {}\ndeformers: {}\nvariants: {}",
                        rig.owner_q0rg_id,
                        rig.nodes.len(),
                        rig.controls.len(),
                        rig.constraints.len(),
                        rig.channels.len(),
                        rig.drivers.len(),
                        rig.poses.len(),
                        rig.deformers.len(),
                        rig.variants.len()
                    ),
                ),
            };
            TreeNode {
                id: ids.next(),
                label: format!("{name} • {kind} #{id}"),
                detail,
                preview: Some(PreviewRef::QProjectAsset {
                    path: project_path.to_vec(),
                    asset_id: id,
                    kind: visual_kind,
                }),
                edit: None,
                children: Vec::new(),
            }
        })
        .collect::<Vec<_>>();

    let q0rgs = project
        .q0rgs
        .iter()
        .map(|q0rg| {
            let layers = q0rg
                .layers
                .iter()
                .map(|layer| {
                    let meta = project
                        .layer_metadata
                        .get(&v2::LayerKey::new(q0rg.q0rg_id, layer.layer_id))
                        .copied()
                        .unwrap_or_default();
                    TreeNode {
                        id: ids.next(),
                        label: format!(
                            "{} • {} keys • {} placements",
                            layer.name,
                            layer.keyframe_frames().len(),
                            layer.placements.len()
                        ),
                        detail: format!(
                            "layer {}\nid: {}\nkind: {:?}\nparent folder: {:?}\nhidden: {}\nlocked: {}\nexplicit keys: {}\nplacements: {}",
                            layer.name,
                            layer.layer_id,
                            meta.kind,
                            meta.parent_folder_id,
                            meta.hidden,
                            meta.locked,
                            layer.explicit_keyframes.len(),
                            layer.placements.len()
                        ),
                        preview: None,
                        edit: None,
                        children: Vec::new(),
                    }
                })
                .collect();
            TreeNode {
                id: ids.next(),
                label: format!("{} • {} frames", q0rg.name, q0rg.frame_count),
                detail: format!(
                    "q0rg {}\nid: {}\nframes: {}\nlayers: {}\nscript bytes: {}",
                    q0rg.name,
                    q0rg.q0rg_id,
                    q0rg.frame_count,
                    q0rg.layers.len(),
                    q0rg.script.len()
                ),
                preview: Some(PreviewRef::QProjectQ0rg {
                    path: project_path.to_vec(),
                    q0rg_id: q0rg.q0rg_id,
                }),
                edit: Some(EditRef::QProjectQ0rgScript {
                    path: project_path.to_vec(),
                    q0rg_id: q0rg.q0rg_id,
                }),
                children: layers,
            }
        })
        .collect::<Vec<_>>();

    let runtime_nodes = project
        .runtime
        .project_graph
        .nodes
        .iter()
        .map(|node| {
            let (source, preview, children) = match &node.source {
                ProjectDependencySource::External(path) => {
                    (format!("external: {path}"), None, Vec::new())
                }
                ProjectDependencySource::Embedded(bytes) => {
                    let preview = Some(PreviewRef::QProjectEmbedded {
                        path: project_path.to_vec(),
                        node_id: node.node_id,
                    });
                    let children = if node.kind == v2::ProjectDependencyKind::Movie {
                        embedded_movie_tree(
                            bytes,
                            ids,
                            project_path,
                            node.node_id,
                            depth,
                            embedded_budget,
                        )
                    } else {
                        Vec::new()
                    };
                    (format!("embedded: {} bytes", bytes.len()), preview, children)
                }
            };
            TreeNode {
                id: ids.next(),
                label: format!("{} • {:?}", node.alias, node.kind),
                detail: format!(
                    "project dependency\nid: {}\nparent: {:?}\nalias: {}\nkind: {:?}\nsource: {}\ncontainer depth: {}",
                    node.node_id,
                    node.parent_node_id,
                    node.alias,
                    node.kind,
                    source,
                    project_path.len()
                ),
                preview,
                edit: match (&node.kind, &node.source) {
                    (v2::ProjectDependencyKind::Q0lang, ProjectDependencySource::Embedded(_)) => {
                        Some(EditRef::QProjectQ0lang {
                            path: project_path.to_vec(),
                            node_id: node.node_id,
                        })
                    }
                    _ => None,
                },
                children,
            }
        })
        .collect::<Vec<_>>();

    let scripts = project
        .runtime
        .frame_scripts
        .iter()
        .enumerate()
        .map(|(index, script)| TreeNode {
            id: ids.next(),
            label: format!(
                "frame script {} • q0rg {} / layer {} / frame {}",
                index, script.q0rg_id, script.layer_id, script.frame
            ),
            detail: format!(
                "frame script\nq0rg: {}\nlayer: {}\nframe: {}\nbytes: {}\n\n{}",
                script.q0rg_id,
                script.layer_id,
                script.frame,
                script.source.len(),
                script.source
            ),
            preview: None,
            edit: Some(EditRef::QProjectFrameScript {
                path: project_path.to_vec(),
                q0rg_id: script.q0rg_id,
                layer_id: script.layer_id,
                frame: script.frame,
            }),
            children: Vec::new(),
        })
        .collect::<Vec<_>>();

    vec![
        TreeNode {
            id: ids.next(),
            label: format!("assets ({})", assets.len()),
            detail: "project assets".to_string(),
            preview: None,
            edit: None,
            children: assets,
        },
        TreeNode {
            id: ids.next(),
            label: format!("q0rgs ({})", q0rgs.len()),
            detail: "symbols, scenes, and timelines".to_string(),
            preview: None,
            edit: None,
            children: q0rgs,
        },
        TreeNode {
            id: ids.next(),
            label: format!("project graph ({})", runtime_nodes.len()),
            detail: "runtime project dependencies; embedded movies expand recursively".to_string(),
            preview: None,
            edit: None,
            children: runtime_nodes,
        },
        TreeNode {
            id: ids.next(),
            label: format!("frame scripts ({})", scripts.len()),
            detail: "q0lang attached to timeline frames".to_string(),
            preview: None,
            edit: None,
            children: scripts,
        },
    ]
}

fn embedded_movie_tree(
    bytes: &[u8],
    ids: &mut Ids,
    parent_path: &[u16],
    node_id: u16,
    depth: usize,
    embedded_budget: &mut usize,
) -> Vec<TreeNode> {
    if depth >= MAX_EMBEDDED_TREE_DEPTH {
        return vec![TreeNode {
            id: ids.next(),
            label: "recursive contents stopped • depth limit".to_string(),
            detail: format!("canripper stops recursive expansion after {MAX_EMBEDDED_TREE_DEPTH} embedded movies"),
            preview: None,
            edit: None,
            children: Vec::new(),
        }];
    }
    let Some(next_budget) = embedded_budget.checked_add(bytes.len()) else {
        return embedded_budget_guard_node(ids);
    };
    if next_budget > MAX_EMBEDDED_TREE_BYTES {
        return embedded_budget_guard_node(ids);
    }
    *embedded_budget = next_budget;

    let (nested, format_label) = match parse_embedded_project(bytes) {
        Ok(value) => value,
        Err(error) => {
            return vec![TreeNode {
                id: ids.next(),
                label: "contents unavailable".to_string(),
                detail: error,
                preview: None,
                edit: None,
                children: Vec::new(),
            }]
        }
    };
    let mut nested_path = parent_path.to_vec();
    nested_path.push(node_id);
    let roots = q_project_tree(&nested, ids, &nested_path, depth + 1, embedded_budget);
    vec![TreeNode {
        id: ids.next(),
        label: format!("inside • {format_label} • {}", nested.meta.name),
        detail: format!(
            "recursive embedded movie\nformat: {format_label}\nproject: {}\nstage: {} x {}\nfps: {}\nassets: {}\nq0rgs: {}\npath depth: {}",
            nested.meta.name,
            nested.meta.stage_width,
            nested.meta.stage_height,
            nested.meta.fps,
            nested.assets.len(),
            nested.q0rgs.len(),
            nested_path.len()
        ),
        preview: Some(PreviewRef::QProjectQ0rg {
            path: nested_path,
            q0rg_id: nested.meta.entry_q0rg_id,
        }),
        edit: None,
        children: roots,
    }]
}

fn embedded_budget_guard_node(ids: &mut Ids) -> Vec<TreeNode> {
    vec![TreeNode {
        id: ids.next(),
        label: "recursive contents stopped • size limit".to_string(),
        detail: format!(
            "canripper caps recursively parsed embedded movie data at {} MiB",
            MAX_EMBEDDED_TREE_BYTES / (1024 * 1024)
        ),
        preview: None,
        edit: None,
        children: Vec::new(),
    }]
}

fn parse_embedded_project(bytes: &[u8]) -> Result<(ProjectV2, &'static str), String> {
    if bytes.starts_with(b"Q0S\0") {
        if !is_q0s_v2(bytes) {
            return Err(
                "embedded movie is a legacy q0s; recursive v2 project structure is unavailable"
                    .to_string(),
            );
        }
        return parse_q0s_v2(bytes)
            .map(|project| (project, "q0s"))
            .map_err(|error| format!("parse embedded q0s: {error}"));
    }
    if bytes.starts_with(b"Q1S\0") {
        if bytes.get(4..6) == Some(&1u16.to_le_bytes()) {
            return Err(
                "embedded movie is a legacy q1s; recursive v2 project structure is unavailable"
                    .to_string(),
            );
        }
        return v2::parse(bytes)
            .map(|project| (project, "q1s"))
            .map_err(|error| format!("parse embedded q1s: {error}"));
    }
    Err("embedded movie is not a q0s/q1s container".to_string())
}
/// The tags of an SWF sorted the way JPEXS shows them: a folder for each kind
/// of character, the frames of the main timeline, scripts, everything else,
/// and the whole tag stream in order.
fn swf_tree(movie: &SwfMovie) -> Vec<TreeNode> {
    let mut ids = Ids::default();
    let mut names = std::collections::HashMap::new();
    for tag in movie.tags.iter().filter(|tag| matches!(tag.code, 56 | 76)) {
        for (id, name) in swf_assets::exported_names(tag) {
            names.insert(id, name);
        }
    }
    let tag_detail = |tag: &crate::swf::SwfTag| {
        let mut detail = format!(
            "tag: {}\ncode: {}\nindex: {}\npayload offset: 0x{:x}\npayload length: {} bytes",
            tag.name,
            tag.code,
            tag.index,
            tag.payload_offset,
            tag.payload.len()
        );
        if let Some(id) = swf_assets::character_id(tag) {
            detail.push_str(&format!("\ncharacter id: {id}"));
            if let Some(name) = names.get(&id) {
                detail.push_str(&format!("\nexported as: {name}"));
            }
        }
        if let Some(about) = swf_assets::describe(tag) {
            detail.push_str(&format!("\nholds: {about}"));
        }
        detail
    };
    let tag_label = |tag: &crate::swf::SwfTag| {
        let mut label = match swf_assets::character_id(tag) {
            Some(id) => format!("{} ({id})", tag.name),
            None => tag.name.to_string(),
        };
        if let Some(name) = swf_assets::character_id(tag).and_then(|id| names.get(&id)) {
            label.push_str(&format!(" “{name}”"));
        }
        if let Some(about) = swf_assets::describe(tag) {
            label.push_str(&format!(" • {about}"));
        }
        label
    };

    let mut folders: std::collections::BTreeMap<Category, Vec<TreeNode>> = Default::default();
    for tag in &movie.tags {
        let category = Category::of(tag.code);
        if category == Category::Control {
            continue;
        }
        folders.entry(category).or_default().push(TreeNode {
            id: ids.next(),
            label: tag_label(tag),
            detail: tag_detail(tag),
            preview: Some(PreviewRef::SwfTag(tag.index)),
            edit: None,
            children: Vec::new(),
        });
    }

    let frames = swf_assets::frames(&movie.tags)
        .into_iter()
        .enumerate()
        .map(|(number, frame)| {
            let listed = frame
                .tags
                .iter()
                .filter_map(|index| movie.tags.get(*index))
                .map(|tag| format!("  {:>6}  {}", tag.index, tag.name))
                .collect::<Vec<_>>()
                .join("\n");
            TreeNode {
                id: ids.next(),
                label: match &frame.label {
                    Some(label) => format!("frame {} “{label}”", number + 1),
                    None => format!("frame {}", number + 1),
                },
                detail: format!(
                    "frame {} of the main timeline\nlabel: {}\ntags: {}\n\n{listed}",
                    number + 1,
                    frame.label.as_deref().unwrap_or("-"),
                    frame.tags.len()
                ),
                preview: Some(PreviewRef::SwfFrame(number as u32)),
                edit: None,
                children: Vec::new(),
            }
        })
        .collect::<Vec<_>>();

    let all = movie
        .tags
        .iter()
        .map(|tag| TreeNode {
            id: ids.next(),
            label: format!("{:05} {}", tag.index, tag_label(tag)),
            detail: tag_detail(tag),
            preview: Some(PreviewRef::SwfTag(tag.index)),
            edit: None,
            children: Vec::new(),
        })
        .collect::<Vec<_>>();

    let folder = |ids: &mut Ids, label: &str, detail: &str, children: Vec<TreeNode>| TreeNode {
        id: ids.next(),
        label: format!("{label} ({})", children.len()),
        detail: detail.to_string(),
        preview: None,
        edit: None,
        children,
    };
    let mut roots = vec![TreeNode {
        id: ids.next(),
        label: format!("movie • {} frames", movie.frame_count),
        detail: format!(
            "main timeline\nstage: {} × {} px\nframe rate: {} fps\nframes: {}",
            movie.width_px, movie.height_px, movie.frame_rate, movie.frame_count
        ),
        preview: Some(PreviewRef::SwfTimeline),
        edit: None,
        children: Vec::new(),
    }];
    for (category, children) in folders {
        let detail = format!("{} tags of this kind", children.len());
        roots.push(folder(&mut ids, category.folder(), &detail, children));
    }
    roots.push(folder(
        &mut ids,
        "frames",
        "the frames of the main timeline and the tags each one runs",
        frames,
    ));
    roots.push(folder(
        &mut ids,
        "all tags",
        "the whole tag stream, in order",
        all,
    ));
    roots
}

#[derive(Default)]
struct Ids(usize);

impl Ids {
    fn next(&mut self) -> usize {
        let id = self.0;
        self.0 += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_extension_only_fake_file() {
        let error = Document::from_bytes(PathBuf::from("fake.swf"), b"nope".to_vec()).unwrap_err();
        assert!(error.contains("unknown format"));
    }

    #[test]
    fn q1s_fixture_is_identified_from_magic_not_extension() {
        let bytes = std::fs::read("../q0s-format/testdata/empty.q1s").expect("fixture");
        let doc = Document::from_bytes(PathBuf::from("renamed.bin"), bytes).expect("open q1s");
        assert_eq!(doc.format, FormatKind::Q1s);
    }

    fn minimal_project(name: &str) -> ProjectV2 {
        ProjectV2 {
            meta: v2::ProjectMeta {
                name: name.to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                background_color: q0s_format::v2::Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![v2::Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 1,
                script: String::new(),
                layers: vec![v2::Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "Layer 1".to_string(),
                    explicit_keyframes: vec![0],
                    placements: Vec::new(),
                }],
            }],
        }
    }

    fn find_preview(nodes: &[TreeNode], wanted: &PreviewRef) -> bool {
        nodes.iter().any(|node| {
            node.preview.as_ref() == Some(wanted) || find_preview(&node.children, wanted)
        })
    }

    #[test]
    fn embedded_q0s_tree_recurses_into_embedded_q0s_internals() {
        let mut deepest = minimal_project("deepest");
        deepest.assets.push(Asset::Vector(v2::VectorAsset {
            asset_id: 7,
            paths: vec![v2::Path {
                anchors: vec![
                    v2::Anchor {
                        point: v2::Vec2::new(2.0, 2.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    v2::Anchor {
                        point: v2::Vec2::new(20.0, 2.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    v2::Anchor {
                        point: v2::Vec2::new(10.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(v2::Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        }));
        let deepest_bytes = q0s_format::write_q0s_v2(&deepest).expect("deepest q0s");

        let mut middle = minimal_project("middle");
        middle
            .runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 2,
                parent_node_id: None,
                alias: "deepest".to_string(),
                kind: v2::ProjectDependencyKind::Movie,
                source: v2::ProjectDependencySource::Embedded(deepest_bytes),
            });
        let middle_bytes = q0s_format::write_q0s_v2(&middle).expect("middle q0s");

        let mut outer = minimal_project("outer");
        outer
            .runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "middle".to_string(),
                kind: v2::ProjectDependencyKind::Movie,
                source: v2::ProjectDependencySource::Embedded(middle_bytes),
            });
        let outer_bytes = q0s_format::write_q0s_v2(&outer).expect("outer q0s");
        let document = Document::from_bytes(PathBuf::from("outer.q0s"), outer_bytes)
            .expect("open recursive q0s");

        let wanted = PreviewRef::QProjectAsset {
            path: vec![1, 2],
            asset_id: 7,
            kind: AssetVisualKind::Vector,
        };
        assert!(
            find_preview(&document.roots, &wanted),
            "deepest asset was not exposed through recursive tree"
        );
        let preview =
            crate::preview::build_preview(&document, &wanted, 0).expect("preview deepest vector");
        assert!(matches!(preview, crate::preview::PreviewContent::Vector(_)));
    }
}
