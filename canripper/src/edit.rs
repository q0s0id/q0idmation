use std::path::Path;

use q0s_format::{
    is_q0s_v2, parse_q0s_v2,
    v2::{self, Asset, ProjectDependencyKind, ProjectDependencySource, ProjectV2},
};

use crate::document::{Document, DocumentData, EditRef, PreviewRef};

pub const MAX_CODE_BYTES: usize = 8 * 1024 * 1024;

pub fn code_source(document: &Document, reference: &EditRef) -> Result<String, String> {
    let DocumentData::QProject(root) = &document.data else {
        return Err("code editing is only available for current q0s/q1s projects".to_string());
    };
    match reference {
        EditRef::QProjectQ0rgScript { path, q0rg_id } => {
            with_project_at_path(root, path, |project| {
                project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == *q0rg_id)
                    .map(|q0rg| q0rg.script.clone())
                    .ok_or_else(|| format!("q0rg {q0rg_id} is missing"))
            })
        }
        EditRef::QProjectFrameScript {
            path,
            q0rg_id,
            layer_id,
            frame,
        } => with_project_at_path(root, path, |project| {
            project
                .runtime
                .frame_scripts
                .iter()
                .find(|script| {
                    script.q0rg_id == *q0rg_id
                        && script.layer_id == *layer_id
                        && script.frame == *frame
                })
                .map(|script| script.source.clone())
                .ok_or_else(|| format!("frame script q{q0rg_id}/l{layer_id}/f{frame} is missing"))
        }),
        EditRef::QProjectQ0lang { path, node_id } => with_project_at_path(root, path, |project| {
            let node = project
                .runtime
                .project_graph
                .nodes
                .iter()
                .find(|node| node.node_id == *node_id)
                .ok_or_else(|| format!("q0lang node {node_id} is missing"))?;
            if node.kind != ProjectDependencyKind::Q0lang {
                return Err(format!("project node {node_id} is not q0lang"));
            }
            let ProjectDependencySource::Embedded(bytes) = &node.source else {
                return Err("external q0lang cannot be edited in-place".to_string());
            };
            String::from_utf8(bytes.clone())
                .map_err(|error| format!("embedded q0lang is not utf-8: {error}"))
        }),
    }
}

pub fn set_code_source(
    document: &mut Document,
    reference: &EditRef,
    source: &str,
) -> Result<(), String> {
    if source.len() > MAX_CODE_BYTES {
        return Err(format!(
            "code is too large ({} bytes; limit is {MAX_CODE_BYTES})",
            source.len()
        ));
    }
    let DocumentData::QProject(root) = &mut document.data else {
        return Err("code editing is only available for current q0s/q1s projects".to_string());
    };

    mutate_project_at_path(root, reference.path(), &mut |project| match reference {
        EditRef::QProjectQ0rgScript { q0rg_id, .. } => {
            if source.len() > usize::from(u16::MAX) {
                return Err("q0rg script exceeds the q0s/q1s u16 source limit".to_string());
            }
            let q0rg = project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == *q0rg_id)
                .ok_or_else(|| format!("q0rg {q0rg_id} is missing"))?;
            q0rg.script = source.to_string();
            Ok(())
        }
        EditRef::QProjectFrameScript {
            q0rg_id,
            layer_id,
            frame,
            ..
        } => {
            if source.len() > usize::from(u16::MAX) {
                return Err("frame script exceeds the q0s/q1s u16 source limit".to_string());
            }
            let script = project
                .runtime
                .frame_scripts
                .iter_mut()
                .find(|script| {
                    script.q0rg_id == *q0rg_id
                        && script.layer_id == *layer_id
                        && script.frame == *frame
                })
                .ok_or_else(|| {
                    format!("frame script q{q0rg_id}/l{layer_id}/f{frame} is missing")
                })?;
            script.source = source.to_string();
            Ok(())
        }
        EditRef::QProjectQ0lang { node_id, .. } => {
            let node = project
                .runtime
                .project_graph
                .nodes
                .iter_mut()
                .find(|node| node.node_id == *node_id)
                .ok_or_else(|| format!("q0lang node {node_id} is missing"))?;
            if node.kind != ProjectDependencyKind::Q0lang {
                return Err(format!("project node {node_id} is not q0lang"));
            }
            let ProjectDependencySource::Embedded(bytes) = &mut node.source else {
                return Err("external q0lang cannot be edited in-place".to_string());
            };
            *bytes = source.as_bytes().to_vec();
            Ok(())
        }
    })
}

impl EditRef {
    fn path(&self) -> &[u16] {
        match self {
            Self::QProjectQ0rgScript { path, .. }
            | Self::QProjectFrameScript { path, .. }
            | Self::QProjectQ0lang { path, .. } => path,
        }
    }
}

pub fn can_replace_with_image(document: &Document, reference: &PreviewRef) -> bool {
    match (reference, &document.data) {
        (PreviewRef::QProjectAsset { path, asset_id, .. }, DocumentData::QProject(root)) => {
            with_project_at_path(root, path, |project| {
                Ok(project.assets.iter().any(|asset| {
                    asset.id() == *asset_id && matches!(asset, Asset::Bitmap(_) | Asset::Vector(_))
                }))
            })
            .unwrap_or(false)
        }
        (PreviewRef::Q1LegacyBitmap(asset_id), DocumentData::Q1Legacy(project)) => project
            .assets
            .iter()
            .any(|asset| asset.asset_id == *asset_id),
        (PreviewRef::Q0LegacyBitmap(asset_id), DocumentData::Q0Legacy(movie)) => {
            movie.bitmaps.contains_key(asset_id)
        }
        _ => false,
    }
}

pub fn replace_with_image_path(
    document: &mut Document,
    reference: &PreviewRef,
    path: &Path,
) -> Result<(), String> {
    let image = image::open(path)
        .map_err(|error| format!("decode {}: {error}", path.display()))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let width =
        u16::try_from(width).map_err(|_| "replacement image width exceeds u16".to_string())?;
    let height =
        u16::try_from(height).map_err(|_| "replacement image height exceeds u16".to_string())?;
    replace_with_rgba(document, reference, width, height, image.into_raw())
}

fn replace_with_rgba(
    document: &mut Document,
    reference: &PreviewRef,
    width: u16,
    height: u16,
    rgba: Vec<u8>,
) -> Result<(), String> {
    let expected = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "replacement image dimensions overflow".to_string())?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err("replacement image has invalid rgba dimensions".to_string());
    }

    match (reference, &mut document.data) {
        (PreviewRef::QProjectAsset { path, asset_id, .. }, DocumentData::QProject(root)) => {
            mutate_project_at_path(root, path, &mut |project| {
                let index = project
                    .assets
                    .iter()
                    .position(|asset| asset.id() == *asset_id)
                    .ok_or_else(|| format!("asset {asset_id} is missing"))?;
                if !matches!(project.assets[index], Asset::Bitmap(_) | Asset::Vector(_)) {
                    return Err(
                        "only vector graphics and bitmaps can be replaced by an image".to_string(),
                    );
                }
                let original_asset = project.assets[index].clone();
                let original_appearance = project.asset_appearances.get(asset_id).cloned();
                project.assets[index] = Asset::Bitmap(v2::BitmapAsset {
                    asset_id: *asset_id,
                    width,
                    height,
                    rgba: rgba.clone(),
                });
                project.asset_appearances.remove(asset_id);
                if let Err(error) = q0s_format::v2::validate(project) {
                    project.assets[index] = original_asset;
                    if let Some(appearance) = original_appearance {
                        project.asset_appearances.insert(*asset_id, appearance);
                    }
                    return Err(format!("replacement would invalidate project: {error}"));
                }
                Ok(())
            })?;
        }
        (PreviewRef::Q1LegacyBitmap(asset_id), DocumentData::Q1Legacy(project)) => {
            let asset = project
                .assets
                .iter_mut()
                .find(|asset| asset.asset_id == *asset_id)
                .ok_or_else(|| format!("legacy q1s bitmap {asset_id} is missing"))?;
            let old = asset.clone();
            asset.width = width;
            asset.height = height;
            asset.rgba = rgba;
            if let Err(error) = q0s_format::validate_q1s(project) {
                *project
                    .assets
                    .iter_mut()
                    .find(|asset| asset.asset_id == *asset_id)
                    .expect("asset existed before validation") = old;
                return Err(format!("replacement would invalidate q1s: {error}"));
            }
        }
        (PreviewRef::Q0LegacyBitmap(asset_id), DocumentData::Q0Legacy(movie)) => {
            let bitmap = movie
                .bitmaps
                .get_mut(asset_id)
                .ok_or_else(|| format!("legacy q0s bitmap {asset_id} is missing"))?;
            let old = bitmap.clone();
            bitmap.width = width;
            bitmap.height = height;
            bitmap.rgba = rgba;
            let verification = q0s_format::write_q0s(movie)
                .map_err(|error| format!("replacement would invalidate q0s: {error}"))
                .and_then(|bytes| {
                    q0s_format::parse_q0s(&bytes)
                        .map(|_| ())
                        .map_err(|error| format!("replacement q0s parse-back failed: {error}"))
                });
            if let Err(error) = verification {
                movie.bitmaps.insert(*asset_id, old);
                return Err(error);
            }
        }
        _ => return Err("selected node is not replaceable graphics/bitmap content".to_string()),
    }
    document.rebuild_tree();
    Ok(())
}

fn with_project_at_path<T>(
    project: &ProjectV2,
    path: &[u16],
    f: impl FnOnce(&ProjectV2) -> Result<T, String>,
) -> Result<T, String> {
    if path.is_empty() {
        return f(project);
    }
    let node = project
        .runtime
        .project_graph
        .nodes
        .iter()
        .find(|node| node.node_id == path[0])
        .ok_or_else(|| format!("embedded path references missing node {}", path[0]))?;
    if node.kind != ProjectDependencyKind::Movie {
        return Err(format!("embedded path node {} is not a movie", path[0]));
    }
    let ProjectDependencySource::Embedded(bytes) = &node.source else {
        return Err(format!("embedded path node {} is external", path[0]));
    };
    let nested = parse_nested_project(bytes)?;
    with_project_at_path(&nested, &path[1..], f)
}

fn mutate_project_at_path(
    project: &mut ProjectV2,
    path: &[u16],
    f: &mut dyn FnMut(&mut ProjectV2) -> Result<(), String>,
) -> Result<(), String> {
    if path.is_empty() {
        return f(project);
    }
    let node = project
        .runtime
        .project_graph
        .nodes
        .iter_mut()
        .find(|node| node.node_id == path[0])
        .ok_or_else(|| format!("embedded path references missing node {}", path[0]))?;
    if node.kind != ProjectDependencyKind::Movie {
        return Err(format!("embedded path node {} is not a movie", path[0]));
    }
    let ProjectDependencySource::Embedded(bytes) = &mut node.source else {
        return Err(format!("embedded path node {} is external", path[0]));
    };
    let mut nested = parse_nested_project(bytes)?;
    mutate_project_at_path(&mut nested, &path[1..], f)?;
    *bytes = serialize_nested_like(bytes, &nested)?;
    Ok(())
}

fn parse_nested_project(bytes: &[u8]) -> Result<ProjectV2, String> {
    if bytes.starts_with(b"Q0S\0") {
        if !is_q0s_v2(bytes) {
            return Err("legacy embedded q0s is not editable as a v2 project".to_string());
        }
        return parse_q0s_v2(bytes).map_err(|error| format!("parse embedded q0s: {error}"));
    }
    if bytes.starts_with(b"Q1S\0") {
        if bytes.get(4..6) == Some(&1u16.to_le_bytes()) {
            return Err("legacy embedded q1s is not editable as a v2 project".to_string());
        }
        return v2::parse(bytes).map_err(|error| format!("parse embedded q1s: {error}"));
    }
    Err("embedded movie is not q0s/q1s".to_string())
}

fn serialize_nested_like(original: &[u8], project: &ProjectV2) -> Result<Vec<u8>, String> {
    if original.starts_with(b"Q0S\0") {
        let bytes = q0s_format::write_q0s_v2(project)
            .map_err(|error| format!("serialize embedded q0s: {error}"))?;
        let parsed = q0s_format::parse_q0s_v2(&bytes)
            .map_err(|error| format!("embedded q0s parse-back: {error}"))?;
        if !v2::wire_equivalent(&parsed, project) {
            return Err("embedded q0s parse-back changed the canonical project".to_string());
        }
        return Ok(bytes);
    }
    if original.starts_with(b"Q1S\0") {
        let bytes =
            v2::write(project).map_err(|error| format!("serialize embedded q1s: {error}"))?;
        let parsed =
            v2::parse(&bytes).map_err(|error| format!("embedded q1s parse-back: {error}"))?;
        if !v2::wire_equivalent(&parsed, project) {
            return Err("embedded q1s parse-back changed the canonical project".to_string());
        }
        return Ok(bytes);
    }
    Err("embedded movie format changed while editing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn project(name: &str) -> ProjectV2 {
        ProjectV2 {
            meta: v2::ProjectMeta {
                name: name.into(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(v2::VectorAsset {
                asset_id: 7,
                paths: Vec::new(),
                fill: Some(v2::Rgba {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: HashMap::from([(7, "hero".to_string())]),
            asset_appearances: HashMap::new(),
            layer_metadata: HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![v2::Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: "old q0rg".into(),
                layers: vec![v2::Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![v2::Placement {
                        instance_id: 0,
                        frame: 0,
                        target: v2::Target::Asset(7),
                        transform: v2::Transform2D::IDENTITY,
                        tween: v2::Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        }
    }

    #[test]
    fn code_edit_repackages_nested_q0s_recursively() {
        let mut inner = project("inner");
        inner
            .runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 9,
                parent_node_id: None,
                alias: "logic".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::Embedded(b"old code".to_vec()),
            });
        let inner_bytes = q0s_format::write_q0s_v2(&inner).unwrap();
        let mut outer = project("outer");
        outer
            .runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 2,
                parent_node_id: None,
                alias: "inner".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::Embedded(inner_bytes),
            });
        let bytes = q0s_format::write_q0s_v2(&outer).unwrap();
        let mut document = Document::from_bytes("outer.q0s".into(), bytes).unwrap();
        let edit = EditRef::QProjectQ0lang {
            path: vec![2],
            node_id: 9,
        };
        set_code_source(&mut document, &edit, "new code\n").unwrap();
        assert_eq!(code_source(&document, &edit).unwrap(), "new code\n");
        let DocumentData::QProject(root) = &document.data else {
            panic!()
        };
        let node = root
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == 2)
            .unwrap();
        let ProjectDependencySource::Embedded(bytes) = &node.source else {
            panic!()
        };
        let reparsed = q0s_format::parse_q0s_v2(bytes).expect("repacked inner q0s");
        let logic = reparsed
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == 9)
            .unwrap();
        assert!(
            matches!(&logic.source, ProjectDependencySource::Embedded(bytes) if bytes == b"new code\n")
        );
    }

    #[test]
    fn replacing_vector_with_bitmap_preserves_asset_identity_and_placements() {
        let project = project("replace");
        let bytes = q0s_format::write_q0s_v2(&project).unwrap();
        let mut document = Document::from_bytes("replace.q0s".into(), bytes).unwrap();
        let target = PreviewRef::QProjectAsset {
            path: Vec::new(),
            asset_id: 7,
            kind: crate::document::AssetVisualKind::Vector,
        };
        replace_with_rgba(&mut document, &target, 2, 3, vec![255; 2 * 3 * 4]).unwrap();
        let DocumentData::QProject(project) = &document.data else {
            panic!()
        };
        assert!(
            matches!(&project.assets[0], Asset::Bitmap(bitmap) if bitmap.asset_id == 7 && bitmap.width == 2 && bitmap.height == 3)
        );
        assert_eq!(
            project.asset_names.get(&7).map(String::as_str),
            Some("hero")
        );
        assert_eq!(
            project.q0rgs[0].layers[0].placements[0].target,
            v2::Target::Asset(7)
        );
    }
}
