//! Graphics in the editor: the library's graphics folders and the lean save.
//!
//! A graphic is a drawing the user never named or filed: shapes, strokes,
//! fills and text objects. The library shows each one in the graphics folder
//! of the timeline that uses it, hides the ones nothing uses, and saving
//! leaves those out of the file. They stay in the open project, so undo, the
//! clipboard and online rooms can still bring them back.

use std::collections::{BTreeSet, HashMap};

use q0s_format::v2::{Asset, ProjectV2};

/// Whether a drawing with this name is still a plain graphic: no name, or a
/// text object's source. A drawing the user named is a library item they
/// chose to keep.
pub(crate) fn plain_name(name: Option<&str>) -> bool {
    name.is_none_or(|name| crate::text_tool::decode_source(name).is_some())
}

/// Whether asset `asset_id` is a plain drawing: a vector that is a graphic
/// whenever it is not filed in a library folder.
pub(crate) fn is_plain_drawing(project: &ProjectV2, asset_id: u16) -> bool {
    project.assets.iter().any(|asset| match asset {
        Asset::Vector(vector) => vector.asset_id == asset_id,
        // A raster layer's frames are its drawings, like raw vectors.
        Asset::Raster(raster) => raster.asset_id == asset_id,
        _ => false,
    }) && plain_name(project.asset_names.get(&asset_id).map(String::as_str))
}

/// Graphics nothing in `project` uses: the ones a save leaves out.
pub(crate) fn unused(project: &ProjectV2) -> Vec<u16> {
    q0s_format::graphics::unused_graphics(project, plain_name)
}

/// `project` as it is written to disk: without the graphics nothing uses.
/// None when there are none, so the caller writes `project` itself.
pub(crate) fn lean_for_save(project: &ProjectV2) -> Option<ProjectV2> {
    if unused(project).is_empty() {
        return None;
    }
    let mut lean = project.clone();
    q0s_format::graphics::drop_unused_graphics(&mut lean, plain_name);
    Some(lean)
}

/// The graphics folders of the timelines in `q0rg_ids`, in the order the
/// library lists them: scenes first, in scene order, then symbols by name.
/// Each is named after its scene, or its symbol.
pub(crate) fn folders<'a>(
    project: &ProjectV2,
    q0rg_ids: impl IntoIterator<Item = &'a u16>,
) -> Vec<(u16, String)> {
    let scenes = project.scenes();
    let names: HashMap<u16, &str> = project
        .q0rgs
        .iter()
        .map(|q0rg| (q0rg.q0rg_id, q0rg.name.as_str()))
        .collect();
    let ids: BTreeSet<u16> = q0rg_ids.into_iter().copied().collect();
    let mut folders: Vec<(Option<usize>, String, u16)> = ids
        .into_iter()
        .map(|id| {
            let scene = scenes.iter().position(|scene| scene.q0rg_id == id);
            let name = match scene {
                Some(index) => scenes[index].name.clone(),
                None => names
                    .get(&id)
                    .filter(|name| !name.trim().is_empty())
                    .map_or_else(|| format!("q0rg {id}"), |name| name.to_string()),
            };
            (scene, name, id)
        })
        .collect();
    folders.sort_by_cached_key(|(scene, name, id)| {
        (scene.is_none(), *scene, name.to_lowercase(), *id)
    });
    folders
        .into_iter()
        .map(|(_, name, id)| (id, name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Anchor, LibraryItemReference, Path, Placement, Q0rg, Rgba, Target, Transform2D, Tween,
        Vec2, VectorAsset,
    };

    fn square(asset_id: u16) -> Asset {
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![Path {
                anchors: vec![
                    corner(0.0, 0.0),
                    corner(10.0, 0.0),
                    corner(10.0, 10.0),
                    corner(0.0, 10.0),
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 200,
                g: 40,
                b: 40,
                a: 255,
            }),
            stroke: None,
        })
    }

    fn place(project: &mut ProjectV2, q0rg_index: usize, target: Target) {
        project.q0rgs[q0rg_index].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target,
            transform: Transform2D::IDENTITY,
            pivot: None,
            tween: Tween::None,
            fx: Default::default(),
        });
    }

    /// The Stage with one symbol on it; vector 1 on the Stage, vector 2 in
    /// the symbol, vector 3 drawn and then deleted.
    fn project() -> ProjectV2 {
        let mut project = crate::state::default_project();
        let mut symbol: Q0rg = project.q0rgs[0].clone();
        symbol.q0rg_id = 2;
        symbol.name = "Hero".into();
        for layer in &mut symbol.layers {
            layer.placements.clear();
        }
        project.q0rgs.push(symbol);
        project.assets.extend([square(1), square(2), square(3)]);
        place(&mut project, 0, Target::Asset(1));
        place(&mut project, 0, Target::Q0rg(2));
        place(&mut project, 1, Target::Asset(2));
        q0s_format::v2::validate(&project).expect("test project is valid");
        project
    }

    #[test]
    fn named_and_filed_drawings_are_kept() {
        let mut project = project();
        project.asset_names.insert(1, "Logo".into());
        project
            .asset_names
            .insert(2, crate::text_tool::encode_source("hi", 24.0));
        project.runtime.library_folders.push("Props".into());
        project
            .runtime
            .library_item_folders
            .insert(LibraryItemReference::Asset(3), "Props".into());
        assert!(!is_plain_drawing(&project, 1), "a named drawing is kept");
        assert!(is_plain_drawing(&project, 2), "text objects are graphics");
        assert!(is_plain_drawing(&project, 3), "unnamed, so plain");
        assert!(!is_plain_drawing(&project, 99), "missing assets are not");
        // Unplace everything: only drawing 2 is unnamed, unfiled and unused.
        for q0rg in &mut project.q0rgs {
            for layer in &mut q0rg.layers {
                layer
                    .placements
                    .retain(|placement| !matches!(placement.target, Target::Asset(_)));
            }
        }
        assert_eq!(unused(&project), vec![2]);
    }

    #[test]
    fn the_lean_save_drops_only_unused_graphics() {
        let project = project();
        assert_eq!(unused(&project), vec![3]);
        let lean = lean_for_save(&project).expect("one graphic is unused");
        let ids: Vec<u16> = lean.assets.iter().map(Asset::id).collect();
        assert_eq!(ids, vec![1, 2]);
        q0s_format::v2::validate(&lean).expect("the lean project is valid");
        // The open project keeps everything, so undo can still use it.
        assert_eq!(project.assets.len(), 3);

        let mut nothing_unused = project.clone();
        place(&mut nothing_unused, 0, Target::Asset(3));
        assert!(lean_for_save(&nothing_unused).is_none());
    }

    #[test]
    fn saving_leaves_unused_graphics_out_and_undo_brings_them_back() {
        let dir = std::env::temp_dir().join(format!(
            "q0editor-graphics-save-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("make test directory");
        let path = dir.join("lean.q1s");

        let mut open = project();
        place(&mut open, 0, Target::Asset(3));
        let mut history = crate::state::History::new();
        history.snapshot(&open);
        // Delete the last use of drawing 3.
        open.q0rgs[0].layers[0]
            .placements
            .retain(|placement| placement.target != Target::Asset(3));

        crate::file_io::save_project(&path, &open).expect("save");
        let saved = crate::file_io::load_project(&path).expect("load");
        assert!(saved.assets.iter().all(|asset| asset.id() != 3));
        assert!(saved.assets.iter().any(|asset| asset.id() == 2));
        assert!(open.assets.iter().any(|asset| asset.id() == 3));

        let open = history.pop_undo(open, |_, _| {}).expect("undo");
        assert!(open.q0rgs[0].layers[0]
            .placements
            .iter()
            .any(|placement| placement.target == Target::Asset(3)));
        crate::file_io::save_project(&path, &open).expect("save again");
        let saved = crate::file_io::load_project(&path).expect("load again");
        assert!(saved.assets.iter().any(|asset| asset.id() == 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn graphics_folders_list_scenes_first_then_symbols_by_name() {
        let mut project = project();
        let mut walk = project.q0rgs[1].clone();
        walk.q0rg_id = 3;
        walk.name = "arm".into();
        project.q0rgs.push(walk);
        let folders = folders(&project, &[3, 2, 1, 2]);
        assert_eq!(
            folders,
            vec![
                (1, project.scenes()[0].name.clone()),
                (3, "arm".to_string()),
                (2, "Hero".to_string()),
            ]
        );
    }

    #[test]
    fn drawings_belong_to_the_symbol_that_uses_them() {
        let mut project = project();
        // Drawing 2 is also placed on the Stage; the symbol still owns it.
        place(&mut project, 0, Target::Asset(2));
        let owners = q0s_format::graphics::vector_owners(&project);
        assert_eq!(owners.get(&1), Some(&1));
        assert_eq!(owners.get(&2), Some(&2));
        assert_eq!(owners.get(&3), None);
    }
}
