//! Tools for q0s work beyond packing: what a movie holds, whether its
//! scripts read, and what a built app carries.

use std::path::Path;

use q0s_format::v2::{Asset, ProjectV2};

use crate::runtimes::Target;

pub struct ProjectReport {
    pub name: String,
    pub file_bytes: u64,
    pub stage: (u16, u16),
    pub fps: u16,
    /// The stage colour.
    pub background: [u8; 3],
    pub scenes: Vec<String>,
    pub symbols: usize,
    pub layers: usize,
    pub longest_timeline: u16,
    pub placements: usize,
    pub scripts: usize,
    pub script_lines: usize,
    pub sounds: usize,
    /// Kind, count, bytes.
    pub by_kind: Vec<(&'static str, usize, u64)>,
    /// Name, kind, bytes: the heaviest assets.
    pub largest: Vec<(String, &'static str, u64)>,
}

fn asset_weight(asset: &Asset) -> (&'static str, u64) {
    match asset {
        Asset::Bitmap(bitmap) => ("bitmap", bitmap.rgba.len() as u64),
        Asset::Vector(vector) => (
            "vector",
            vector
                .paths
                .iter()
                .map(|path| path.anchors.len() as u64 * 24)
                .sum::<u64>()
                + 32,
        ),
        Asset::Q0v(video) => ("video", video.bytes.len() as u64),
        Asset::Rig(_) => ("rig", 256),
        Asset::Model(_) => ("3D model", 4096),
        Asset::Raster(raster) => (
            "raster",
            (raster.tiles.len() * q0s_format::pixels::TILE_BYTES) as u64,
        ),
    }
}

pub fn load_project(path: &Path) -> Result<(ProjectV2, u64), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let project = q0s_format::v2::parse(&bytes).map_err(|error| {
        if bytes.starts_with(b"Q0S\0") || bytes.starts_with(b"Q1S\0") {
            "an older q0s format: it plays and packs as it is; open and save it in q0editor to see its details here and to re-pack it".to_string()
        } else {
            format!("{} is not a q0s movie ({error})", path.display())
        }
    })?;
    Ok((project, bytes.len() as u64))
}

pub fn project_report(project: &ProjectV2, file_bytes: u64) -> ProjectReport {
    let mut by_kind: Vec<(&'static str, usize, u64)> = Vec::new();
    let mut largest: Vec<(String, &'static str, u64)> = Vec::new();
    for asset in &project.assets {
        let (kind, bytes) = asset_weight(asset);
        match by_kind
            .iter_mut()
            .find(|(existing, _, _)| *existing == kind)
        {
            Some(entry) => {
                entry.1 += 1;
                entry.2 += bytes;
            }
            None => by_kind.push((kind, 1, bytes)),
        }
        let name = project
            .asset_names
            .get(&asset.id())
            .cloned()
            .unwrap_or_else(|| format!("{kind} {}", asset.id()));
        largest.push((name, kind, bytes));
    }
    by_kind.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    largest.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    largest.truncate(12);
    let mut scripts = 0;
    let mut script_lines = 0;
    for source in script_sources(project) {
        scripts += 1;
        script_lines += source.1.lines().count();
    }
    ProjectReport {
        name: project.meta.name.clone(),
        file_bytes,
        stage: (project.meta.stage_width, project.meta.stage_height),
        fps: project.meta.fps,
        background: {
            let color = project.meta.background_color;
            [color.r, color.g, color.b]
        },
        scenes: project
            .scenes()
            .into_iter()
            .map(|scene| scene.name)
            .collect(),
        symbols: project.q0rgs.len(),
        layers: project.q0rgs.iter().map(|q0rg| q0rg.layers.len()).sum(),
        longest_timeline: project
            .q0rgs
            .iter()
            .map(|q0rg| q0rg.frame_count)
            .max()
            .unwrap_or(0),
        placements: project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .map(|layer| layer.placements.len())
            .sum(),
        scripts,
        script_lines,
        sounds: project.audio_clips.len(),
        by_kind,
        largest,
    }
}

/// Every script in the movie with where it lives.
pub fn script_sources(project: &ProjectV2) -> Vec<(String, String)> {
    let name_of = |id: u16| {
        project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == id)
            .map_or_else(|| format!("symbol {id}"), |q0rg| q0rg.name.clone())
    };
    let mut sources: Vec<(String, String)> = project
        .q0rgs
        .iter()
        .filter(|q0rg| !q0rg.script.trim().is_empty())
        .map(|q0rg| {
            (
                format!("{} (symbol script)", q0rg.name),
                q0rg.script.clone(),
            )
        })
        .collect();
    sources.extend(
        project
            .runtime
            .frame_scripts
            .iter()
            .filter(|script| !script.source.trim().is_empty())
            .map(|script| {
                (
                    format!(
                        "{}, layer {}, frame {}",
                        name_of(script.q0rg_id),
                        script.layer_id,
                        script.frame + 1
                    ),
                    script.source.clone(),
                )
            }),
    );
    sources
}

pub struct ScriptIssue {
    pub place: String,
    pub line: usize,
    pub message: String,
    pub text: String,
}

/// What the q0lang parser says about every script.
pub fn check_scripts(project: &ProjectV2) -> (usize, Vec<ScriptIssue>) {
    let sources = script_sources(project);
    let mut issues = Vec::new();
    for (place, source) in &sources {
        let program = q0s_format::q0lang::parser::parse(source);
        for diagnostic in program.diagnostics {
            issues.push(ScriptIssue {
                place: place.clone(),
                line: diagnostic.line,
                message: diagnostic.message,
                text: source
                    .lines()
                    .nth(diagnostic.line.saturating_sub(1))
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            });
        }
    }
    (sources.len(), issues)
}

/// A built app, read back.
pub struct BuiltApp {
    pub kind: &'static str,
    pub bundle: q0qir::Bundle,
    pub file_bytes: u64,
}

pub fn read_built_app(path: &Path) -> Result<BuiltApp, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file_bytes = bytes.len() as u64;
    let (kind, target) = if bytes.starts_with(b"PK") {
        ("Android APK", Some(Target::AndroidApk))
    } else if bytes.starts_with(b"\x7fELF") && bytes.get(8..11) == Some(b"AI\x02".as_slice()) {
        ("Linux AppImage", Some(Target::LinuxAppImage))
    } else if bytes.starts_with(b"MZ") {
        ("Windows program", None)
    } else {
        ("QIR bundle", None)
    };
    let bundle = match target {
        Some(target) => crate::builder::verify(target, path)?,
        None => q0qir::Bundle::decode(&bytes, true)
            .map_err(|error| format!("{}: {error}", path.display()))?,
    };
    Ok(BuiltApp {
        kind,
        bundle,
        file_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_problems_name_their_place_and_line() {
        let mut project = q0s_format::v2::ProjectV2 {
            meta: q0s_format::v2::ProjectMeta {
                name: "t".into(),
                fps: 30,
                stage_width: 100,
                stage_height: 100,
                background_color: q0s_format::v2::Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![q0s_format::v2::Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: "x = 1\nthis is not q0lang at all ((\n".into(),
                layers: Vec::new(),
            }],
        };
        let (count, issues) = check_scripts(&project);
        assert_eq!(count, 1);
        assert!(!issues.is_empty());
        assert_eq!(issues[0].line, 2);
        assert!(issues[0].place.contains("Stage"));
        project.q0rgs[0].script = "x = 1\n".into();
        assert!(check_scripts(&project).1.is_empty());
    }
}
