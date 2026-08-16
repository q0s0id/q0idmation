//! Export a `ProjectV2` to the `.q0s` player format.
//!
//! The player format is a versioned wrapper:
//!   * **v1** вЂ” legacy bitmap-only `Movie` (one bitmap per frame). Big
//!     and pixelated; we no longer write it. Old files still play
//!     because q0player's loader recognises the v1 magic+version.
//!   * **v2** вЂ” full vector data: assets (vector paths + bitmaps), q0rg
//!     hierarchy, transforms, tweens. Same on-disk layout as `.q1s` but
//!     with `Q0S\0` magic. q0player rasterises each frame at playback
//!     time via the shared software rasteriser, so the file stays small
//!     and looks crisp at any zoom.
//!
//! The editor writes v2 so exported files retain the full project structure.

use std::path::Path;

use q0s_format::v2::{ProjectDependencyKind, ProjectDependencySource, ProjectV2};

/// Public entry: produce raw bytes for a `.q0s` v2 file.
pub fn export_to_q0s_bytes(project: &ProjectV2) -> Result<Vec<u8>, q0s_format::Error> {
    q0s_format::write_q0s_v2(project)
}

#[derive(Debug, Clone)]
pub struct BundledProjectExport {
    pub project: ProjectV2,
    pub bytes: Vec<u8>,
}

const MAX_BUNDLE_DEPTH: usize = 64;
const MAX_BUNDLED_DEPENDENCY_BYTES: u64 = 256 * 1024 * 1024;

fn resolve_dependency_path(
    base_file: Option<&Path>,
    stored: &str,
) -> Result<std::path::PathBuf, String> {
    let dependency = Path::new(stored);
    if dependency.is_absolute() {
        return Ok(dependency.to_path_buf());
    }
    let base_dir = base_file.and_then(Path::parent).ok_or_else(|| {
        format!("cannot resolve relative dependency `{stored}` without its owning project path")
    })?;
    Ok(base_dir.join(dependency))
}

fn read_bounded_dependency(path: &Path, alias: &str) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        format!(
            "cannot stat dependency `{alias}` at {}: {error}",
            path.display()
        )
    })?;
    if metadata.len() > MAX_BUNDLED_DEPENDENCY_BYTES {
        return Err(format!(
            "dependency `{alias}` is too large to bundle ({} bytes; limit is {})",
            metadata.len(),
            MAX_BUNDLED_DEPENDENCY_BYTES
        ));
    }
    std::fs::read(path).map_err(|error| {
        format!(
            "cannot read dependency `{alias}` from {}: {error}",
            path.display()
        )
    })
}

fn validate_q0lang_dependency(alias: &str, bytes: &[u8]) -> Result<(), String> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| format!("q0lang dependency `{alias}` is not utf-8: {error}"))?;
    let compiled = q0s_format::q0lang::compiler::compile_source(source);
    if compiled.fatal || !compiled.diagnostics.is_empty() {
        let summary = compiled
            .diagnostics
            .iter()
            .take(4)
            .map(|diagnostic| format!("line {}: {}", diagnostic.line, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "q0lang dependency `{alias}` did not compile cleanly{}{}",
            if summary.is_empty() { "" } else { ": " },
            summary
        ));
    }
    Ok(())
}

fn bundle_project_recursive(
    project: &ProjectV2,
    owner_path: Option<&Path>,
    depth: usize,
    active_movies: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Result<ProjectV2, String> {
    if depth > MAX_BUNDLE_DEPTH {
        return Err(format!(
            "project bundle nesting exceeds {MAX_BUNDLE_DEPTH} movies"
        ));
    }

    let mut bundled = project.clone();
    for node in &mut bundled.runtime.project_graph.nodes {
        // parent_node_id only describes the authoring namespace/tree. Every
        // source path in this ProjectV2 is stored relative to this owning
        // project, so parented nodes must be embedded too. A linked movie's
        // *own* graph is additionally bundled when that movie is parsed below.
        match node.kind {
            ProjectDependencyKind::Q0lang => {
                let bytes = match &node.source {
                    ProjectDependencySource::Embedded(bytes) => bytes.clone(),
                    ProjectDependencySource::External(stored) => {
                        let resolved = resolve_dependency_path(owner_path, stored)?;
                        read_bounded_dependency(&resolved, &node.alias)?
                    }
                };
                validate_q0lang_dependency(&node.alias, &bytes)?;
                node.source = ProjectDependencySource::Embedded(bytes);
            }
            ProjectDependencyKind::Movie => {
                let (bytes, child_path, canonical_guard) = match &node.source {
                    ProjectDependencySource::Embedded(bytes) => (bytes.clone(), None, None),
                    ProjectDependencySource::External(stored) => {
                        let resolved = resolve_dependency_path(owner_path, stored)?;
                        let canonical =
                            std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
                        if !active_movies.insert(canonical.clone()) {
                            return Err(format!(
                                "movie dependency cycle while bundling `{}` at {}",
                                node.alias,
                                resolved.display()
                            ));
                        }
                        (
                            read_bounded_dependency(&resolved, &node.alias)?,
                            Some(resolved),
                            Some(canonical),
                        )
                    }
                };
                let child = q0s_format::parse_q0s_v2(&bytes).map_err(|error| {
                    format!(
                        "movie dependency `{}` is not a valid vector q0s: {error}",
                        node.alias
                    )
                })?;
                let child = bundle_project_recursive(
                    &child,
                    child_path.as_deref(),
                    depth + 1,
                    active_movies,
                )?;
                if let Some(canonical) = canonical_guard {
                    active_movies.remove(&canonical);
                }
                let bytes = q0s_format::write_q0s_v2(&child).map_err(|error| {
                    format!(
                        "serialize bundled movie dependency `{}`: {error}",
                        node.alias
                    )
                })?;
                // Parse-back is deliberately required for every nested movie,
                // not only the root export. A bad child must never be hidden in
                // an otherwise valid outer q0s.
                let reparsed = q0s_format::parse_q0s_v2(&bytes).map_err(|error| {
                    format!(
                        "parse-back bundled movie dependency `{}`: {error}",
                        node.alias
                    )
                })?;
                if !q0s_format::v2::wire_equivalent(&reparsed, &child) {
                    return Err(format!(
                        "parse-back changed bundled movie dependency `{}`",
                        node.alias
                    ));
                }
                node.source = ProjectDependencySource::Embedded(bytes);
            }
        }
    }
    Ok(bundled)
}

/// Build a self-contained runtime q0s. Root q0lang sources and linked vector
/// q0s movies are embedded; each linked movie is recursively bundled using its
/// own directory as the base for relative dependencies. Authoring q1s links
/// remain untouched because only a clone is rewritten.
pub fn export_with_embedded_dependencies(
    project: &ProjectV2,
    source_project_path: Option<&Path>,
) -> Result<BundledProjectExport, String> {
    let mut active_movies = std::collections::HashSet::new();
    let bundled = bundle_project_recursive(project, source_project_path, 0, &mut active_movies)?;
    let bytes = q0s_format::write_q0s_v2(&bundled)
        .map_err(|error| format!("serialize bundled q0s: {error}"))?;
    let parsed = q0s_format::parse_q0s_v2(&bytes)
        .map_err(|error| format!("parse-back bundled q0s: {error}"))?;
    if !q0s_format::v2::wire_equivalent(&parsed, &bundled) {
        return Err("parse-back changed the bundled canonical project".to_string());
    }
    Ok(BundledProjectExport {
        project: bundled,
        bytes,
    })
}

/// Atomically publish a self-contained runtime q0s and verify the bytes after
/// they reach disk. This is the non-UI equivalent of q0enc's q0s export path.
pub fn export_bundled_q0s_to_path(
    path: &Path,
    project: &ProjectV2,
    source_project_path: Option<&Path>,
) -> Result<BundledProjectExport, String> {
    let bundled = export_with_embedded_dependencies(project, source_project_path)?;
    crate::file_io::write_bytes_atomic(path, &bundled.bytes)
        .map_err(|error| format!("write bundled q0s: {error}"))?;
    let reread = std::fs::read(path).map_err(|error| format!("reread bundled q0s: {error}"))?;
    let reparsed = q0s_format::parse_q0s_v2(&reread)
        .map_err(|error| format!("parse-back written bundled q0s: {error}"))?;
    if !q0s_format::v2::wire_equivalent(&reparsed, &bundled.project) {
        return Err("written bundled q0s does not match the canonical export".to_string());
    }
    Ok(bundled)
}

/// Compatibility name for callers from the first q0lang-only bundling pass.
/// It now bundles movie dependencies too.
pub fn export_with_embedded_q0lang(
    project: &ProjectV2,
    source_project_path: Option<&Path>,
) -> Result<BundledProjectExport, String> {
    export_with_embedded_dependencies(project, source_project_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Anchor, Asset, Layer, Path as VPath, Placement, ProjectDependencyKind,
        ProjectDependencyNode, ProjectDependencySource, ProjectMeta, Q0rg, Rgba, Target,
        Transform2D, Tween, Vec2, VectorAsset,
    };

    fn one_red_square_project() -> ProjectV2 {
        let asset = Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(30.0, 10.0),
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
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        });
        ProjectV2 {
            meta: ProjectMeta {
                name: "t".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![asset],
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
                    name: "L".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        }
    }

    #[test]
    fn bundled_q0lang_survives_source_file_removal() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "q0editor-bundle-q0lang-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create bundle test dir");
        let source_path = dir.join("game.q0l");
        let project_path = dir.join("game.q1s");
        let source = "import q0.time\npb func step x {\n  return x + 1\n}\n";
        std::fs::write(&source_path, source).expect("write q0l");

        let mut project = one_red_square_project();
        project
            .runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "game".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("game.q0l".into()),
            });
        let bundled = export_with_embedded_q0lang(&project, Some(&project_path))
            .expect("bundle external q0lang");
        std::fs::remove_file(&source_path).expect("remove source after bundle");

        let parsed = q0s_format::parse_q0s_v2(&bundled.bytes).expect("parse bundled q0s");
        assert!(matches!(
            &parsed.runtime.project_graph.nodes[0].source,
            ProjectDependencySource::Embedded(bytes) if bytes == source.as_bytes()
        ));
        assert!(matches!(
            &project.runtime.project_graph.nodes[0].source,
            ProjectDependencySource::External(path) if path == "game.q0l"
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup bundle test dir");
    }

    #[test]
    fn bundler_embeds_parented_authoring_tree_nodes_too() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "q0editor-bundle-parented-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create parented bundle test dir");
        let movie_path = dir.join("room.q0s");
        let logic_path = dir.join("room_logic.q0l");
        let root_path = dir.join("root.q1s");
        std::fs::write(
            &movie_path,
            q0s_format::write_q0s_v2(&one_red_square_project()).expect("serialize room"),
        )
        .expect("write room");
        std::fs::write(&logic_path, "x = 1\n").expect("write parented logic");

        let mut root = one_red_square_project();
        root.runtime.project_graph.nodes.extend([
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "room".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::External("room.q0s".into()),
            },
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: Some(1),
                alias: "logic".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("room_logic.q0l".into()),
            },
        ]);
        let bundled = export_with_embedded_dependencies(&root, Some(&root_path))
            .expect("bundle parented node");
        std::fs::remove_file(&movie_path).expect("remove movie source");
        std::fs::remove_file(&logic_path).expect("remove q0lang source");
        assert!(bundled
            .project
            .runtime
            .project_graph
            .nodes
            .iter()
            .all(|node| matches!(node.source, ProjectDependencySource::Embedded(_))));
        std::fs::remove_dir_all(&dir).expect("cleanup parented bundle test dir");
    }

    #[test]
    fn bundled_movie_and_its_q0lang_survive_source_removal() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "q0editor-bundle-movie-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create movie bundle test dir");
        let child_logic_path = dir.join("room_logic.q0l");
        let child_movie_path = dir.join("room2.q0s");
        let root_project_path = dir.join("root.q1s");
        let source = "import q0.time\npb func roomTick x {\n  return x + 1\n}\n";
        std::fs::write(&child_logic_path, source).expect("write child q0l");

        let mut child = one_red_square_project();
        child.meta.name = "room two".into();
        child
            .runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "room_logic".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("room_logic.q0l".into()),
            });
        std::fs::write(
            &child_movie_path,
            q0s_format::write_q0s_v2(&child).expect("write child q0s bytes"),
        )
        .expect("write child q0s");

        let mut root = one_red_square_project();
        root.runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "room2".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::External("room2.q0s".into()),
            });
        let bundled = export_with_embedded_dependencies(&root, Some(&root_project_path))
            .expect("bundle linked movie recursively");

        std::fs::remove_file(&child_movie_path).expect("remove child movie after bundle");
        std::fs::remove_file(&child_logic_path).expect("remove child q0l after bundle");

        let root_after = q0s_format::parse_q0s_v2(&bundled.bytes).expect("parse root bundle");
        let ProjectDependencySource::Embedded(child_bytes) =
            &root_after.runtime.project_graph.nodes[0].source
        else {
            panic!("linked movie must be embedded");
        };
        let child_after = q0s_format::parse_q0s_v2(child_bytes).expect("parse embedded child");
        assert_eq!(child_after.meta.name, "room two");
        assert!(matches!(
            &child_after.runtime.project_graph.nodes[0].source,
            ProjectDependencySource::Embedded(bytes) if bytes == source.as_bytes()
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup movie bundle test dir");
    }

    #[test]
    fn export_v2_round_trips_through_player_loader() {
        let project = one_red_square_project();
        let bytes = export_to_q0s_bytes(&project).expect("export");
        assert!(q0s_format::is_q0s_v2(&bytes));
        let parsed = q0s_format::parse_q0s_v2(&bytes).expect("parse");
        assert_eq!(parsed, project);
    }

    #[test]
    fn export_v2_is_dramatically_smaller_than_a_bitmap_snapshot_would_be() {
        // Sanity: vector .q0s should be tiny compared to even a single
        // pre-baked frame bitmap (stage_w*stage_h*4 = 16384 bytes for
        // 64Г—64). One asset + one placement is well under 200 bytes.
        let project = one_red_square_project();
        let bytes = export_to_q0s_bytes(&project).expect("export");
        let bitmap_per_frame = (64 * 64 * 4) as usize;
        assert!(
            bytes.len() < bitmap_per_frame,
            "vector .q0s ({}B) should beat bitmap-per-frame ({}B)",
            bytes.len(),
            bitmap_per_frame
        );
    }

    #[test]
    fn export_preserves_rig_binding_channels_and_instance_identity() {
        let mut project = one_red_square_project();
        project.q0rgs[0].layers[0].placements[0].instance_id = 44;
        crate::rigging::ensure_rig(&mut project, 1).expect("rig");
        let bone = crate::rigging::add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .expect("bone");
        let selection = crate::state::Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        crate::rigging::bind_selected_placement_to_node(&mut project, &selection, bone, 0)
            .expect("bind");
        crate::rigging::set_node_rotation(&mut project, 1, bone, 1, 0.35, true);

        let bytes = export_to_q0s_bytes(&project).expect("export rigged q0s");
        let parsed = q0s_format::parse_q0s_v2(&bytes).expect("parse rigged q0s");
        assert!(q0s_format::v2::wire_equivalent(&parsed, &project));
        assert_eq!(parsed.q0rgs[0].layers[0].placements[0].instance_id, 44);
        let rig = q0s_format::rig::rig_for_q0rg(&parsed, 1).expect("exported rig");
        assert_eq!(
            rig.nodes[0].binding.expect("exported binding").instance_id,
            44
        );
        assert!(rig.channels.iter().any(|channel| {
            channel.property == q0s_format::v2::RigPropertyRef::NodeRotation(bone)
        }));
    }
}
