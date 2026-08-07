use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use egui::{Color32, Painter, Pos2};
use geo::{Area, BooleanOps, Buffer, Contains, Coord, LineString, MultiPolygon, Point, Polygon};
use q0s_format::v2::{
    Asset, Path as VPath, ProjectV2, Rgba, Target, Transform2D, Tween, VectorAsset,
};

use crate::app::EditorApp;
use crate::render::{active_placements_at, StageView};

const HALO_STEPS: usize = 8;
const GEOMETRY_EPSILON: f64 = 1.0e-8;

/// Experimental live appearance attached to a raw vector surface.
///
/// This deliberately does not enter q0s-format yet. The classic project data
/// stays an ordinary fill-only VectorAsset, so disabling the cargo feature or
/// returning to main gives the exact legacy behaviour and file format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrushMaterial {
    Solid,
    /// Finite-support soft halo around the vector source. `radius` is in stage
    /// units and is also the support radius used by the appearance-aware eraser.
    SoftHalo {
        radius: f32,
        opacity: f32,
    },
}

impl BrushMaterial {
    pub fn support_radius(self) -> f32 {
        match self {
            Self::Solid => 0.0,
            Self::SoftHalo { radius, .. } => radius.max(0.0),
        }
    }
}

#[derive(Debug)]
struct AppearanceRegistry {
    by_asset: HashMap<u16, BrushMaterial>,
    current: BrushMaterial,
}

impl Default for AppearanceRegistry {
    fn default() -> Self {
        Self {
            by_asset: HashMap::new(),
            // The experiment branch intentionally starts visibly enabled. The
            // normal branch does not compile this module at all.
            current: BrushMaterial::SoftHalo {
                radius: 10.0,
                opacity: 0.55,
            },
        }
    }
}

fn registry() -> &'static Mutex<AppearanceRegistry> {
    static REGISTRY: OnceLock<Mutex<AppearanceRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(AppearanceRegistry::default()))
}

pub fn current_material() -> BrushMaterial {
    registry()
        .lock()
        .map(|state| state.current)
        .unwrap_or(BrushMaterial::Solid)
}

pub fn material_for_asset(asset_id: u16) -> BrushMaterial {
    registry()
        .lock()
        .ok()
        .and_then(|state| state.by_asset.get(&asset_id).copied())
        .unwrap_or(BrushMaterial::Solid)
}

fn remember_material(asset_id: u16, material: BrushMaterial) {
    if let Ok(mut state) = registry().lock() {
        match material {
            BrushMaterial::Solid => {
                state.by_asset.remove(&asset_id);
            }
            _ => {
                state.by_asset.insert(asset_id, material);
            }
        }
    }
}

fn clone_material_mapping(source_asset_id: u16, target_asset_id: u16) {
    let material = material_for_asset(source_asset_id);
    remember_material(target_asset_id, material);
}

/// After the legacy classic-brush commit, attach the currently selected live
/// material to the resulting same-colour raw paint surface. Classic drawing
/// already merges same-colour paint into one planar asset, so tagging the active
/// surface here keeps the experiment isolated from the trusted commit path.
pub fn tag_current_brush_surface(app: &EditorApp) {
    let material = current_material();
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let color = app.session.brush.color;

    let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return;
    };

    for (placement_idx, _) in active_placements_at(layer, frame) {
        let Some(placement) = layer.placements.get(placement_idx) else {
            continue;
        };
        if placement.transform != Transform2D::IDENTITY || !matches!(placement.tween, Tween::None) {
            continue;
        }
        let Target::Asset(asset_id) = placement.target else {
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
        if vector.fill == Some(color) && vector.stroke.is_none() {
            remember_material(asset_id, material);
        }
    }
}

/// Convert the visible eraser footprint into source geometry that must be
/// removed for the selected finite-support material.
///
/// For a solid fill this is exactly the legacy eraser. For a halo with support
/// radius R, every source point within R of the visible erase region can
/// contribute pixels back into that region, so it must also be removed. This is
/// the Minkowski sum `erase ⊕ disk(R)`.
pub fn source_cut_for_visible_erase(
    visible_erase: &MultiPolygon<f64>,
    material: BrushMaterial,
) -> MultiPolygon<f64> {
    let radius = f64::from(material.support_radius());
    if radius <= GEOMETRY_EPSILON {
        visible_erase.clone()
    } else {
        visible_erase.buffer(radius)
    }
}

fn visible_support(geometry: &MultiPolygon<f64>, material: BrushMaterial) -> MultiPolygon<f64> {
    let radius = f64::from(material.support_radius());
    if radius <= GEOMETRY_EPSILON {
        geometry.clone()
    } else {
        geometry.buffer(radius)
    }
}

#[derive(Debug, Clone)]
struct RawAppearanceCandidate {
    asset_id: u16,
    geometry: MultiPolygon<f64>,
    material: BrushMaterial,
}

fn collect_raw_candidates(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Vec<RawAppearanceCandidate> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };

    active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(placement_idx, _)| {
            let placement = layer.placements.get(placement_idx)?;
            if placement.transform != Transform2D::IDENTITY
                || !matches!(placement.tween, Tween::None)
            {
                return None;
            }
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                return None;
            };
            if vector.fill.is_none() || vector.stroke.is_some() {
                return None;
            }
            let geometry = vector_fill_geometry(vector);
            (!geometry.0.is_empty()).then_some(RawAppearanceCandidate {
                asset_id,
                geometry,
                material: material_for_asset(asset_id),
            })
        })
        .collect()
}

/// Erase the visible appearance rather than only the source contour.
///
/// Each raw asset gets its own cut radius. This is important when a solid fill
/// sits next to a halo fill: using one global maximum radius would over-erase
/// the solid neighbour. Returns false when no visible raw appearance was hit so
/// the legacy eraser can retain its open-stroke/display-object fallback.
pub fn erase_visible_region(app: &mut EditorApp, visible_erase: MultiPolygon<f64>) -> bool {
    if visible_erase.0.is_empty() {
        return false;
    }

    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let candidates = collect_raw_candidates(&app.state.project, q0rg_id, layer_id, frame);

    let mut updates = Vec::new();
    let mut update_asset_ids = BTreeSet::new();
    for candidate in candidates {
        if !update_asset_ids.insert(candidate.asset_id) {
            continue;
        }
        let support = visible_support(&candidate.geometry, candidate.material);
        if support.intersection(&visible_erase).unsigned_area() <= GEOMETRY_EPSILON {
            update_asset_ids.remove(&candidate.asset_id);
            continue;
        }

        let source_cut = source_cut_for_visible_erase(&visible_erase, candidate.material);
        if candidate.geometry.intersection(&source_cut).unsigned_area() <= GEOMETRY_EPSILON {
            update_asset_ids.remove(&candidate.asset_id);
            continue;
        }
        updates.push((
            candidate.asset_id,
            candidate.geometry.difference(&source_cut),
        ));
    }
    if updates.is_empty() {
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

    let writable_assets = crate::brush::prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &update_asset_ids,
    );
    for (source, target) in &writable_assets {
        if source != target {
            clone_material_mapping(*source, *target);
        }
    }

    let mut remove_asset_ids = BTreeSet::new();
    for (original_asset_id, geometry) in &updates {
        let asset_id = *writable_assets
            .get(original_asset_id)
            .unwrap_or(original_asset_id);
        let paths = crate::brush::coverage_to_paths(geometry);
        if paths.is_empty() {
            remove_asset_ids.insert(asset_id);
        } else if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = paths;
            vector.stroke = None;
        }
    }

    remove_current_layer_asset_placements(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &remove_asset_ids,
    );
    remove_unreferenced_assets(&mut app.state.project, &remove_asset_ids);

    app.session.selection = crate::state::Selection::None;
    app.session.status = "Appearance-aware raw fill erased".to_string();
    app.state.dirty = true;
    true
}

fn remove_current_layer_asset_placements(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    asset_ids: &BTreeSet<u16>,
) {
    if asset_ids.is_empty() {
        return;
    }
    if let Some(layer) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
        })
    {
        layer.placements.retain(|placement| {
            placement.frame != frame
                || placement.transform != Transform2D::IDENTITY
                || !matches!(placement.tween, Tween::None)
                || !matches!(placement.target, Target::Asset(id) if asset_ids.contains(&id))
        });
    }
}

fn remove_unreferenced_assets(project: &mut ProjectV2, candidates: &BTreeSet<u16>) {
    if candidates.is_empty() {
        return;
    }
    let referenced: BTreeSet<u16> = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        })
        .collect();
    project
        .assets
        .retain(|asset| !candidates.contains(&asset.id()) || referenced.contains(&asset.id()));
}

/// Paint the experimental halo after the trusted ProjectV2 renderer. The base
/// fill is still rendered by the normal path; these are non-overlapping outer
/// bands only, which avoids repeatedly alpha-blending the same halo pixels.
pub fn render_registered_appearances(app: &EditorApp, painter: &Painter, view: &StageView) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(q0rg) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return;
    };

    for layer in &q0rg.layers {
        for (placement_idx, _) in active_placements_at(layer, frame) {
            let Some(placement) = layer.placements.get(placement_idx) else {
                continue;
            };
            if placement.transform != Transform2D::IDENTITY
                || !matches!(placement.tween, Tween::None)
            {
                continue;
            }
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let material = material_for_asset(asset_id);
            let BrushMaterial::SoftHalo { radius, opacity } = material else {
                continue;
            };
            if radius <= 0.0 || opacity <= 0.0 {
                continue;
            }
            let Some(Asset::Vector(vector)) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
            else {
                continue;
            };
            let Some(fill) = vector.fill else {
                continue;
            };
            let geometry = vector_fill_geometry(vector);
            if geometry.0.is_empty() {
                continue;
            }
            paint_soft_halo(painter, view, &geometry, fill, radius, opacity);
        }
    }
}

fn paint_soft_halo(
    painter: &Painter,
    view: &StageView,
    geometry: &MultiPolygon<f64>,
    fill: Rgba,
    radius: f32,
    opacity: f32,
) {
    let mut previous = geometry.clone();
    for step in 1..=HALO_STEPS {
        let t = step as f32 / HALO_STEPS as f32;
        let outer = geometry.buffer(f64::from(radius.max(0.0) * t));
        let band = outer.difference(&previous);
        previous = outer;
        if band.0.is_empty() {
            continue;
        }
        let falloff = (1.0 - (t - 0.5 / HALO_STEPS as f32)).clamp(0.0, 1.0);
        let alpha = (f32::from(fill.a) * opacity.clamp(0.0, 1.0) * falloff * falloff)
            .round()
            .clamp(0.0, 255.0) as u8;
        if alpha == 0 {
            continue;
        }
        let contours = multipolygon_to_screen_contours(&band, view);
        if contours.is_empty() {
            continue;
        }
        crate::render::paint_complex_fill(
            painter,
            &contours,
            Color32::from_rgba_unmultiplied(fill.r, fill.g, fill.b, alpha),
        );
    }
}

fn multipolygon_to_screen_contours(
    geometry: &MultiPolygon<f64>,
    view: &StageView,
) -> Vec<Vec<Pos2>> {
    let mut contours = Vec::new();
    for polygon in &geometry.0 {
        push_ring_screen(&mut contours, polygon.exterior(), view);
        for hole in polygon.interiors() {
            push_ring_screen(&mut contours, hole, view);
        }
    }
    contours
}

fn push_ring_screen(contours: &mut Vec<Vec<Pos2>>, ring: &LineString<f64>, view: &StageView) {
    let mut points: Vec<Pos2> = ring
        .0
        .iter()
        .map(|coord| {
            Pos2::new(
                view.origin.x + coord.x as f32 * view.scale,
                view.origin.y + coord.y as f32 * view.scale,
            )
        })
        .collect();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() >= 3 {
        contours.push(points);
    }
}

fn vector_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    let mut rings: Vec<(&VPath, Polygon<f64>, f64)> = vector
        .paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let polygon = path_to_polygon(path)?;
            let area = signed_ring_area(polygon.exterior());
            (area.abs() > GEOMETRY_EPSILON).then_some((path, polygon, area))
        })
        .collect();
    rings.sort_by(|left, right| right.2.abs().total_cmp(&left.2.abs()));

    let mut surface = MultiPolygon(Vec::new());
    let mut inside_windings: Vec<i32> = Vec::with_capacity(rings.len());
    for index in 0..rings.len() {
        let (_, polygon, area) = &rings[index];
        let sample = polygon
            .exterior()
            .0
            .first()
            .map(|coord| Point::new(coord.x, coord.y));
        let parent = sample.and_then(|point| {
            (0..index)
                .rev()
                .find(|candidate| rings[*candidate].1.contains(&point))
        });
        let outside_winding = parent.map(|parent| inside_windings[parent]).unwrap_or(0);
        let inside_winding = outside_winding + if *area > 0.0 { 1 } else { -1 };
        if outside_winding == 0 && inside_winding != 0 {
            surface = surface.union(polygon);
        } else if outside_winding != 0 && inside_winding == 0 {
            surface = surface.difference(polygon);
        }
        inside_windings.push(inside_winding);
    }
    surface
}

fn path_to_polygon(path: &VPath) -> Option<Polygon<f64>> {
    if path.anchors.len() < 3 {
        return None;
    }
    let mut coords: Vec<Coord<f64>> = path
        .anchors
        .iter()
        .map(|anchor| Coord {
            x: f64::from(anchor.point.x),
            y: f64::from(anchor.point.y),
        })
        .collect();
    coords.dedup();
    if coords.len() < 3 {
        return None;
    }
    if coords.first() != coords.last() {
        coords.push(coords[0]);
    }
    Some(Polygon::new(LineString::new(coords), Vec::new()))
}

fn signed_ring_area(ring: &LineString<f64>) -> f64 {
    ring.0
        .windows(2)
        .map(|pair| pair[0].x * pair[1].y - pair[1].x * pair[0].y)
        .sum::<f64>()
        * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> MultiPolygon<f64> {
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

    #[test]
    fn solid_visible_erase_is_exactly_the_legacy_source_cut() {
        let erase = rect(45.0, 20.0, 55.0, 80.0);
        let cut = source_cut_for_visible_erase(&erase, BrushMaterial::Solid);
        let mismatch = erase.difference(&cut).union(&cut.difference(&erase));
        assert!(mismatch.unsigned_area() <= GEOMETRY_EPSILON);
    }

    #[test]
    fn halo_source_cut_expands_by_the_material_support_radius() {
        let erase = rect(45.0, 20.0, 55.0, 80.0);
        let cut = source_cut_for_visible_erase(
            &erase,
            BrushMaterial::SoftHalo {
                radius: 10.0,
                opacity: 0.5,
            },
        );
        let bounds = geo::BoundingRect::bounding_rect(&cut).expect("buffered erase bounds");
        assert!((bounds.min().x - 35.0).abs() < 0.05);
        assert!((bounds.max().x - 65.0).abs() < 0.05);
        assert!((bounds.min().y - 10.0).abs() < 0.05);
        assert!((bounds.max().y - 90.0).abs() < 0.05);
    }

    #[test]
    fn reapplying_halo_cannot_bleed_back_into_the_visible_erase_region() {
        let source = rect(0.0, 0.0, 100.0, 100.0);
        let visible_erase = rect(45.0, -5.0, 55.0, 105.0);
        let material = BrushMaterial::SoftHalo {
            radius: 10.0,
            opacity: 0.5,
        };
        let source_cut = source_cut_for_visible_erase(&visible_erase, material);
        let remaining_source = source.difference(&source_cut);
        let rerendered_support = visible_support(&remaining_source, material);
        let bleed = rerendered_support
            .intersection(&visible_erase)
            .unsigned_area();
        assert!(
            bleed <= 1.0e-6,
            "halo bled {bleed} area back into erased pixels"
        );
    }

    #[test]
    fn halo_is_hittable_outside_the_source_geometry() {
        let source = rect(0.0, 0.0, 20.0, 20.0);
        let material = BrushMaterial::SoftHalo {
            radius: 8.0,
            opacity: 0.5,
        };
        let point = Point::new(25.0, 10.0);
        assert!(!source.contains(&point));
        assert!(visible_support(&source, material).contains(&point));
    }
}
