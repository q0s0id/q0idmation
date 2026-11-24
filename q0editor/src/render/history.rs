use super::*;

impl TextureCache {
    /// History swaps the model snapshot, but geometry for unchanged asset ids
    /// belongs to both snapshots. Keep it warm for the first repaint.
    pub(crate) fn restore_history(
        &mut self,
        before: &ProjectV2,
        restored: &ProjectV2,
        previous_revision: (u64, u64),
        revision: (u64, u64),
    ) {
        let changed_assets = changed_asset_ids(before, restored);
        let changed_timelines = changed_timeline_ids(before, restored);
        let affected_symbols =
            affected_symbol_ids(before, restored, &changed_assets, &changed_timelines);
        let cached_bounds = self.placement_bounds_cache.take();
        self.invalidate_assets(changed_assets.iter().copied());

        // Rebuild lookup tables against the restored snapshot. Outline and
        // symbol bounds are owned values and can survive an index reorder.
        if let Some(mut cached) =
            cached_bounds.filter(|cached| cached.revision == previous_revision)
        {
            let mut bounds = PlacementBoundsCache::new(restored);
            cached
                .bounds
                .asset_outlines
                .retain(|id, _| !changed_assets.contains(id));
            bounds.asset_outlines = cached.bounds.asset_outlines;
            cached
                .bounds
                .q0rg_local_bounds
                .retain(|(id, _), _| !affected_symbols.contains(id));
            bounds.q0rg_local_bounds = cached.bounds.q0rg_local_bounds;
            cached
                .bounds
                .q0rg_static_body_bounds
                .retain(|(id, _), _| !affected_symbols.contains(id));
            bounds.q0rg_static_body_bounds = cached.bounds.q0rg_static_body_bounds;
            cached
                .bounds
                .q0rg_frame_bounds
                .retain(|(id, _, _), _| !affected_symbols.contains(id));
            bounds.q0rg_frame_bounds = cached.bounds.q0rg_frame_bounds;
            cached
                .bounds
                .q0rg_rigged
                .retain(|(id, _), _| !affected_symbols.contains(id));
            bounds.q0rg_rigged = cached.bounds.q0rg_rigged;
            self.store_placement_bounds_cache(bounds, revision);
        }

        if self.vector_bounds_revision == Some(previous_revision) {
            self.vector_bounds_revision = Some(revision);
        }
        if self.active_placement_indexes_revision == Some(previous_revision) {
            self.active_placement_indexes
                .retain(|(id, _), _| !changed_timelines.contains(id));
            self.active_placement_indexes_revision = Some(revision);
        }
        if self.rig_pose_cache_revision == Some(previous_revision) {
            self.rig_pose_cache
                .retain(|(id, _), _| !affected_symbols.contains(id));
            self.rig_pose_cache_revision = Some(revision);
        }

        self.library_frame_previews.clear();
        self.library_frame_preview_revision = None;
        self.live_raw_stroke_scale_compensation.clear();
        self.classic_brush_preview = None;
        // Whole-frame outputs can also depend on parenting/rig state, even
        // when their own layer and asset ids did not change.
        self.fx_placement_textures.clear();
        self.mask_frame_textures.clear();
        self.mask_frame_revision = None;
        self.mask_stale
            .retain(|id, _| !affected_symbols.contains(id));
        #[cfg(feature = "appearance-mask-eraser")]
        {
            self.raw_area_selection = None;
        }
        // Shape tween plans validate their endpoints on the next render pass.
    }
}

fn changed_asset_ids(before: &ProjectV2, restored: &ProjectV2) -> HashSet<u16> {
    let mut old: HashMap<_, _> = before
        .assets
        .iter()
        .map(|asset| (asset.id(), asset))
        .collect();
    let mut changed: HashSet<_> = restored
        .assets
        .iter()
        .filter_map(|asset| {
            let id = asset.id();
            (old.remove(&id) != Some(asset)
                || before.asset_appearances.get(&id) != restored.asset_appearances.get(&id))
            .then_some(id)
        })
        .collect();
    changed.extend(old.into_keys());
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(target: Target) -> Placement {
        Placement {
            instance_id: 0,
            frame: 0,
            target,
            transform: Transform2D::IDENTITY,
            pivot: None,
            tween: Tween::None,
            fx: PlacementFx::default(),
        }
    }

    fn triangle(asset_id: u16, x: f32) -> Asset {
        Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![VPath {
                anchors: [(x, 0.0), (x + 20.0, 0.0), (x, 20.0)]
                    .into_iter()
                    .map(|(x, y)| Anchor {
                        point: Vec2::new(x, y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        })
    }

    fn scene() -> ProjectV2 {
        let mut project = crate::state::default_project();
        let root = &mut project.q0rgs[0];
        let mut child = root.clone();
        child.q0rg_id = 2;
        child.layers[0]
            .placements
            .push(placement(Target::Asset(10)));
        root.layers[0].placements.push(placement(Target::Q0rg(2)));
        project.q0rgs.push(child);
        project
            .assets
            .extend([triangle(10, 0.0), triangle(11, 50.0)]);
        project
    }

    fn warm_vectors(cache: &mut TextureCache, project: &ProjectV2) {
        for asset in &project.assets {
            if let Asset::Vector(vector) = asset {
                cache.vector_render_geometry(vector);
            }
        }
    }

    #[test]
    fn history_of_an_instance_transform_keeps_its_symbol_geometry_and_bounds() {
        let before = scene();
        let mut restored = before.clone();
        restored.q0rgs[0].layers[0].placements[0].transform.tx = 40.0;
        let mut textures = TextureCache::default();
        warm_vectors(&mut textures, &before);
        let revisions = ((1, 6), (1, 7));
        textures.with_placement_bounds_cache(&before, revisions.0, |cache, _| {
            cache.q0rg_shown_bbox(&before, 1, 0).unwrap();
            cache.q0rg_shown_bbox(&before, 2, 0).unwrap();
        });

        textures.restore_history(&before, &restored, revisions.0, revisions.1);
        warm_vectors(&mut textures, &restored);
        assert_eq!(textures.vector_render_geometry_build_count(), 2);
        textures.with_placement_bounds_cache(&restored, revisions.1, |cache, _| {
            assert!(cache.q0rg_frame_bounds.keys().any(|(id, _, _)| *id == 2));
            assert!(!cache.q0rg_frame_bounds.keys().any(|(id, _, _)| *id == 1));
            assert_eq!(
                cache.q0rg_shown_bbox(&restored, 1, 0),
                Some((40., 0., 60., 20.))
            );
        });
    }

    #[test]
    fn history_of_an_asset_edit_invalidates_its_mesh_and_parent_bounds_only() {
        let before = scene();
        let mut restored = before.clone();
        // Swap the ids' slots as well, so retained bounds must use new lookups.
        restored.assets = vec![triangle(11, 50.0), triangle(10, 15.0)];
        let mut textures = TextureCache::default();
        warm_vectors(&mut textures, &before);
        let revisions = ((1, 0), (1, 1));
        textures.with_placement_bounds_cache(&before, revisions.0, |cache, _| {
            assert_eq!(
                cache.q0rg_shown_bbox(&before, 1, 0),
                Some((0., 0., 20., 20.))
            );
        });

        textures.restore_history(&before, &restored, revisions.0, revisions.1);
        warm_vectors(&mut textures, &restored);
        assert_eq!(textures.vector_render_geometry_build_count(), 3);
        textures.with_placement_bounds_cache(&restored, revisions.1, |cache, _| {
            assert_eq!(
                cache.q0rg_shown_bbox(&restored, 1, 0),
                Some((15., 0., 35., 20.))
            );
        });
    }
}

fn changed_timeline_ids(before: &ProjectV2, restored: &ProjectV2) -> HashSet<u16> {
    let mut old: HashMap<_, _> = before
        .q0rgs
        .iter()
        .map(|symbol| (symbol.q0rg_id, symbol))
        .collect();
    let mut changed: HashSet<_> = restored
        .q0rgs
        .iter()
        .filter_map(|symbol| {
            (old.remove(&symbol.q0rg_id) != Some(symbol)).then_some(symbol.q0rg_id)
        })
        .collect();
    changed.extend(old.into_keys());
    changed
}

fn affected_symbol_ids(
    before: &ProjectV2,
    restored: &ProjectV2,
    changed_assets: &HashSet<u16>,
    changed_timelines: &HashSet<u16>,
) -> HashSet<u16> {
    if before.meta != restored.meta
        || before.runtime.scene3d != restored.runtime.scene3d
        || before.runtime.layer_parents != restored.runtime.layer_parents
    {
        return before
            .q0rgs
            .iter()
            .chain(&restored.q0rgs)
            .map(|symbol| symbol.q0rg_id)
            .collect();
    }
    let mut affected = changed_timelines.clone();
    for key in before
        .layer_metadata
        .keys()
        .chain(restored.layer_metadata.keys())
    {
        if before.layer_metadata.get(key) != restored.layer_metadata.get(key) {
            affected.insert(key.q0rg_id);
        }
    }
    for asset in before.assets.iter().chain(&restored.assets) {
        if let Asset::Rig(rig) = asset {
            if changed_assets.contains(&rig.asset_id) {
                affected.insert(rig.owner_q0rg_id);
            }
        }
    }

    // A child's change can affect every symbol that contains it, but a move
    // of one instance does not change the child's own outline or geometry.
    let mut parents: HashMap<u16, HashSet<u16>> = HashMap::new();
    for symbol in &restored.q0rgs {
        for placement in symbol.layers.iter().flat_map(|layer| &layer.placements) {
            match placement.target {
                Target::Asset(id) if changed_assets.contains(&id) => {
                    affected.insert(symbol.q0rg_id);
                }
                Target::Q0rg(id) => {
                    parents.entry(id).or_default().insert(symbol.q0rg_id);
                }
                Target::Asset(_) => {}
            }
        }
    }
    let mut pending: Vec<_> = affected.iter().copied().collect();
    while let Some(id) = pending.pop() {
        for &parent in parents.get(&id).into_iter().flatten() {
            if affected.insert(parent) {
                pending.push(parent);
            }
        }
    }
    affected
}
