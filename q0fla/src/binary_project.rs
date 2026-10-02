use super::*;
use archive::{Archive, Kind};
use q0s_format::v2::{Asset, BitmapAsset, LayerKey, LayerKind, LayerMetadata};

pub(super) fn import(path: &Path) -> Result<ImportedProject, ImportError> {
    let mut compound = CompoundFile::open(File::open(path)?).map_err(|error| {
        ImportError::Invalid(format!("legacy FLA compound container is damaged: {error}"))
    })?;
    let streams = compound
        .walk()
        .filter(|entry| entry.is_stream())
        .map(|entry| StreamInfo {
            name: entry
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
            path: entry.path().to_owned(),
            len: entry.len(),
        })
        .collect::<Vec<_>>();
    let contents = streams
        .iter()
        .find(|stream| stream.name.eq_ignore_ascii_case("Contents"))
        .map(|stream| read_stream(&mut compound, stream))
        .transpose()?
        .unwrap_or_default();
    let metadata = read_legacy_metadata(&contents);
    let mut pages = streams
        .iter()
        .filter_map(|s| numbered_stream(&s.name, "Page ").map(|n| (n, s)))
        .collect::<Vec<_>>();
    let mut symbols = streams
        .iter()
        .filter_map(|s| numbered_stream(&s.name, "Symbol ").map(|n| (n, s)))
        .collect::<Vec<_>>();
    pages.sort_by_key(|(n, _)| *n);
    symbols.sort_by_key(|(n, _)| *n);
    if pages.is_empty() || pages.len() + symbols.len() >= usize::from(u16::MAX) {
        return Err(ImportError::Invalid(
            "legacy FLA has no scene or too many library symbols".into(),
        ));
    }
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let mut project = ProjectV2 {
        meta: ProjectMeta {
            name,
            fps: metadata.fps,
            stage_width: metadata.width,
            stage_height: metadata.height,
            background_color: metadata.background,
            entry_q0rg_id: 1,
        },
        assets: Vec::new(),
        asset_names: HashMap::new(),
        asset_appearances: HashMap::new(),
        layer_metadata: HashMap::new(),
        audio_clips: Vec::new(),
        runtime: ProjectRuntimeData::default(),
        q0rgs: Vec::new(),
    };
    let mut symbol_ids = BTreeMap::new();
    let mut next_q0rg = 1_u16;
    for (i, (number, _)) in pages.iter().enumerate() {
        let name = metadata
            .names
            .scenes
            .get(number)
            .cloned()
            .unwrap_or_else(|| format!("Scene {}", i + 1));
        project.q0rgs.push(Q0rg {
            q0rg_id: next_q0rg,
            name: name.clone(),
            frame_count: 1,
            script: String::new(),
            layers: Vec::new(),
        });
        project.runtime.scenes.push(ProjectScene {
            scene_id: (i + 1) as u16,
            q0rg_id: next_q0rg,
            name,
        });
        next_q0rg += 1;
    }
    for (number, _) in &symbols {
        let name = metadata
            .names
            .symbols
            .get(number)
            .cloned()
            .unwrap_or_else(|| format!("Symbol {number}"));
        if let Some((folder, _)) = name.rsplit_once('/') {
            if !folder.is_empty() {
                project.runtime.library_folders.push(folder.to_owned());
            }
        }
        symbol_ids.insert(*number, next_q0rg);
        project.q0rgs.push(Q0rg {
            q0rg_id: next_q0rg,
            name,
            frame_count: 1,
            script: String::new(),
            layers: Vec::new(),
        });
        next_q0rg += 1;
    }
    project.runtime.library_folders.sort();
    project.runtime.library_folders.dedup();
    let mut warnings = Vec::new();
    let mut next_asset = 1_u16;
    let mut bitmaps = HashMap::new();
    for stream in &streams {
        let Some(number) = numbered_stream(&stream.name, "Media ") else {
            continue;
        };
        let bytes = read_stream(&mut compound, stream)?;
        if !matches!(bytes.get(..2), Some([3, 3] | [3, 5])) {
            continue;
        }
        match xfl_content::decode_lossless_bitmap_dat(&bytes) {
            Ok((width, height, rgba)) => {
                let id = next_asset;
                next_asset = next_asset
                    .checked_add(1)
                    .ok_or_else(|| ImportError::Invalid("asset id overflow".into()))?;
                project.assets.push(Asset::Bitmap(BitmapAsset {
                    asset_id: id,
                    width,
                    height,
                    rgba,
                }));
                project.asset_names.insert(id, stream.name.clone());
                bitmaps.insert(number, id);
            }
            Err(error) => warnings.push(format!("{}: {error}", stream.name)),
        }
    }
    let owners = pages
        .iter()
        .enumerate()
        .map(|(i, (_, s))| ((i + 1) as u16, *s))
        .chain(symbols.iter().map(|(n, s)| (symbol_ids[n], *s)))
        .collect::<Vec<_>>();
    for (q0rg_id, stream) in owners {
        let bytes = read_stream(&mut compound, stream)?;
        let owner = project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map(|q| q.name.clone())
            .unwrap_or_default();
        match archive::parse(&bytes) {
            Ok(archive) => {
                warnings.extend(archive.warnings.iter().map(|w| format!("{owner}: {w}")));
                let mut converter = Converter {
                    archive: &archive,
                    project: &mut project,
                    symbols: &symbol_ids,
                    bitmaps: &bitmaps,
                    next_asset: &mut next_asset,
                    next_q0rg: &mut next_q0rg,
                    warnings: &mut warnings,
                    owner: &owner,
                    targets: HashMap::new(),
                };
                converter.timeline(q0rg_id)?;
            }
            Err(error) => {
                warnings.push(format!(
                    "{owner}: {error}; this stream uses static vector recovery"
                ));
                let recovered = recover_shapes(
                    &bytes,
                    MAX_BINARY_SHAPES.saturating_sub(project.assets.len()),
                );
                let mut layer = empty_layer(1, "Recovered artwork");
                add_shapes_to_layer(
                    recovered,
                    0,
                    &owner,
                    &mut layer,
                    &mut project,
                    &mut next_asset,
                    &mut warnings,
                )?;
                project
                    .q0rgs
                    .iter_mut()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .expect("allocated stream")
                    .layers = vec![layer];
            }
        }
    }
    // All library frame counts are now known, including forward references.
    let lengths = project
        .q0rgs
        .iter()
        .map(|q| (q.q0rg_id, q.frame_count))
        .collect::<HashMap<_, _>>();
    for q in &mut project.q0rgs {
        for layer in &mut q.layers {
            for placement in &mut layer.placements {
                if let Target::Q0rg(id) = placement.target {
                    let max = lengths[&id].saturating_sub(1);
                    placement.fx.q0rg_playback.first_frame =
                        placement.fx.q0rg_playback.first_frame.min(max);
                    placement.fx.q0rg_playback.single_frame =
                        placement.fx.q0rg_playback.single_frame.min(max);
                }
            }
        }
    }
    warnings.sort();
    warnings.dedup();
    q0s_format::v2::assign_missing_instance_ids(&mut project)
        .map_err(|e| ImportError::Invalid(e.to_string()))?;
    q0s_format::v2::validate(&project).map_err(|error| {
        ImportError::Invalid(format!("converted q1s model failed validation: {error}"))
    })?;
    let imported_layers = project.q0rgs.iter().map(|q| q.layers.len()).sum();
    let imported_keyframes = project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .map(|l| l.keyframe_frames().len())
        .sum();
    Ok(ImportedProject {
        project,
        report: ImportReport {
            source_kind: "legacy binary fla".into(),
            imported_layers,
            imported_keyframes,
            warnings,
        },
    })
}

struct Converter<'a> {
    archive: &'a Archive,
    project: &'a mut ProjectV2,
    symbols: &'a BTreeMap<u32, u16>,
    bitmaps: &'a HashMap<u32, u16>,
    next_asset: &'a mut u16,
    next_q0rg: &'a mut u16,
    warnings: &'a mut Vec<String>,
    owner: &'a str,
    targets: HashMap<usize, Vec<Target>>,
}

impl Converter<'_> {
    fn shape_targets(&mut self, shape: &RawShape) -> Result<Vec<Target>, ImportError> {
        if !shape
            .fills
            .values()
            .any(|fill| matches!(fill, RawFillStyle::Bitmap { .. }))
        {
            return Ok(xfl_content::shape_assets(
                shape,
                self.owner,
                self.project,
                self.next_asset,
                self.warnings,
            )?
            .into_iter()
            .map(Target::Asset)
            .collect());
        }
        let mut targets = Vec::new();
        let mut styles = shape.fills.keys().copied().collect::<Vec<_>>();
        styles.sort_unstable();
        for style in styles {
            let fill = &shape.fills[&style];
            let mut isolated = shape.clone();
            isolated.fills.retain(|id, _| *id == style);
            isolated.strokes.clear();
            for edge in &mut isolated.edges {
                edge.fill0 = edge.fill0.filter(|id| *id == style);
                edge.fill1 = edge.fill1.filter(|id| *id == style);
                edge.stroke = None;
            }
            let RawFillStyle::Bitmap {
                reference,
                transform,
                tiled,
            } = fill
            else {
                targets.extend(
                    xfl_content::shape_assets(
                        &isolated,
                        self.owner,
                        self.project,
                        self.next_asset,
                        self.warnings,
                    )?
                    .into_iter()
                    .map(Target::Asset),
                );
                continue;
            };
            let Some(&bitmap) = self.bitmaps.get(reference) else {
                self.warnings.push(format!(
                    "{}: missing bitmap fill Media {reference}",
                    self.owner
                ));
                continue;
            };
            let texture = Affine::compose(shape.affine, *transform);
            let Some(transform) = xfl_content::affine_to_transform(texture) else {
                self.warnings.push(format!(
                    "{}: degenerate bitmap fill transform was skipped",
                    self.owner
                ));
                continue;
            };
            isolated.fills.insert(
                style,
                RawFillStyle::Solid(Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                }),
            );
            let mask_assets = xfl_content::shape_assets(
                &isolated,
                self.owner,
                self.project,
                self.next_asset,
                self.warnings,
            )?;
            if mask_assets.is_empty() {
                continue;
            }
            let id = self.allocate_group()?;
            let mut content = empty_layer(1, "Bitmap");
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (0_i32, 0_i32, 1_i32, 1_i32);
            let bitmap_size = self
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Bitmap(value) if value.asset_id == bitmap => {
                        Some((f32::from(value.width), f32::from(value.height)))
                    }
                    _ => None,
                })
                .expect("allocated bitmap media");
            if *tiled {
                let inverse = texture.inverse().ok_or_else(|| {
                    ImportError::Invalid("bitmap fill transform is not invertible".into())
                })?;
                let mut bounds = (
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                );
                for asset in &self.project.assets {
                    let Asset::Vector(vector) = asset else {
                        continue;
                    };
                    if !mask_assets.contains(&vector.asset_id) {
                        continue;
                    }
                    for path in &vector.paths {
                        for anchor in &path.anchors {
                            for point in [Some(anchor.point), anchor.in_handle, anchor.out_handle]
                                .into_iter()
                                .flatten()
                            {
                                let point = inverse.apply(point);
                                bounds.0 = bounds.0.min(point.x);
                                bounds.1 = bounds.1.min(point.y);
                                bounds.2 = bounds.2.max(point.x);
                                bounds.3 = bounds.3.max(point.y);
                            }
                        }
                    }
                }
                min_x = (bounds.0 / bitmap_size.0).floor() as i32;
                min_y = (bounds.1 / bitmap_size.1).floor() as i32;
                max_x = (bounds.2 / bitmap_size.0).ceil() as i32;
                max_y = (bounds.3 / bitmap_size.1).ceil() as i32;
                let count = i64::from(max_x)
                    .saturating_sub(i64::from(min_x))
                    .saturating_mul(i64::from(max_y).saturating_sub(i64::from(min_y)));
                if !(1..=4096).contains(&count) {
                    return Err(ImportError::Unsupported(format!(
                        "{}: bitmap fill repetition exceeds 4096 tiles",
                        self.owner
                    )));
                }
            }
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let transform = if *tiled {
                        xfl_content::affine_to_transform(Affine::compose(
                            texture,
                            Affine {
                                tx: x as f32 * bitmap_size.0,
                                ty: y as f32 * bitmap_size.1,
                                ..Affine::IDENTITY
                            },
                        ))
                        .ok_or_else(|| {
                            ImportError::Invalid(
                                "bitmap tile transform is not representable".into(),
                            )
                        })?
                    } else {
                        transform
                    };
                    content.push_placement(Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(bitmap),
                        transform,
                        tween: Tween::None,
                        fx: PlacementFx::default(),
                    });
                }
            }
            let mut mask = empty_layer(2, "Fill boundary");
            for asset in mask_assets {
                mask.push_placement(Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(asset),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                });
            }
            self.project.layer_metadata.insert(
                LayerKey::new(id, 1),
                LayerMetadata {
                    mask_layer_id: Some(2),
                    ..Default::default()
                },
            );
            self.project.layer_metadata.insert(
                LayerKey::new(id, 2),
                LayerMetadata {
                    kind: LayerKind::Mask,
                    ..Default::default()
                },
            );
            self.project.q0rgs.push(Q0rg {
                q0rg_id: id,
                name: format!("{} / Bitmap fill {id}", self.owner),
                frame_count: 1,
                script: String::new(),
                layers: vec![content, mask],
            });
            targets.push(Target::Q0rg(id));
        }
        let mut strokes = shape.clone();
        strokes.fills.clear();
        for edge in &mut strokes.edges {
            edge.fill0 = None;
            edge.fill1 = None;
        }
        targets.extend(
            xfl_content::shape_assets(
                &strokes,
                self.owner,
                self.project,
                self.next_asset,
                self.warnings,
            )?
            .into_iter()
            .map(Target::Asset),
        );
        Ok(targets)
    }

    fn allocate_group(&mut self) -> Result<u16, ImportError> {
        let id = *self.next_q0rg;
        *self.next_q0rg = id
            .checked_add(1)
            .ok_or_else(|| ImportError::Invalid("legacy groups exceed q0rg id space".into()))?;
        Ok(id)
    }

    fn timeline(&mut self, q0rg_id: u16) -> Result<(), ImportError> {
        let root = &self.archive.nodes[self.archive.root];
        let layer_ids = root
            .children
            .iter()
            .copied()
            .filter(|id| matches!(self.archive.nodes[*id].kind, Kind::Layer(_)))
            .collect::<Vec<_>>();
        let model_ids = layer_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, (i + 1) as u16))
            .collect::<HashMap<_, _>>();
        let mut layers = Vec::new();
        let mut length = 1_u16;
        for node_id in layer_ids {
            let Kind::Layer(info) = &self.archive.nodes[node_id].kind else {
                unreachable!()
            };
            let layer_id = model_ids[&node_id];
            let mut layer = empty_layer(layer_id, &info.name);
            let metadata = LayerMetadata {
                kind: match info.kind {
                    3 => LayerKind::Folder,
                    4 => LayerKind::Mask,
                    _ => LayerKind::Normal,
                },
                locked: info.locked,
                hidden: info.hidden,
                parent_folder_id: info
                    .parent
                    .filter(|parent| matches!(&self.archive.nodes[*parent].kind, Kind::Layer(info) if info.kind == 3))
                    .and_then(|parent| model_ids.get(&parent).copied()),
                mask_layer_id: info
                    .parent
                    .filter(|parent| matches!(&self.archive.nodes[*parent].kind, Kind::Layer(info) if info.kind == 4))
                    .and_then(|parent| model_ids.get(&parent).copied()),
                ..Default::default()
            };
            if metadata != LayerMetadata::default() {
                self.project
                    .layer_metadata
                    .insert(LayerKey::new(q0rg_id, layer_id), metadata);
            }
            let mut frame = 0_u16;
            if info.kind == 3 {
                layer.explicit_keyframes.clear();
                layer.timeline_frame_count = Some(0);
                layers.push(layer);
                continue;
            }
            for child in self.archive.nodes[node_id].children.clone() {
                let Kind::Frame { span } = self.archive.nodes[child].kind else {
                    continue;
                };
                layer.ensure_explicit_keyframe(frame);
                if info.kind != 3 {
                    self.frame_content(child, frame, &mut layer)?;
                }
                frame = frame.checked_add(span).ok_or_else(|| {
                    ImportError::Invalid(format!(
                        "{}: layer timeline exceeds q1s frame range",
                        self.owner
                    ))
                })?;
            }
            length = length.max(frame);
            layer.timeline_frame_count = Some(frame.max(1));
            layers.push(layer);
        }
        if layers.is_empty() {
            layers.push(empty_layer(1, "Artwork"));
        }
        // Binary FLA stores bottom layers first, matching q1s paint order.
        let q = self
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
            .expect("allocated timeline");
        q.frame_count = length;
        q.layers = layers;
        crate::normalize_folder_layer_order(q, &self.project.layer_metadata);
        Ok(())
    }

    fn frame_content(
        &mut self,
        id: usize,
        frame: u16,
        layer: &mut Layer,
    ) -> Result<(), ImportError> {
        let node = &self.archive.nodes[id];
        // A frame's own shape is its raw artwork; grouped/display children are
        // separate depth entries. Neither the frame nor groups discard children.
        if let Some(shape) = &node.shape {
            let mut shape = shape.clone();
            shape.affine = node.affine;
            for target in self.shape_targets(&shape)? {
                layer.push_placement(Placement {
                    instance_id: 0,
                    frame,
                    target,
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                });
            }
        }
        for child in node.children.clone() {
            self.place(child, frame, layer, node.affine, 0)?;
        }
        Ok(())
    }

    fn place(
        &mut self,
        id: usize,
        frame: u16,
        layer: &mut Layer,
        parent: Affine,
        depth: usize,
    ) -> Result<(), ImportError> {
        if depth > 256 {
            return Err(ImportError::Invalid(
                "legacy group nesting exceeds limit".into(),
            ));
        }
        let node = &self.archive.nodes[id];
        let targets = if let Some(targets) = self.targets.get(&id) {
            targets.clone()
        } else {
            let mut targets = Vec::new();
            match &node.kind {
                Kind::Symbol { reference, .. } => {
                    if let Some(id) = self.symbols.get(reference) {
                        targets.push(Target::Q0rg(*id));
                    } else {
                        self.warnings.push(format!(
                            "{}: missing library Symbol {reference}",
                            self.owner
                        ));
                    }
                }
                Kind::Bitmap { reference } => {
                    if let Some(id) = self.bitmaps.get(reference) {
                        targets.push(Target::Asset(*id));
                    } else {
                        self.warnings
                            .push(format!("{}: missing bitmap Media {reference}", self.owner));
                    }
                }
                Kind::Text(text) => {
                    if let Some(id) = xfl_content::text_asset(
                        text,
                        self.owner,
                        self.project,
                        self.next_asset,
                        self.warnings,
                    )? {
                        targets.push(Target::Asset(id));
                    }
                }
                _ => {
                    if let Some(shape) = &node.shape {
                        targets.extend(self.shape_targets(shape)?);
                    }
                    if !node.children.is_empty() || targets.len() > 1 {
                        let group_id = self.allocate_group()?;
                        let mut group = empty_layer(1, "Artwork");
                        for target in targets {
                            group.push_placement(Placement {
                                instance_id: 0,
                                frame: 0,
                                target,
                                transform: Transform2D::IDENTITY,
                                tween: Tween::None,
                                fx: PlacementFx::default(),
                            });
                        }
                        for child in node.children.clone() {
                            self.place(child, 0, &mut group, Affine::IDENTITY, depth + 1)?;
                        }
                        self.project.q0rgs.push(Q0rg {
                            q0rg_id: group_id,
                            name: format!("{} / Group {group_id}", self.owner),
                            frame_count: 1,
                            script: String::new(),
                            layers: vec![group],
                        });
                        targets = vec![Target::Q0rg(group_id)];
                    }
                }
            }
            self.targets.insert(id, targets.clone());
            targets
        };
        if targets.is_empty() {
            return Ok(());
        }
        let Some(transform) =
            xfl_content::affine_to_transform(Affine::compose(parent, node.affine))
        else {
            self.warnings.push(format!(
                "{}: degenerate display-object transform was skipped",
                self.owner
            ));
            return Ok(());
        };
        for target in targets {
            let mut fx = PlacementFx::default();
            if let Kind::Symbol {
                first_frame,
                mode,
                opacity,
                ..
            } = node.kind
            {
                fx.q0rg_playback.first_frame = first_frame;
                fx.q0rg_playback.single_frame = first_frame;
                fx.q0rg_playback.mode = mode;
                fx.opacity = opacity;
            }
            layer.push_placement(Placement {
                instance_id: 0,
                frame,
                target,
                transform,
                tween: Tween::None,
                fx,
            });
        }
        Ok(())
    }
}
