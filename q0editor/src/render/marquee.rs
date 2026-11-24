use super::*;
use crate::state::PlacementRef;
use q0s_format::v2::{Layer, Stroke as VectorStroke};

type Bounds = (f32, f32, f32, f32);

#[derive(Default)]
pub(crate) struct MarqueeCandidates<'a> {
    pub objects: Vec<PlacementRef>,
    pub raw: Vec<(PlacementRef, &'a VectorAsset)>,
}

pub(crate) fn marquee_candidates<'a>(
    project: &'a ProjectV2,
    q0rg_id: u16,
    frame: u16,
    bounds: Bounds,
    textures: &mut TextureCache,
    revision: Option<(u64, u64)>,
) -> MarqueeCandidates<'a> {
    let mut picker = Picker::new(project, q0rg_id, textures, revision);
    let Some(q0rg) = picker.lookup.q0rg(q0rg_id) else {
        return MarqueeCandidates::default();
    };
    let area = Area::rect(bounds);
    let mut hits = MarqueeCandidates::default();
    for layer in q0rg.layers.iter().rev() {
        if project.layer_is_locked(q0rg_id, layer.layer_id) {
            continue;
        }
        picker.visit_painted_layer(
            q0rg,
            layer,
            frame,
            area,
            (area, Affine::IDENTITY),
            0,
            true,
            |index, raw| {
                let reference = PlacementRef {
                    q0rg_id,
                    layer_id: layer.layer_id,
                    placement_idx: index,
                };
                if let Some(vector) = raw {
                    hits.raw.push((reference, vector));
                } else {
                    hits.objects.push(reference);
                }
                false
            },
        );
    }
    hits
}

/// Test the sampled poses, including raw drawings, for Edit Multiple Frames.
pub(crate) fn frame_region_hits(
    project: &ProjectV2,
    q0rg_id: u16,
    poses: &[(PlacementRef, u16)],
    bounds: Bounds,
    textures: &mut TextureCache,
    revision: (u64, u64),
) -> Vec<bool> {
    let mut results = vec![false; poses.len()];
    let mut picker = Picker::new(project, q0rg_id, textures, Some(revision));
    let Some(symbol) = picker.lookup.q0rg(q0rg_id) else {
        return results;
    };
    let mut groups = std::collections::BTreeMap::<_, HashMap<_, _>>::new();
    for (result, (reference, frame)) in poses.iter().enumerate() {
        groups
            .entry((*frame, reference.layer_id))
            .or_default()
            .insert(reference.placement_idx, result);
    }
    let area = Area::rect(bounds);
    for ((frame, layer_id), indices) in groups {
        let Some(layer) = symbol
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
        else {
            continue;
        };
        picker.visit_painted_layer(
            symbol,
            layer,
            frame,
            area,
            (area, Affine::IDENTITY),
            0,
            false,
            |index, _| {
                if let Some(&result) = indices.get(&index) {
                    results[result] = true;
                }
                false
            },
        );
    }
    results
}

struct Picker<'a, 'cache> {
    project: &'a ProjectV2,
    lookup: RenderLookup<'a>,
    textures: &'cache mut TextureCache,
    projects_flat: bool,
}

impl<'a, 'cache> Picker<'a, 'cache> {
    fn new(
        project: &'a ProjectV2,
        q0rg_id: u16,
        textures: &'cache mut TextureCache,
        revision: Option<(u64, u64)>,
    ) -> Self {
        let index = textures.render_index(project, revision);
        Self {
            project,
            lookup: RenderLookup::new(project, index, revision.is_none()),
            textures,
            projects_flat: q0glblend::free_view().is_some()
                || project
                    .runtime
                    .scene3d
                    .setup(q0rg_id)
                    .is_some_and(|setup| setup.light_flat && setup.has_lights()),
        }
    }

    fn layer_visible(&self, q0rg_id: u16, layer_id: u16) -> bool {
        self.project.layer_is_visible(q0rg_id, layer_id)
            && self
                .project
                .layer_metadata
                .get(&LayerKey { q0rg_id, layer_id })
                .is_none_or(|metadata| metadata.fx.filters.opacity > 1.0e-6)
    }

    fn symbol_hits(
        &mut self,
        symbol: &Q0rg,
        frame: u16,
        area: Area,
        stroke_space: (Area, Affine),
        depth: u8,
    ) -> bool {
        depth <= Q0RG_RECURSION_LIMIT
            && symbol.layers.iter().rev().any(|layer| {
                self.visit_painted_layer(
                    symbol,
                    layer,
                    frame,
                    area,
                    stroke_space,
                    depth,
                    false,
                    |_, _| true,
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_painted_layer(
        &mut self,
        symbol: &Q0rg,
        layer: &Layer,
        frame: u16,
        area: Area<'_>,
        (stroke_area, affine): (Area<'_>, Affine),
        depth: u8,
        separate_raw: bool,
        on_hit: impl FnMut(usize, Option<&'a VectorAsset>) -> bool,
    ) -> bool {
        let q0rg_id = symbol.q0rg_id;
        if !self.layer_visible(q0rg_id, layer.layer_id) {
            return false;
        }
        let metadata = self.project.layer_metadata(q0rg_id, layer.layer_id);
        if metadata.kind == q0s_format::v2::LayerKind::Mask {
            if depth != 0 || !self.textures.mask_art_visible(q0rg_id, layer.layer_id) {
                return false;
            }
            return self.visit_layer(
                q0rg_id,
                layer,
                frame,
                area,
                (stroke_area, affine),
                depth,
                separate_raw,
                on_hit,
            );
        }
        let mut clipped = None;
        for id in gpu_mask_sources(self.project, symbol, layer.layer_id, frame, self.textures) {
            let Some(mask) = symbol.layers.iter().find(|layer| layer.layer_id == id) else {
                return false;
            };
            if !mask_layer_has_active_art(self.project, symbol, id, frame, self.textures)
                || !self.layer_visible(q0rg_id, id)
            {
                return false;
            }
            let output = RefCell::new(Vec::new());
            let clip = clipped.as_deref().or(area.clip);
            // The captured geometry is expressed in the same coordinates as
            // the query so the masked artwork must touch the same spot.
            self.visit_layer(
                q0rg_id,
                mask,
                frame,
                area.with_mask(clip, Some(&output)),
                (stroke_area.with_mask(clip, Some(&output)), affine),
                depth,
                false,
                |_, _| false,
            );
            let region = output.into_inner();
            if region.is_empty() {
                return false;
            }
            clipped = Some(region);
        }
        let clip = clipped.as_deref().or(area.clip);
        self.visit_layer(
            q0rg_id,
            layer,
            frame,
            area.with_mask(clip, area.capture),
            (stroke_area.with_mask(clip, area.capture), affine),
            depth,
            separate_raw,
            on_hit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_layer(
        &mut self,
        q0rg_id: u16,
        layer: &Layer,
        frame: u16,
        area: Area,
        (stroke_area, to_stroke_space): (Area, Affine),
        depth: u8,
        separate_raw: bool,
        mut on_hit: impl FnMut(usize, Option<&'a VectorAsset>) -> bool,
    ) -> bool {
        // Use the same resolved frame and authored meshes as painting. A symbol's
        // bounds across every keyframe are irrelevant to this query.
        let active = self
            .textures
            .active_placement_states_at(q0rg_id, layer, frame);
        if active.is_empty() {
            return false;
        }
        let pose = self.lookup.rig_pose(q0rg_id, frame);
        let rig = self.lookup.rig(q0rg_id);
        let morph = active
            .iter()
            .any(|state| layer.placements[state.index].tween.is_shape())
            .then(|| {
                self.textures
                    .shape_tween_frame(&self.lookup, q0rg_id, layer, frame)
            })
            .flatten();

        for state in active.iter().rev() {
            let placement = &layer.placements[state.index];
            if state.fx.opacity <= 0.0 {
                continue;
            }
            let target = match (rig, pose.as_deref()) {
                (Some(rig), Some(pose)) => q0s_format::rig::resolved_variant_target(
                    rig,
                    pose,
                    placement.instance_id,
                    placement.target,
                ),
                _ => placement.target,
            };
            let affine = pose
                .as_deref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(state.transform));
            let asset = match target {
                Target::Asset(id) => self.lookup.asset(id),
                Target::Q0rg(_) => None,
            };
            if separate_raw && target == placement.target {
                if let Some(Asset::Vector(vector)) = asset {
                    if crate::tools::is_raw_vector_placement(
                        self.project,
                        placement,
                        vector.asset_id,
                    ) {
                        if (self
                            .project
                            .asset_appearances
                            .contains_key(&vector.asset_id)
                            || self
                                .vector_bounds(vector)
                                .is_some_and(|bounds| area.bounds.intersects(as_rect(bounds))))
                            && (area.clip.is_none()
                                || area.to_local(affine).is_some_and(|local| {
                                    self.asset_hits(
                                        asset.expect("vector asset"),
                                        local,
                                        Some((stroke_area, affine)),
                                    )
                                }))
                        {
                            on_hit(state.index, Some(vector));
                        }
                        continue;
                    }
                }
            }
            let projection = if depth == 0
                && (self.projects_flat || q0s_format::three::frame::is_3d(asset, &state.fx))
            {
                resolved_placement_3d_stage_projection(
                    self.project,
                    q0rg_id,
                    layer,
                    state.index,
                    frame,
                    ResolvedPlacement {
                        local_affine: affine,
                        fx: state.fx,
                        playback_start_frame: state.playback_start_frame,
                    },
                    None,
                )
            } else {
                None
            };
            let projected_clip;
            let local = if let Some(projection) = projection {
                let Some(bounds) = projection.stage_bounds else {
                    continue;
                };
                let intersection = area.bounds.intersect(as_rect(bounds));
                if intersection.is_negative() {
                    continue;
                }
                if matches!(asset, Some(Asset::Model(_))) {
                    if area.overlaps_rect(intersection) && on_hit(state.index, None) {
                        return true;
                    }
                    continue;
                }
                let map = |point| projection.stage_to_local_card(point);
                projected_clip = area.clip.map(|pieces| {
                    pieces
                        .iter()
                        .filter_map(|piece| piece.map(map))
                        .collect::<Vec<_>>()
                });
                Area::from_rect(intersection)
                    .map(map)
                    .map(|local| local.with_mask(projected_clip.as_deref(), None))
            } else {
                area.to_local(affine)
            };
            let Some(local) = local else {
                continue;
            };
            // Strokes under a non-uniform transform are painted at a uniform
            // width around the transformed centerline, after composing ancestors.
            let stroke_space = projection
                .is_none()
                .then(|| (stroke_area, Affine::compose(to_stroke_space, affine)));
            let hit = if let Some(shapes) = morph.as_ref().and_then(|morph| morph.get(&state.index))
            {
                shapes
                    .iter()
                    .any(|shape| morphed_shape_hits(shape, local, stroke_space))
            } else if let Some(asset) = asset {
                let deformed = match (asset, rig, pose.as_deref()) {
                    (Asset::Vector(vector), Some(rig), Some(pose)) => {
                        q0s_format::rig::deform_vector_for_instance(
                            rig,
                            pose,
                            placement.instance_id,
                            vector,
                        )
                    }
                    _ => None,
                };
                if let Some(vector) = deformed {
                    let appearance = self.project.asset_appearances.get(&asset.id());
                    let geometry = build_dynamic_vector_body_geometry(&vector, appearance);
                    vector_hits(&vector, &geometry, local, stroke_space)
                } else {
                    self.asset_hits(asset, local, stroke_space)
                }
            } else if let Target::Q0rg(child) = target {
                child != q0rg_id
                    && self.lookup.q0rg(child).is_some_and(|symbol| {
                        let child_frame = state.fx.q0rg_playback.resolve(
                            frame,
                            state.playback_start_frame,
                            symbol.frame_count,
                        );
                        self.symbol_hits(
                            symbol,
                            child_frame,
                            local,
                            stroke_space.unwrap_or((local, Affine::IDENTITY)),
                            depth + 1,
                        )
                    })
            } else {
                false
            };
            if hit && on_hit(state.index, None) {
                return true;
            }
        }
        false
    }

    fn asset_hits(
        &mut self,
        asset: &Asset,
        area: Area,
        stroke_space: Option<(Area, Affine)>,
    ) -> bool {
        match asset {
            Asset::Vector(vector) => {
                if let Some(appearance) = self.project.asset_appearances.get(&vector.asset_id) {
                    if appearance.fill_gradient.is_some() {
                        let cached = self.textures.gradient_render_geometry(vector, appearance);
                        if let (Some(mesh), Some(bounds)) = (&cached.mesh, cached.source_bounds) {
                            if cached.stops.iter().any(|stop| stop.color.a != 0)
                                && area.bounds.intersects(bounds)
                                && area.mesh_hits(mesh)
                            {
                                return true;
                            }
                        }
                        if let Some(stroke) = vector.stroke.filter(|stroke| stroke.color.a != 0) {
                            let geometry = self.textures.vector_render_geometry(vector);
                            let (stroke_area, affine) =
                                stroke_space.unwrap_or((area, Affine::IDENTITY));
                            return stroke_paths_hit(
                                &geometry.stroke_paths,
                                stroke,
                                stroke_area,
                                affine,
                            );
                        }
                        return false;
                    }
                    #[cfg(feature = "appearance-mask-eraser")]
                    if !appearance.erase_mask.is_empty() || !appearance.clip_mask.is_empty() {
                        return area.to_local(appearance.field_transform).is_some_and(
                            |canonical| {
                                cached_vector_appearance_body_geometry(
                                    self.textures,
                                    vector,
                                    appearance,
                                )
                                .is_some_and(|geometry| fill_hits(geometry, canonical))
                            },
                        );
                    }
                }
                let Some(bounds) = self.vector_bounds(vector) else {
                    return false;
                };
                let wide_stroke_may_hit = vector
                    .stroke
                    .zip(nonuniform_stroke_space(stroke_space))
                    .is_some_and(|(stroke, (parent_area, affine))| {
                        let padding =
                            stroke.width * affine.uniform_scale() * q0s_format::geom::MITER_LIMIT;
                        Area::rect(bounds)
                            .map(|point| Some(affine.apply(point)))
                            .is_some_and(|body| {
                                body.bounds.expand(padding).intersects(parent_area.bounds)
                            })
                    });
                if !area.bounds.intersects(as_rect(bounds)) && !wide_stroke_may_hit {
                    return false;
                }
                let geometry = self.textures.vector_render_geometry(vector);
                vector_hits(vector, geometry, area, stroke_space)
            }
            Asset::Bitmap(bitmap) => {
                let width = i32::from(bitmap.width);
                let height = i32::from(bitmap.height);
                area.pixels_hit(0, 0, width, height, |x, y| {
                    let i = (y as usize * width as usize + x as usize) * 4 + 3;
                    bitmap.rgba.get(i).is_some_and(|&alpha| alpha != 0)
                })
            }
            Asset::Raster(raster) => {
                use q0s_format::pixels::TILE_I32;
                let first_col = (area.bounds.min.x.floor() as i32).div_euclid(TILE_I32);
                let last_col = (area.bounds.max.x.ceil() as i32).div_euclid(TILE_I32);
                if first_col > i32::from(i16::MAX) || last_col < i32::from(i16::MIN) {
                    return false;
                }
                let first_col = first_col.max(i32::from(i16::MIN)) as i16;
                let last_col = last_col.min(i32::from(i16::MAX)) as i16;
                raster
                    .tiles
                    .range((first_col, i16::MIN)..=(last_col, i16::MAX))
                    .any(|(&(tx, ty), tile)| {
                        let x = i32::from(tx) * TILE_I32;
                        let y = i32::from(ty) * TILE_I32;
                        area.pixels_hit(x, y, x + TILE_I32, y + TILE_I32, |px, py| {
                            tile.pixel((px - x) as usize, (py - y) as usize)[3] != 0
                        })
                    })
            }
            Asset::Q0v(video) => q0video::q0v::probe_header(&video.bytes).is_ok_and(|header| {
                header.spec.video
                    && area.overlaps_rect(Rect::from_min_max(
                        Pos2::ZERO,
                        pos2(header.spec.width as f32, header.spec.height as f32),
                    ))
            }),
            Asset::Model(model) => model.flat_bounds().is_some_and(|(min, max)| {
                area.overlaps_rect(as_rect((min.x, min.y, max.x, max.y)))
            }),
            Asset::Rig(_) => false,
        }
    }

    fn vector_bounds(&mut self, vector: &VectorAsset) -> Option<Bounds> {
        *self
            .textures
            .vector_bounds_by_asset
            .entry(vector.asset_id)
            .or_insert_with(|| vector_conservative_local_bounds(vector))
    }
}

fn fill_hits(geometry: &CachedVectorRenderGeometry, area: Area) -> bool {
    geometry
        .fill_bounds
        .is_some_and(|bounds| area.bounds.intersects(bounds))
        && area.triangles_hit(&geometry.fill_indices, |index| {
            let p = geometry.fill_vertices[index];
            pos2(p.x, p.y)
        })
}

fn vector_hits(
    vector: &VectorAsset,
    geometry: &CachedVectorRenderGeometry,
    area: Area,
    stroke_space: Option<(Area, Affine)>,
) -> bool {
    (vector.fill.is_some_and(|fill| fill.a != 0) && fill_hits(geometry, area))
        || vector.stroke.is_some_and(|stroke| {
            stroke.color.a != 0
                && match nonuniform_stroke_space(stroke_space) {
                    Some((parent, affine)) => {
                        stroke_paths_hit(&geometry.stroke_paths, stroke, parent, affine)
                    }
                    None => area.triangles_hit(&geometry.stroke_indices, |index| {
                        let p = geometry.stroke_vertices[index];
                        pos2(p.x, p.y)
                    }),
                }
        })
}

fn morphed_shape_hits(
    shape: &ShapeTweenVector,
    area: Area,
    stroke_space: Option<(Area, Affine)>,
) -> bool {
    let mesh_hits = |fill: &CachedVectorGroupFill| {
        area.bounds.intersects(fill.bounds) && area.mesh_hits(&fill.mesh)
    };
    shape.fill_mesh.as_ref().is_some_and(mesh_hits)
        || shape
            .stroke
            .filter(|stroke| stroke.color.a != 0)
            .is_some_and(|stroke| match nonuniform_stroke_space(stroke_space) {
                Some((parent, affine)) => {
                    stroke_paths_hit(&shape.stroke_paths, stroke, parent, affine)
                }
                None => shape.stroke_mesh.as_ref().is_some_and(mesh_hits),
            })
}

fn nonuniform_stroke_space(space: Option<(Area, Affine)>) -> Option<(Area, Affine)> {
    space.filter(|&(_, affine)| !affine_is_similarity(affine))
}

fn stroke_paths_hit(
    paths: &[(Vec<Vec2>, bool)],
    stroke: VectorStroke,
    area: Area,
    affine: Affine,
) -> bool {
    let width = stroke.width * affine.uniform_scale();
    paths.iter().any(|(path, closed)| {
        let points: Vec<_> = path
            .iter()
            .map(|&p| {
                let p = affine.apply(p);
                pos2(p.x, p.y)
            })
            .collect();
        Rect::from_points(&points)
            .expand(width * q0s_format::geom::MITER_LIMIT)
            .intersects(area.bounds)
            && styled_stroke_mesh(
                &points,
                width,
                *closed,
                Color32::WHITE,
                stroke.cap,
                stroke.join,
            )
            .is_some_and(|mesh| area.mesh_hits(&mesh))
    })
}

#[derive(Clone, Copy)]
struct Area<'a> {
    corners: [Pos2; 4],
    bounds: Rect,
    to_reference: Affine,
    clip: Option<&'a [ClipPolygon]>,
    capture: Option<&'a RefCell<Vec<ClipPolygon>>>,
}

impl<'a> Area<'a> {
    fn rect(bounds: Bounds) -> Self {
        Self::from_rect(as_rect(bounds))
    }

    fn from_rect(bounds: Rect) -> Self {
        let corners = [
            bounds.left_top(),
            bounds.right_top(),
            bounds.right_bottom(),
            bounds.left_bottom(),
        ];
        Self {
            corners,
            bounds,
            to_reference: Affine::IDENTITY,
            clip: None,
            capture: None,
        }
    }

    fn map(self, mut map: impl FnMut(Vec2) -> Option<Vec2>) -> Option<Self> {
        let mut corners = self.corners;
        for corner in &mut corners {
            let p = map(Vec2::new(corner.x, corner.y))?;
            if !p.x.is_finite() || !p.y.is_finite() {
                return None;
            }
            *corner = pos2(p.x, p.y);
        }
        Some(Self {
            corners,
            bounds: Rect::from_points(&corners),
            ..self
        })
    }

    fn to_local(self, affine: Affine) -> Option<Self> {
        let inverse = affine.inverse()?;
        let mut area = self.map(|point| Some(inverse.apply(point)))?;
        area.to_reference = Affine::compose(self.to_reference, affine);
        Some(area)
    }

    fn with_mask<'b>(
        self,
        clip: Option<&'b [ClipPolygon]>,
        capture: Option<&'b RefCell<Vec<ClipPolygon>>>,
    ) -> Area<'b> {
        Area {
            corners: self.corners,
            bounds: self.bounds,
            to_reference: self.to_reference,
            clip,
            capture,
        }
    }

    fn overlaps_rect(self, rect: Rect) -> bool {
        self.bounds.intersects(rect) && self.overlaps(&Self::from_rect(rect).corners)
    }

    fn overlaps(self, polygon: &[Pos2]) -> bool {
        if separated(&self.corners, polygon) || separated(polygon, &self.corners) {
            return false;
        }
        let to_reference = |point: Pos2| {
            let point = self.to_reference.apply(Vec2::new(point.x, point.y));
            pos2(point.x, point.y)
        };
        if let Some(pieces) = self.clip {
            // All callers pass a triangle or a quad. Keep the common masked
            // test on the stack, even for densely tessellated sprites.
            let mut points = [Pos2::ZERO; 4];
            for (dest, &source) in points.iter_mut().zip(polygon) {
                *dest = to_reference(source);
            }
            let points = &points[..polygon.len()];
            let bounds = Rect::from_points(points);
            let matching = pieces
                .iter()
                .filter(|piece| piece.bounds.intersects(bounds));
            if let Some(output) = self.capture {
                let mut output = output.borrow_mut();
                output.extend(
                    matching.filter_map(|piece| ClipPolygon::intersect(points, &piece.points)),
                );
                return false;
            }
            return matching.into_iter().any(|piece| {
                !separated(points, &piece.points) && !separated(&piece.points, points)
            });
        }
        if let Some(output) = self.capture {
            if let Some(mut piece) = ClipPolygon::intersect(polygon, &self.corners) {
                for point in &mut piece.points {
                    *point = to_reference(*point);
                }
                piece.bounds = Rect::from_points(&piece.points);
                output.borrow_mut().push(piece);
            }
            return false;
        }
        true
    }

    fn mesh_hits(self, mesh: &Mesh) -> bool {
        self.triangles_hit(&mesh.indices, |index| mesh.vertices[index].pos)
    }

    fn triangles_hit(self, indices: &[u32], mut vertex: impl FnMut(usize) -> Pos2) -> bool {
        indices.as_chunks::<3>().0.iter().any(|triangle| {
            let points = [
                vertex(triangle[0] as usize),
                vertex(triangle[1] as usize),
                vertex(triangle[2] as usize),
            ];
            self.bounds.intersects(Rect::from_points(&points)) && self.overlaps(&points)
        })
    }

    fn pixels_hit(
        self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        mut opaque: impl FnMut(i32, i32) -> bool,
    ) -> bool {
        let xmin = x0.max(self.bounds.min.x.floor() as i32);
        let xmax = x1.min(self.bounds.max.x.ceil() as i32);
        let ymin = y0.max(self.bounds.min.y.floor() as i32);
        let ymax = y1.min(self.bounds.max.y.ceil() as i32);
        (ymin..ymax).any(|y| {
            (xmin..xmax).any(|x| {
                opaque(x, y)
                    && self.overlaps_rect(as_rect((
                        x as f32,
                        y as f32,
                        (x + 1) as f32,
                        (y + 1) as f32,
                    )))
            })
        })
    }
}

struct ClipPolygon {
    points: Vec<Pos2>,
    bounds: Rect,
}

impl ClipPolygon {
    fn map(&self, mut map: impl FnMut(Vec2) -> Option<Vec2>) -> Option<Self> {
        let points = self
            .points
            .iter()
            .map(|p| {
                let p = map(Vec2::new(p.x, p.y))?;
                Some(pos2(p.x, p.y))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            bounds: Rect::from_points(&points),
            points,
        })
    }

    fn intersect(subject: &[Pos2], clip: &[Pos2]) -> Option<Self> {
        let orientation = polygon_area_twice(clip).signum();
        let mut points = subject.to_vec();
        let mut next = Vec::with_capacity(subject.len() + clip.len());
        for (&start, &end) in clip.iter().zip(clip.iter().cycle().skip(1)) {
            next.clear();
            let normal = (end - start).rot90() * -orientation;
            let mut prev = *points.last()?;
            let mut distance = (prev - start).dot(normal);
            for &point in &points {
                let new_distance = (point - start).dot(normal);
                if (distance < 0.0) != (new_distance < 0.0) {
                    next.push(prev + (point - prev) * (distance / (distance - new_distance)));
                }
                if new_distance >= 0.0 {
                    next.push(point);
                }
                prev = point;
                distance = new_distance;
            }
            std::mem::swap(&mut points, &mut next);
        }
        (points.len() >= 3 && polygon_area_twice(&points).abs() > 0.0).then(|| Self {
            bounds: Rect::from_points(&points),
            points,
        })
    }
}

fn polygon_area_twice(points: &[Pos2]) -> f32 {
    points[1..]
        .windows(2)
        .map(|pair| {
            let a = pair[0] - points[0];
            let b = pair[1] - points[0];
            a.x * b.y - a.y * b.x
        })
        .sum()
}

fn as_rect((x0, y0, x1, y1): Bounds) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

fn separated(a: &[Pos2], b: &[Pos2]) -> bool {
    a.iter().zip(a.iter().cycle().skip(1)).any(|(&a0, &a1)| {
        let normal = (a1 - a0).rot90();
        let interval = |points: &[Pos2]| {
            points
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &point| {
                    let position = (point - a0).dot(normal);
                    (lo.min(position), hi.max(position))
                })
        };
        let (a_min, a_max) = interval(a);
        let (b_min, b_max) = interval(b);
        a_max < b_min || b_max < a_min
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contour(points: &[(f32, f32)]) -> VPath {
        VPath {
            anchors: points
                .iter()
                .map(|&(x, y)| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed: true,
        }
    }

    fn placement(target: Target, frame: u16) -> Placement {
        Placement {
            instance_id: 0,
            target,
            frame,
            transform: Transform2D::IDENTITY,
            pivot: None,
            fx: PlacementFx::default(),
            tween: Tween::None,
        }
    }

    fn vector(asset_id: u16, paths: Vec<VPath>) -> VectorAsset {
        VectorAsset {
            asset_id,
            paths,
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        }
    }

    #[test]
    fn marquee_uses_painted_geometry_in_the_active_symbol_frame() {
        let mut project = crate::state::default_project();
        let triangle = vector(10, vec![contour(&[(0., 0.), (100., 0.), (0., 100.)])]);
        let ring = vector(
            11,
            vec![
                contour(&[(200., 0.), (300., 0.), (300., 100.), (200., 100.)]),
                contour(&[(220., 20.), (220., 80.), (280., 80.), (280., 20.)]),
            ],
        );
        let mut textures = TextureCache::default();
        textures.vector_render_geometry(&triangle);
        textures.vector_render_geometry(&ring);
        project
            .assets
            .extend([Asset::Vector(triangle), Asset::Vector(ring)]);
        let mut symbol = project.q0rgs[0].clone();
        symbol.q0rg_id = 2;
        symbol.frame_count = 2;
        symbol.layers[0].placements = vec![
            placement(Target::Asset(10), 0),
            placement(Target::Asset(11), 1),
        ];
        project.q0rgs[0].layers[0].placements = vec![placement(Target::Q0rg(2), 0)];
        project.q0rgs.push(symbol);
        let mut hit = |frame, bounds| {
            !marquee_candidates(&project, 1, frame, bounds, &mut textures, Some((1, 1)))
                .objects
                .is_empty()
        };

        assert!(!hit(0, (80., 80., 90., 90.)), "empty triangle bbox corner");
        assert!(hit(0, (10., 10., 20., 20.)));
        assert!(
            !hit(1, (10., 10., 20., 20.)),
            "triangle is on a different frame"
        );
        assert!(!hit(1, (230., 30., 250., 50.)), "hole in the ring");
        assert!(hit(1, (205., 30., 215., 50.)));
        assert_eq!(textures.vector_render_geometry_build_count(), 2);
    }

    #[test]
    fn marquee_ignores_transparent_bitmap_pixels() {
        let mut project = crate::state::default_project();
        let mut rgba = vec![0; 4 * 2 * 2];
        rgba[3] = 255;
        project
            .assets
            .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                asset_id: 12,
                width: 2,
                height: 2,
                rgba: Arc::new(rgba),
            }));
        project.q0rgs[0].layers[0]
            .placements
            .push(placement(Target::Asset(12), 0));
        let mut textures = TextureCache::default();
        let mut hit = |bounds| {
            !marquee_candidates(&project, 1, 0, bounds, &mut textures, Some((1, 1)))
                .objects
                .is_empty()
        };
        assert!(hit((0.1, 0.1, 0.9, 0.9)));
        assert!(!hit((1.1, 1.1, 1.9, 1.9)));
    }

    #[test]
    fn marquee_checks_the_shared_area_of_mask_and_artwork() {
        let mut project = crate::state::default_project();
        let artwork = vector(
            10,
            vec![contour(&[(60., 20.), (95., 20.), (95., 85.), (60., 85.)])],
        );
        let mask = vector(11, vec![contour(&[(0., 0.), (100., 0.), (0., 100.)])]);
        project
            .assets
            .extend([Asset::Vector(artwork), Asset::Vector(mask)]);
        let mut symbol = project.q0rgs[0].clone();
        symbol.q0rg_id = 2;
        symbol.layers[0].placements = vec![placement(Target::Asset(10), 0)];
        symbol.layers.push(Layer {
            layer_id: 2,
            explicit_keyframes: vec![0, 1],
            placements: vec![placement(Target::Asset(11), 0)],
            ..symbol.layers[0].clone()
        });
        project.layer_metadata.insert(
            LayerKey {
                q0rg_id: 2,
                layer_id: 1,
            },
            LayerMetadata {
                mask_layer_id: Some(2),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey {
                q0rg_id: 2,
                layer_id: 2,
            },
            LayerMetadata {
                kind: q0s_format::v2::LayerKind::Mask,
                locked: true,
                ..Default::default()
            },
        );
        let mut instance = placement(Target::Q0rg(2), 0);
        instance.transform.tx = 100.;
        project.q0rgs[0].layers[0].placements.push(instance);
        project.q0rgs.push(symbol);
        let mut textures = TextureCache::default();
        let revision = (1, 1);
        let visible = (162., 22., 168., 27.);
        let masked = (145., 45., 195., 80.);
        assert!(
            !marquee_candidates(&project, 1, 0, visible, &mut textures, Some(revision))
                .objects
                .is_empty()
        );
        assert!(
            marquee_candidates(&project, 1, 0, masked, &mut textures, Some(revision))
                .objects
                .is_empty()
        );
        let reference = PlacementRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        assert_eq!(
            frame_region_hits(
                &project,
                1,
                &[(reference, 0), (reference, 1)],
                masked,
                &mut textures,
                revision
            ),
            [false, true],
            "a blank mask keyframe disables clipping at its own sampled frame",
        );
    }
}
