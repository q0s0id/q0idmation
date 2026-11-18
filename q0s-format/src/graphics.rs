//! Graphics: the plain drawings a timeline draws. Each one is a vector asset
//! only because the format stores artwork that way; the user never chose to
//! keep it in the library. Editors group graphics by the timeline that uses
//! them and leave the unused ones out of saved files.
//!
//! Nothing here changes how a project is stored: which graphics a q0rg owns
//! is worked out from the project each time, so files from any version read
//! the same.

use std::collections::{HashMap, HashSet};

use crate::v2::{Asset, LibraryItemReference, ProjectV2, Target};

/// Every asset something in `project` points at: placements on any layer of
/// any timeline (masks and guides included), rig variant choices and
/// deformers, particle looks and audio clips.
pub fn referenced_asset_ids(project: &ProjectV2) -> HashSet<u16> {
    let mut used = HashSet::new();
    for placement in project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
    {
        if let Target::Asset(id) = placement.target {
            used.insert(id);
        }
    }
    for asset in &project.assets {
        if let Asset::Rig(rig) = asset {
            for choice in rig.variants.iter().flat_map(|variant| &variant.choices) {
                if let Target::Asset(id) = choice.target {
                    used.insert(id);
                }
            }
            used.extend(rig.deformers.iter().map(|deformer| deformer.asset_id()));
        }
    }
    for emitter in &project.runtime.particles.emitters {
        if let crate::particles::ParticleLook::Item(Target::Asset(id)) = emitter.look {
            used.insert(id);
        }
    }
    used.extend(project.audio_clips.iter().map(|clip| clip.asset_id));
    used
}

/// The q0rg each used vector belongs to. A vector drawn inside a symbol
/// belongs to that symbol even when a scene also places it; otherwise the
/// first timeline in project order that places it wins. Vectors that only a
/// rig or a particle emitter refers to belong to that rig's or emitter's
/// timeline.
pub fn vector_owners(project: &ProjectV2) -> HashMap<u16, u16> {
    let vectors: HashSet<u16> = project
        .assets
        .iter()
        .filter_map(|asset| match asset {
            Asset::Vector(vector) => Some(vector.asset_id),
            _ => None,
        })
        .collect();
    let mut owners = HashMap::new();
    let claim_from = |owners: &mut HashMap<u16, u16>, symbols: bool| {
        for q0rg in &project.q0rgs {
            if project.is_scene_root(q0rg.q0rg_id) == symbols {
                continue;
            }
            for placement in q0rg.layers.iter().flat_map(|layer| &layer.placements) {
                if let Target::Asset(id) = placement.target {
                    if vectors.contains(&id) {
                        owners.entry(id).or_insert(q0rg.q0rg_id);
                    }
                }
            }
        }
    };
    claim_from(&mut owners, true);
    claim_from(&mut owners, false);
    for asset in &project.assets {
        if let Asset::Rig(rig) = asset {
            for choice in rig.variants.iter().flat_map(|variant| &variant.choices) {
                if let Target::Asset(id) = choice.target {
                    if vectors.contains(&id) {
                        owners.entry(id).or_insert(rig.owner_q0rg_id);
                    }
                }
            }
        }
    }
    for emitter in &project.runtime.particles.emitters {
        if let crate::particles::ParticleLook::Item(Target::Asset(id)) = emitter.look {
            if vectors.contains(&id) {
                owners.entry(id).or_insert(emitter.q0rg_id);
            }
        }
    }
    owners
}

/// Whether vector `asset_id` is a graphic: a drawing the user has not filed
/// in a library folder and whose name `plain_name` accepts (`None` when it
/// has no name). Named drawings are library items the user chose to keep.
pub fn is_graphic(
    project: &ProjectV2,
    asset_id: u16,
    plain_name: impl Fn(Option<&str>) -> bool,
) -> bool {
    !project
        .runtime
        .library_item_folders
        .contains_key(&LibraryItemReference::Asset(asset_id))
        && plain_name(project.asset_names.get(&asset_id).map(String::as_str))
}

/// Graphics that nothing in the project uses any more, in asset order.
pub fn unused_graphics(project: &ProjectV2, plain_name: impl Fn(Option<&str>) -> bool) -> Vec<u16> {
    let used = referenced_asset_ids(project);
    project
        .assets
        .iter()
        .filter_map(|asset| match asset {
            Asset::Vector(vector) => Some(vector.asset_id),
            _ => None,
        })
        .filter(|id| !used.contains(id) && is_graphic(project, *id, &plain_name))
        .collect()
}

/// Removes the unused graphics with their names, looks and folder entries,
/// and returns their ids.
pub fn drop_unused_graphics(
    project: &mut ProjectV2,
    plain_name: impl Fn(Option<&str>) -> bool,
) -> Vec<u16> {
    let doomed = unused_graphics(project, plain_name);
    if doomed.is_empty() {
        return doomed;
    }
    let set: HashSet<u16> = doomed.iter().copied().collect();
    project.assets.retain(|asset| !set.contains(&asset.id()));
    project.asset_names.retain(|id, _| !set.contains(id));
    project.asset_appearances.retain(|id, _| !set.contains(id));
    project
        .runtime
        .library_item_folders
        .retain(|item, _| !matches!(item, LibraryItemReference::Asset(id) if set.contains(id)));
    doomed
}
