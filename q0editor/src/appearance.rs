use std::collections::BTreeSet;

use geo::{BooleanOps, BoundingRect, Buffer, MultiPolygon};
use q0s_format::v2::{
    Asset, Path as VPath, ProjectV2, Rgba, Target, Transform2D, Tween, VectorAppearance,
    VectorAsset, VectorMaterial,
};

use crate::{app::EditorApp, brush::BrushSettings};

pub const DEFAULT_HALO_RADIUS: f32 = 10.0;
pub const DEFAULT_HALO_OPACITY: f32 = 0.55;

pub fn default_brush_appearance() -> VectorAppearance {
    VectorAppearance {
        material: VectorMaterial::SoftHalo {
            radius: DEFAULT_HALO_RADIUS,
            opacity: DEFAULT_HALO_OPACITY,
        },
        erase_mask: Vec::new(),
    }
}

/// Classic brush paint is plain vector fill. The material exists only when
/// Glow is explicitly enabled in brush properties.
pub(crate) fn brush_material(settings: BrushSettings) -> Option<VectorMaterial> {
    settings.glow.then_some(default_brush_appearance().material)
}

/// Build the appearance for the single merged raw-fill asset produced by a
/// classic brush commit. Existing mask cuts follow same-colour paint through
/// merge drawing, while the freshly painted region clears those cuts so a new
/// stroke can genuinely paint back over an erased area.
pub(crate) fn merged_brush_appearance(
    project: &ProjectV2,
    same_color_asset_ids: &BTreeSet<u16>,
    freshly_painted: &MultiPolygon<f64>,
    material: VectorMaterial,
) -> VectorAppearance {
    let mut mask = MultiPolygon(Vec::new());
    for asset_id in same_color_asset_ids {
        let Some(appearance) = project.asset_appearances.get(asset_id) else {
            continue;
        };
        let coverage = mask_paths_to_coverage(&appearance.erase_mask);
        if !coverage.0.is_empty() {
            mask = mask.union(&coverage);
        }
    }
    if !freshly_painted.0.is_empty() && !mask.0.is_empty() {
        mask = mask.difference(freshly_painted);
    }
    VectorAppearance {
        material,
        erase_mask: crate::brush::coverage_to_paths(&mask),
    }
}

pub(crate) fn clone_asset_appearance(project: &mut ProjectV2, source: u16, destination: u16) {
    if let Some(appearance) = project.asset_appearances.get(&source).cloned() {
        project.asset_appearances.insert(destination, appearance);
    }
}

pub(crate) fn split_asset_appearance(
    project: &mut ProjectV2,
    source_asset_id: u16,
    selected_asset_id: u16,
) {
    let Some(original) = project.asset_appearances.get(&source_asset_id).cloned() else {
        return;
    };
    let old_mask = mask_paths_to_coverage(&original.erase_mask);
    let source_support = project
        .assets
        .iter()
        .find(|asset| asset.id() == source_asset_id)
        .and_then(|asset| match asset {
            Asset::Vector(vector) if vector.fill.is_some() && vector.stroke.is_none() => {
                Some(material_support(
                    &crate::brush::vector_fill_geometry(vector),
                    original.material,
                ))
            }
            _ => None,
        });
    let selected_support = project
        .assets
        .iter()
        .find(|asset| asset.id() == selected_asset_id)
        .and_then(|asset| match asset {
            Asset::Vector(vector) if vector.fill.is_some() && vector.stroke.is_none() => {
                Some(material_support(
                    &crate::brush::vector_fill_geometry(vector),
                    original.material,
                ))
            }
            _ => None,
        });

    if let Some(selected_support) = selected_support {
        let selected_mask = if old_mask.0.is_empty() {
            MultiPolygon(Vec::new())
        } else {
            old_mask.intersection(&selected_support)
        };
        project.asset_appearances.insert(
            selected_asset_id,
            VectorAppearance {
                material: original.material,
                erase_mask: crate::brush::coverage_to_paths(&selected_mask),
            },
        );
    }
    if let Some(source_support) = source_support {
        let source_mask = if old_mask.0.is_empty() {
            MultiPolygon(Vec::new())
        } else {
            old_mask.intersection(&source_support)
        };
        project.asset_appearances.insert(
            source_asset_id,
            VectorAppearance {
                material: original.material,
                erase_mask: crate::brush::coverage_to_paths(&source_mask),
            },
        );
    }
}

pub(crate) fn mask_paths_to_coverage(paths: &[VPath]) -> MultiPolygon<f64> {
    if paths.is_empty() {
        return MultiPolygon(Vec::new());
    }
    crate::brush::vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: paths.to_vec(),
        fill: Some(Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }),
        stroke: None,
    })
}

pub(crate) fn material_support(
    source: &MultiPolygon<f64>,
    material: VectorMaterial,
) -> MultiPolygon<f64> {
    match material {
        VectorMaterial::Solid => source.clone(),
        VectorMaterial::SoftHalo { radius, .. } => source.buffer(f64::from(radius.max(0.0))),
    }
}

/// Opaque/vector body that is still visible after the non-destructive erase
/// mask. This is what the editor tessellates as real vector geometry.
pub(crate) fn visible_source_surface_for_vector(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
) -> MultiPolygon<f64> {
    let source = crate::brush::vector_fill_geometry(vector);
    let Some(appearance) = appearance else {
        return source;
    };
    let mask = mask_paths_to_coverage(&appearance.erase_mask);
    if mask.0.is_empty() {
        source
    } else {
        source.difference(&mask)
    }
}

/// Full selectable visual support: vector body plus the finite halo support,
/// with erased areas removed. This intentionally uses the material's finite
/// support radius rather than the source path bbox so glow is real graphics to
/// hit-testing, selection and transform bounds.
pub(crate) fn visible_material_surface_for_vector(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
) -> MultiPolygon<f64> {
    let source = crate::brush::vector_fill_geometry(vector);
    let Some(appearance) = appearance else {
        return source;
    };
    let support = material_support(&source, appearance.material);
    let mask = mask_paths_to_coverage(&appearance.erase_mask);
    if mask.0.is_empty() {
        support
    } else {
        support.difference(&mask)
    }
}

pub(crate) fn visible_material_surface_for_paths(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
    path_indices: &[usize],
) -> MultiPolygon<f64> {
    let subset = VectorAsset {
        asset_id: vector.asset_id,
        paths: path_indices
            .iter()
            .filter_map(|index| vector.paths.get(*index).cloned())
            .collect(),
        fill: vector.fill,
        stroke: None,
    };
    visible_material_surface_for_vector(&subset, appearance)
}

pub(crate) fn asset_visible_material_bounds(
    project: &ProjectV2,
    asset_id: u16,
) -> Option<(f32, f32, f32, f32)> {
    let Asset::Vector(vector) = project.assets.iter().find(|asset| asset.id() == asset_id)? else {
        return None;
    };
    if vector.fill.is_none() || vector.stroke.is_some() {
        return None;
    }
    let surface =
        visible_material_surface_for_vector(vector, project.asset_appearances.get(&asset_id));
    let bounds = surface.bounding_rect()?;
    Some((
        bounds.min().x as f32,
        bounds.min().y as f32,
        bounds.max().x as f32,
        bounds.max().y as f32,
    ))
}

/// Erase the resolved appearance, not the source vector. The exact eraser
/// footprint is intersected with the finite material support and then unioned
/// into the asset-local mask. No radius is added to the eraser region.
pub fn erase_visible_region(app: &mut EditorApp, region: MultiPolygon<f64>) -> bool {
    if region.0.is_empty() {
        return false;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };

    let mut hit_asset_ids = BTreeSet::new();
    for (placement_idx, _) in crate::render::active_placements_at(layer, frame) {
        let Some(placement) = layer.placements.get(placement_idx) else {
            continue;
        };
        if placement.transform != Transform2D::IDENTITY || !matches!(placement.tween, Tween::None) {
            continue;
        }
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(appearance) = app.state.project.asset_appearances.get(&asset_id) else {
            continue;
        };
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        if vector.fill.is_none() || vector.stroke.is_some() {
            continue;
        }
        let source = crate::brush::vector_fill_geometry(vector);
        if source.0.is_empty() {
            continue;
        }
        let support = material_support(&source, appearance.material);
        let old_mask = mask_paths_to_coverage(&appearance.erase_mask);
        let visible_support = if old_mask.0.is_empty() {
            support
        } else {
            support.difference(&old_mask)
        };
        if !visible_support.intersection(&region).0.is_empty() {
            hit_asset_ids.insert(asset_id);
        }
    }
    if hit_asset_ids.is_empty() {
        return false;
    }

    app.history.snapshot(&app.state.project);
    if crate::tools::materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
    )
    .is_none()
    {
        return false;
    }
    let writable = crate::brush::prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &hit_asset_ids,
    );

    let mut changed = false;
    for original_asset_id in hit_asset_ids {
        let asset_id = writable
            .get(&original_asset_id)
            .copied()
            .unwrap_or(original_asset_id);
        let Some(appearance) = app.state.project.asset_appearances.get(&asset_id).cloned() else {
            continue;
        };
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let source = crate::brush::vector_fill_geometry(vector);
        let support = material_support(&source, appearance.material);
        let visible_cut = support.intersection(&region);
        if visible_cut.0.is_empty() {
            continue;
        }
        let old_mask = mask_paths_to_coverage(&appearance.erase_mask);
        let new_mask = if old_mask.0.is_empty() {
            visible_cut
        } else {
            old_mask.union(&visible_cut)
        };
        let new_paths = crate::brush::coverage_to_paths(&new_mask);
        if new_paths != appearance.erase_mask {
            if let Some(entry) = app.state.project.asset_appearances.get_mut(&asset_id) {
                entry.erase_mask = new_paths;
                changed = true;
            }
        }
    }

    if changed {
        app.textures.invalidate();
        app.session.selection = crate::state::Selection::None;
        app.session.status = "Appearance mask erased".to_string();
        app.state.dirty = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use geo::{Area, BoundingRect, Coord, LineString, Polygon};
    use q0s_format::v2::{Anchor, Placement};

    use super::*;

    fn square_path(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> VPath {
        VPath {
            anchors: [
                (min_x, min_y),
                (max_x, min_y),
                (max_x, max_y),
                (min_x, max_y),
            ]
            .into_iter()
            .map(|(x, y)| Anchor {
                point: q0s_format::v2::Vec2::new(x, y),
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        }
    }

    fn rect_region(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> MultiPolygon<f64> {
        MultiPolygon(vec![Polygon::new(
            LineString::new(vec![
                Coord { x: min_x, y: min_y },
                Coord { x: max_x, y: min_y },
                Coord { x: max_x, y: max_y },
                Coord { x: min_x, y: max_y },
                Coord { x: min_x, y: min_y },
            ]),
            Vec::new(),
        )])
    }

    fn app_with_appearance() -> EditorApp {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square_path(0.0, 0.0, 10.0, 10.0)],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.asset_appearances.insert(
            1,
            VectorAppearance {
                material: VectorMaterial::SoftHalo {
                    radius: 10.0,
                    opacity: 0.5,
                },
                erase_mask: Vec::new(),
            },
        );
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });
        app
    }

    #[test]
    fn mask_eraser_never_mutates_source_geometry() {
        let mut app = app_with_appearance();
        let before = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let eraser = rect_region(15.0, 4.0, 17.0, 6.0);
        assert!(erase_visible_region(&mut app, eraser));
        let after = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            before, after,
            "appearance erasing must not cut source paths"
        );
    }

    #[test]
    fn classic_geometry_eraser_refuses_to_cut_appearance_source_paths() {
        let mut app = app_with_appearance();
        let before = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let eraser = rect_region(2.0, 2.0, 8.0, 8.0);
        assert!(
            !crate::brush::erase_brush_region(&mut app, eraser),
            "appearance assets must be owned exclusively by the mask eraser"
        );
        let after = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        assert_eq!(before, after);
    }

    #[test]
    fn mask_eraser_records_exact_footprint_not_halo_expansion() {
        let mut app = app_with_appearance();
        let eraser = rect_region(15.0, 4.0, 17.0, 6.0);
        let eraser_area = eraser.unsigned_area();
        assert!(erase_visible_region(&mut app, eraser));
        let mask = mask_paths_to_coverage(&app.state.project.asset_appearances[&1].erase_mask);
        let bounds = mask.bounding_rect().expect("mask bounds");
        assert!((bounds.min().x - 15.0).abs() < 0.05);
        assert!((bounds.max().x - 17.0).abs() < 0.05);
        assert!((bounds.min().y - 4.0).abs() < 0.05);
        assert!((bounds.max().y - 6.0).abs() < 0.05);
        assert!((mask.unsigned_area() - eraser_area).abs() < 0.1);
    }
}
