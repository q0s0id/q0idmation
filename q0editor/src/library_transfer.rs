use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use q0s_format::v2::{Asset, FrameLabelKey, InstanceKey, LayerKey, ProjectV2, RigDeformer, Target};

use crate::state::LibraryItem;

#[derive(Debug, Clone)]
struct TransferClosure {
    q0rgs: BTreeSet<u16>,
    assets: BTreeSet<u16>,
}

fn collect_transfer_closure(
    source: &ProjectV2,
    root: LibraryItem,
) -> Result<TransferClosure, String> {
    let mut q0rgs = BTreeSet::new();
    let mut assets = BTreeSet::new();
    let mut q0rg_queue = VecDeque::new();
    let mut asset_queue = VecDeque::new();

    match root {
        LibraryItem::Q0rg(id) => q0rg_queue.push_back(id),
        LibraryItem::Asset(id) => asset_queue.push_back(id),
    }

    loop {
        while let Some(q0rg_id) = q0rg_queue.pop_front() {
            if !q0rgs.insert(q0rg_id) {
                continue;
            }
            let q0rg = source
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .ok_or_else(|| format!("source library q0rg {q0rg_id} is missing"))?;
            for placement in q0rg.layers.iter().flat_map(|layer| &layer.placements) {
                match placement.target {
                    Target::Q0rg(id) => q0rg_queue.push_back(id),
                    Target::Asset(id) => asset_queue.push_back(id),
                }
            }
            for clip in source
                .audio_clips
                .iter()
                .filter(|clip| clip.q0rg_id == q0rg_id)
            {
                asset_queue.push_back(clip.asset_id);
            }
            for asset in &source.assets {
                if let Asset::Rig(rig) = asset {
                    if rig.owner_q0rg_id == q0rg_id {
                        asset_queue.push_back(rig.asset_id);
                    }
                }
            }
        }

        let Some(asset_id) = asset_queue.pop_front() else {
            if q0rg_queue.is_empty() {
                break;
            }
            continue;
        };
        if !assets.insert(asset_id) {
            continue;
        }
        let asset = source
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .ok_or_else(|| format!("source library asset {asset_id} is missing"))?;
        if let Asset::Rig(rig) = asset {
            q0rg_queue.push_back(rig.owner_q0rg_id);
            for deformer in &rig.deformers {
                asset_queue.push_back(deformer.asset_id());
            }
            for variant in &rig.variants {
                for choice in &variant.choices {
                    match choice.target {
                        Target::Q0rg(id) => q0rg_queue.push_back(id),
                        Target::Asset(id) => asset_queue.push_back(id),
                    }
                }
            }
        }
    }

    Ok(TransferClosure { q0rgs, assets })
}

/// Keep just the library dependency closure needed by copied placements.
/// This lets the editor clipboard survive a project-tab switch without holding
/// a second copy of every unrelated bitmap and video in the source project.
pub fn snapshot_library_items(
    source: &ProjectV2,
    roots: &[LibraryItem],
) -> Result<ProjectV2, String> {
    let mut q0rg_ids = BTreeSet::new();
    let mut asset_ids = BTreeSet::new();
    for &root in roots {
        let closure = collect_transfer_closure(source, root)?;
        q0rg_ids.extend(closure.q0rgs);
        asset_ids.extend(closure.assets);
    }

    let mut snapshot = ProjectV2 {
        meta: source.meta.clone(),
        assets: source
            .assets
            .iter()
            .filter(|asset| asset_ids.contains(&asset.id()))
            .cloned()
            .collect(),
        asset_names: source
            .asset_names
            .iter()
            .filter(|(id, _)| asset_ids.contains(id))
            .map(|(id, name)| (*id, name.clone()))
            .collect(),
        asset_appearances: source
            .asset_appearances
            .iter()
            .filter(|(id, _)| asset_ids.contains(id))
            .map(|(id, appearance)| (*id, appearance.clone()))
            .collect(),
        layer_metadata: source
            .layer_metadata
            .iter()
            .filter(|(key, _)| q0rg_ids.contains(&key.q0rg_id))
            .map(|(key, metadata)| (*key, *metadata))
            .collect(),
        audio_clips: source
            .audio_clips
            .iter()
            .filter(|clip| q0rg_ids.contains(&clip.q0rg_id))
            .copied()
            .collect(),
        runtime: Default::default(),
        q0rgs: source
            .q0rgs
            .iter()
            .filter(|q0rg| q0rg_ids.contains(&q0rg.q0rg_id))
            .cloned()
            .collect(),
    };
    snapshot.runtime.frame_scripts = source
        .runtime
        .frame_scripts
        .iter()
        .filter(|script| q0rg_ids.contains(&script.q0rg_id))
        .cloned()
        .collect();
    snapshot.runtime.instance_names = source
        .runtime
        .instance_names
        .iter()
        .filter(|(key, _)| q0rg_ids.contains(&key.q0rg_id))
        .map(|(key, name)| (*key, name.clone()))
        .collect();
    snapshot.runtime.frame_labels = source
        .runtime
        .frame_labels
        .iter()
        .filter(|(key, _)| q0rg_ids.contains(&key.q0rg_id))
        .map(|(key, name)| (*key, name.clone()))
        .collect();
    Ok(snapshot)
}

fn allocate_ids(
    used: impl Iterator<Item = u16>,
    source_ids: &BTreeSet<u16>,
) -> Result<HashMap<u16, u16>, String> {
    let mut used: HashSet<u16> = used.collect();
    let mut next = 1_u32;
    let mut map = HashMap::with_capacity(source_ids.len());
    for &source_id in source_ids {
        while next <= u32::from(u16::MAX) && used.contains(&(next as u16)) {
            next += 1;
        }
        if next > u32::from(u16::MAX) {
            return Err("destination id space exhausted while importing library item".to_string());
        }
        let id = next as u16;
        used.insert(id);
        map.insert(source_id, id);
        next += 1;
    }
    Ok(map)
}

fn remap_target(
    target: Target,
    q0rg_ids: &HashMap<u16, u16>,
    asset_ids: &HashMap<u16, u16>,
) -> Result<Target, String> {
    match target {
        Target::Q0rg(id) => q0rg_ids
            .get(&id)
            .copied()
            .map(Target::Q0rg)
            .ok_or_else(|| format!("missing imported q0rg mapping for {id}")),
        Target::Asset(id) => asset_ids
            .get(&id)
            .copied()
            .map(Target::Asset)
            .ok_or_else(|| format!("missing imported asset mapping for {id}")),
    }
}

fn remap_deformer_asset(
    deformer: &mut RigDeformer,
    asset_ids: &HashMap<u16, u16>,
) -> Result<(), String> {
    let old = deformer.asset_id();
    let new = *asset_ids
        .get(&old)
        .ok_or_else(|| format!("missing rig deformer asset mapping for {old}"))?;
    match deformer {
        RigDeformer::Skin { asset_id, .. }
        | RigDeformer::Bend { asset_id, .. }
        | RigDeformer::Cage { asset_id, .. } => *asset_id = new,
    }
    Ok(())
}

/// Clone one library item from another open project into `destination`.
///
/// Q0rg roots pull their full reachable symbol/asset graph. IDs are always
/// destination-local and freshly allocated, so identical numeric ids in two
/// open q1s documents can never alias each other. The returned project is
/// validated before it is exposed to the caller; the input projects are never
/// mutated on failure.
pub fn import_library_item(
    source: &ProjectV2,
    destination: &ProjectV2,
    root: LibraryItem,
) -> Result<(ProjectV2, LibraryItem), String> {
    let closure = collect_transfer_closure(source, root)?;
    let q0rg_ids = allocate_ids(destination.q0rgs.iter().map(|q| q.q0rg_id), &closure.q0rgs)?;
    let asset_ids = allocate_ids(destination.assets.iter().map(Asset::id), &closure.assets)?;

    let mut out = destination.clone();

    for &source_asset_id in &closure.assets {
        let mut asset = source
            .assets
            .iter()
            .find(|asset| asset.id() == source_asset_id)
            .cloned()
            .ok_or_else(|| format!("source library asset {source_asset_id} disappeared"))?;
        let new_asset_id = asset_ids[&source_asset_id];
        match &mut asset {
            Asset::Bitmap(bitmap) => bitmap.asset_id = new_asset_id,
            Asset::Vector(vector) => vector.asset_id = new_asset_id,
            Asset::Q0v(video) => video.asset_id = new_asset_id,
            Asset::Rig(rig) => {
                rig.asset_id = new_asset_id;
                rig.owner_q0rg_id = *q0rg_ids.get(&rig.owner_q0rg_id).ok_or_else(|| {
                    "rig owner q0rg was not included in library transfer".to_string()
                })?;
                for deformer in &mut rig.deformers {
                    remap_deformer_asset(deformer, &asset_ids)?;
                }
                for variant in &mut rig.variants {
                    for choice in &mut variant.choices {
                        choice.target = remap_target(choice.target, &q0rg_ids, &asset_ids)?;
                    }
                }
            }
        }
        out.assets.push(asset);
        if let Some(name) = source.asset_names.get(&source_asset_id) {
            out.asset_names.insert(new_asset_id, name.clone());
        }
        if let Some(appearance) = source.asset_appearances.get(&source_asset_id) {
            out.asset_appearances
                .insert(new_asset_id, appearance.clone());
        }
    }

    for &source_q0rg_id in &closure.q0rgs {
        let mut q0rg = source
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == source_q0rg_id)
            .cloned()
            .ok_or_else(|| format!("source library q0rg {source_q0rg_id} disappeared"))?;
        q0rg.q0rg_id = q0rg_ids[&source_q0rg_id];
        for placement in q0rg
            .layers
            .iter_mut()
            .flat_map(|layer| &mut layer.placements)
        {
            placement.target = remap_target(placement.target, &q0rg_ids, &asset_ids)?;
        }
        out.q0rgs.push(q0rg);
    }

    for (key, metadata) in &source.layer_metadata {
        if let Some(&new_q0rg_id) = q0rg_ids.get(&key.q0rg_id) {
            out.layer_metadata
                .insert(LayerKey::new(new_q0rg_id, key.layer_id), *metadata);
        }
    }
    for clip in &source.audio_clips {
        let Some(&new_q0rg_id) = q0rg_ids.get(&clip.q0rg_id) else {
            continue;
        };
        let Some(&new_asset_id) = asset_ids.get(&clip.asset_id) else {
            return Err(format!(
                "missing imported audio asset mapping for {}",
                clip.asset_id
            ));
        };
        let mut clip = *clip;
        clip.q0rg_id = new_q0rg_id;
        clip.asset_id = new_asset_id;
        out.audio_clips.push(clip);
    }

    for script in &source.runtime.frame_scripts {
        if let Some(&new_q0rg_id) = q0rg_ids.get(&script.q0rg_id) {
            let mut script = script.clone();
            script.q0rg_id = new_q0rg_id;
            out.runtime.frame_scripts.push(script);
        }
    }
    for (key, name) in &source.runtime.instance_names {
        if let Some(&new_q0rg_id) = q0rg_ids.get(&key.q0rg_id) {
            out.runtime
                .instance_names
                .insert(InstanceKey::new(new_q0rg_id, key.instance_id), name.clone());
        }
    }
    for (key, name) in &source.runtime.frame_labels {
        if let Some(&new_q0rg_id) = q0rg_ids.get(&key.q0rg_id) {
            out.runtime
                .frame_labels
                .insert(FrameLabelKey::new(new_q0rg_id, key.frame), name.clone());
        }
    }

    // Imported symbols retain their source instance identities internally. They
    // live under fresh q0rg ids, so their runtime/rig keys remain disjoint from
    // the destination project's existing symbols.
    q0s_format::v2::validate(&out)
        .map_err(|error| format!("imported library item is invalid: {error}"))?;

    let imported_root = match root {
        LibraryItem::Q0rg(id) => LibraryItem::Q0rg(q0rg_ids[&id]),
        LibraryItem::Asset(id) => LibraryItem::Asset(asset_ids[&id]),
    };
    Ok((out, imported_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Anchor, FrameScript, Layer, LayerMetadata, Path as VPath, Placement, Q0rg, Rgba,
        Transform2D, Tween, Vec2, VectorAppearance, VectorAsset, VectorMaterial,
    };

    fn vector(id: u16, x: f32) -> Asset {
        Asset::Vector(VectorAsset {
            asset_id: id,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(x, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(x + 10.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(x + 10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            }),
            stroke: None,
        })
    }

    #[test]
    fn q0rg_transfer_recursively_remaps_nested_symbols_and_assets() {
        let mut source = crate::state::default_project();
        source.assets.push(vector(50, 0.0));
        source.assets.push(vector(51, 20.0));
        source.asset_names.insert(50, "source vector".into());
        source.asset_appearances.insert(
            50,
            VectorAppearance {
                material: VectorMaterial::Solid,
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: q0s_format::transform::Affine::IDENTITY,
                fill_gradient: None,
            },
        );
        source.q0rgs.push(Q0rg {
            q0rg_id: 20,
            name: "child".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 7,
                timeline_frame_count: None,
                name: "child layer".into(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 200,
                    frame: 0,
                    target: Target::Asset(51),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        });
        source.q0rgs.push(Q0rg {
            q0rg_id: 21,
            name: "root symbol".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 8,
                timeline_frame_count: None,
                name: "root layer".into(),
                explicit_keyframes: Vec::new(),
                placements: vec![
                    Placement {
                        instance_id: 300,
                        frame: 0,
                        target: Target::Asset(50),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    },
                    Placement {
                        instance_id: 301,
                        frame: 0,
                        target: Target::Q0rg(20),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    },
                ],
            }],
        });
        source.layer_metadata.insert(
            LayerKey::new(21, 8),
            LayerMetadata {
                locked: true,
                ..Default::default()
            },
        );
        source.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 21,
            layer_id: 8,
            frame: 0,
            source: "stop();".into(),
        });
        source
            .runtime
            .instance_names
            .insert(InstanceKey::new(21, 300), "hand".into());
        source
            .runtime
            .frame_labels
            .insert(FrameLabelKey::new(21, 0), "idle".into());

        let mut destination = crate::state::default_project();
        destination.assets.push(vector(50, 100.0)); // Deliberate numeric collision.
        destination.q0rgs.push(Q0rg {
            q0rg_id: 20,
            name: "existing".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![],
        });
        let original_source = source.clone();

        let (imported, root) =
            import_library_item(&source, &destination, LibraryItem::Q0rg(21)).unwrap();
        let LibraryItem::Q0rg(new_root) = root else {
            panic!("q0rg root")
        };
        assert_ne!(new_root, 21);
        assert_eq!(
            source, original_source,
            "cross-project drag must never mutate its source library"
        );
        let root = imported
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == new_root)
            .unwrap();
        let nested = match root.layers[0].placements[1].target {
            Target::Q0rg(id) => id,
            _ => panic!("nested q0rg"),
        };
        assert_ne!(nested, 20);
        let root_asset = match root.layers[0].placements[0].target {
            Target::Asset(id) => id,
            _ => panic!("root asset"),
        };
        assert_ne!(root_asset, 50);
        let nested_q0rg = imported.q0rgs.iter().find(|q| q.q0rg_id == nested).unwrap();
        let nested_asset = match nested_q0rg.layers[0].placements[0].target {
            Target::Asset(id) => id,
            _ => panic!("nested asset"),
        };
        assert!(imported
            .assets
            .iter()
            .any(|asset| asset.id() == nested_asset));
        assert_eq!(
            imported.asset_names.get(&root_asset).map(String::as_str),
            Some("source vector")
        );
        assert!(imported.asset_appearances.contains_key(&root_asset));
        assert!(imported
            .layer_metadata
            .contains_key(&LayerKey::new(new_root, 8)));
        assert!(imported
            .runtime
            .frame_scripts
            .iter()
            .any(|script| script.q0rg_id == new_root));
        assert_eq!(
            imported
                .runtime
                .instance_names
                .get(&InstanceKey::new(new_root, 300))
                .map(String::as_str),
            Some("hand")
        );
        assert_eq!(
            imported
                .runtime
                .frame_labels
                .get(&FrameLabelKey::new(new_root, 0))
                .map(String::as_str),
            Some("idle")
        );
        q0s_format::v2::validate(&imported).unwrap();
    }

    #[test]
    fn asset_transfer_allocates_a_fresh_destination_id_and_keeps_metadata() {
        let mut source = crate::state::default_project();
        source.assets.push(vector(9, 0.0));
        source.asset_names.insert(9, "shared hand".into());
        let mut destination = crate::state::default_project();
        destination.assets.push(vector(9, 100.0));
        let (imported, item) =
            import_library_item(&source, &destination, LibraryItem::Asset(9)).unwrap();
        let LibraryItem::Asset(new_id) = item else {
            panic!("asset root")
        };
        assert_ne!(new_id, 9);
        assert_eq!(
            imported.asset_names.get(&new_id).map(String::as_str),
            Some("shared hand")
        );
        assert_eq!(imported.assets.len(), destination.assets.len() + 1);
        q0s_format::v2::validate(&imported).unwrap();
    }
}
