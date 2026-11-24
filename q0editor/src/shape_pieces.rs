//! A shape's fill and the outline round it go together.
//!
//! Merge drawing keeps a closed shape's fill and its outline as separate
//! pieces, the way Animate does, and lines drawn across a fill part it into
//! pieces. A double-click on a fill or its outline picks the fill and all
//! the lines along its edge, so the whole shape can move together.

use std::collections::BTreeSet;

use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, Path as VPath, ProjectV2, Target, Vec2, VectorAsset};

use crate::render::TextureCache;
use crate::state::{PathRef, StrokeSegmentRef};

/// What a click landed on.
pub(crate) enum Picked {
    /// A fill region: its outer path and any holes.
    Fill(Vec<PathRef>),
    /// A line, or a whole outline.
    Line(PathRef),
    /// One side of a filled shape's own outline.
    Side(StrokeSegmentRef),
}

/// The fill regions to pick, the lines and the filled shapes' own outline
/// sides that go with what was clicked.
pub(crate) struct Shape {
    pub fills: Vec<PathRef>,
    pub strokes: Vec<PathRef>,
    pub sides: Vec<StrokeSegmentRef>,
}

/// A fill region on the layer, and its edge in stage space.
struct Region {
    refs: Vec<PathRef>,
    /// Its own outline drawn round it in the same drawing.
    outlined: bool,
    rings: Vec<Vec<Vec2>>,
    bounds: Bounds,
}

/// A line on the layer, in stage space, and how near its middle the edge
/// of a fill must come for the line to run along it.
struct Line {
    reference: PathRef,
    points: Vec<Vec2>,
    bounds: Bounds,
    reach: f32,
}

/// min x, min y, max x, max y.
type Bounds = (f32, f32, f32, f32);

/// Lines that run along at least this share of themselves on a fill's
/// edge are its outline.
const ALONG: f32 = 0.6;

/// The whole shape a click on `picked` means, or None when the piece has
/// nothing round it or inside it.
pub(crate) fn shape_of(
    project: &ProjectV2,
    textures: &mut TextureCache,
    frame: u16,
    picked: &Picked,
) -> Option<Shape> {
    let start = match picked {
        Picked::Fill(refs) => *refs.first()?,
        Picked::Line(reference) => *reference,
        Picked::Side(side) => side.path,
    };
    let lines = lines_on(project, start.q0rg_id, start.layer_id, frame);
    let chosen: Vec<Region> = match picked {
        // A fill's regions are all in its own drawing.
        Picked::Fill(refs) => regions_on(project, textures, start, frame, |placement_idx, _| {
            placement_idx == start.placement_idx
        })
        .into_iter()
        .filter(|region| refs.iter().any(|reference| region.refs.contains(reference)))
        .collect(),
        Picked::Side(side) => regions_on(project, textures, start, frame, |placement_idx, _| {
            placement_idx == start.placement_idx
        })
        .into_iter()
        .filter(|region| region.refs.contains(&side.path))
        .collect(),
        Picked::Line(reference) => {
            let line = lines.iter().find(|line| line.reference == *reference)?;
            regions_on(project, textures, start, frame, |_, bounds| {
                overlaps(bounds, line.bounds, line.reach)
            })
            .into_iter()
            .filter(|region| runs_along(line, region))
            .collect()
        }
    };
    if chosen.is_empty() {
        return None;
    }
    let mut shape = Shape {
        fills: Vec::new(),
        strokes: Vec::new(),
        sides: Vec::new(),
    };
    // Sets keep a big pick from going quadratic.
    let mut fills = BTreeSet::new();
    let mut strokes = BTreeSet::new();
    let mut sides = BTreeSet::new();
    for region in &chosen {
        for reference in &region.refs {
            if fills.insert(*reference) {
                shape.fills.push(*reference);
            }
            if region.outlined {
                for segment_idx in 0..sides_of(project, *reference) {
                    if sides.insert((*reference, segment_idx)) {
                        shape.sides.push(StrokeSegmentRef {
                            path: *reference,
                            segment_idx,
                        });
                    }
                }
            }
        }
        for line in &lines {
            if !strokes.contains(&line.reference) && runs_along(line, region) {
                strokes.insert(line.reference);
                shape.strokes.push(line.reference);
            }
        }
    }
    if let Picked::Line(reference) = picked {
        if strokes.insert(*reference) {
            shape.strokes.push(*reference);
        }
    }
    // A fill on its own, with nothing round it, is just the fill.
    (!shape.strokes.is_empty() || !shape.sides.is_empty()).then_some(shape)
}

fn sides_of(project: &ProjectV2, reference: PathRef) -> usize {
    path_of(project, reference).map_or(0, |path| match path.anchors.len() {
        0 | 1 => 0,
        count if path.closed => count,
        count => count - 1,
    })
}

fn path_of(project: &ProjectV2, reference: PathRef) -> Option<&VPath> {
    let placement = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)?;
    let Target::Asset(asset_id) = placement.target else {
        return None;
    };
    match project.assets.iter().find(|asset| asset.id() == asset_id)? {
        Asset::Vector(vector) => vector.paths.get(reference.path_idx),
        _ => None,
    }
}

/// The drawings on a layer at a frame, with where each is placed.
fn drawings_on(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Vec<(usize, Affine, &VectorAsset)> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(placement_idx, transform)| {
            let Target::Asset(asset_id) = layer.placements.get(placement_idx)?.target else {
                return None;
            };
            match project.assets.iter().find(|a| a.id() == asset_id)? {
                Asset::Vector(vector) => {
                    Some((placement_idx, Affine::from_transform(transform), vector))
                }
                _ => None,
            }
        })
        .collect()
}

/// The lines on a layer at a frame.
fn lines_on(project: &ProjectV2, q0rg_id: u16, layer_id: u16, frame: u16) -> Vec<Line> {
    let mut lines = Vec::new();
    for (placement_idx, affine, vector) in drawings_on(project, q0rg_id, layer_id, frame) {
        let Some(stroke) = vector.stroke else {
            continue;
        };
        let reach = stroke.width * 0.5 * affine.uniform_scale() + 1.0;
        for (path_idx, path) in vector.paths.iter().enumerate() {
            // A filled shape's own outline goes with its fill already.
            if path.anchors.len() < 2 || (path.closed && vector.fill.is_some()) {
                continue;
            }
            let points: Vec<Vec2> = crate::render::flatten_path(path)
                .into_iter()
                .map(|point| affine.apply(point))
                .collect();
            lines.push(Line {
                reference: PathRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    path_idx,
                },
                bounds: bounds_of(points.iter()),
                points,
                reach,
            });
        }
    }
    lines
}

/// The fill regions of the drawings on `at`'s layer that `wanted` keeps,
/// given each drawing's placement index and its bounds on the stage.
fn regions_on(
    project: &ProjectV2,
    textures: &mut TextureCache,
    at: PathRef,
    frame: u16,
    wanted: impl Fn(usize, Bounds) -> bool,
) -> Vec<Region> {
    let mut regions = Vec::new();
    for (placement_idx, affine, vector) in drawings_on(project, at.q0rg_id, at.layer_id, frame) {
        if vector.fill.is_none() || !wanted(placement_idx, stage_bounds(vector, affine)) {
            continue;
        }
        let appearance = project.asset_appearances.get(&vector.asset_id);
        for (owners, rings) in fill_regions(vector, appearance, textures) {
            let rings: Vec<Vec<Vec2>> = rings
                .into_iter()
                .map(|ring| ring.into_iter().map(|point| affine.apply(point)).collect())
                .collect();
            regions.push(Region {
                refs: owners
                    .into_iter()
                    .map(|path_idx| PathRef {
                        q0rg_id: at.q0rg_id,
                        layer_id: at.layer_id,
                        placement_idx,
                        path_idx,
                    })
                    .collect(),
                outlined: vector.stroke.is_some(),
                bounds: bounds_of(rings.iter().flatten()),
                rings,
            });
        }
    }
    regions
}

/// A drawing's fill regions in its own space: the closed paths each is
/// made of, and its rings. These are the very regions a click on the fill
/// finds, kept by the stage until the drawing changes, so a big drawing's
/// regions are worked out once.
#[cfg(feature = "appearance-mask-eraser")]
fn fill_regions(
    vector: &VectorAsset,
    appearance: Option<&q0s_format::v2::VectorAppearance>,
    textures: &mut TextureCache,
) -> Vec<(Vec<usize>, Vec<Vec<Vec2>>)> {
    let field = appearance.map_or(Affine::IDENTITY, |appearance| appearance.field_transform);
    let Some(components) = textures.raw_selection_components(vector, appearance) else {
        return Vec::new();
    };
    if !components.masked_islands.is_empty() {
        // These islands share source paths. A line hit cannot select them
        // through those paths without picking the other islands too.
        return Vec::new();
    }
    components
        .components
        .iter()
        .map(|component| {
            let rings = component
                .canonical_surface
                .0
                .iter()
                .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
                .map(|ring| {
                    ring.coords()
                        .map(|c| field.apply(Vec2::new(c.x as f32, c.y as f32)))
                        .collect()
                })
                .collect();
            (component.path_indices.clone(), rings)
        })
        .collect()
}

#[cfg(not(feature = "appearance-mask-eraser"))]
fn fill_regions(
    vector: &VectorAsset,
    _appearance: Option<&q0s_format::v2::VectorAppearance>,
    _textures: &mut TextureCache,
) -> Vec<(Vec<usize>, Vec<Vec<Vec2>>)> {
    let topology = crate::brush::vector_fill_topology(vector);
    let known = topology
        .component_path_indices
        .filter(|owners| owners.len() == topology.surface.0.len());
    topology
        .surface
        .0
        .iter()
        .enumerate()
        .filter_map(|(index, polygon)| {
            let rings: Vec<Vec<Vec2>> = std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .map(|ring| {
                    ring.coords()
                        .map(|c| Vec2::new(c.x as f32, c.y as f32))
                        .collect()
                })
                .collect();
            let owners = match &known {
                Some(owners) => owners[index].clone(),
                // Crossing outlines were unioned: a region is made of the
                // closed paths along its edge.
                None => vector
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| {
                        path.closed
                            && path
                                .anchors
                                .iter()
                                .any(|anchor| distance_to_rings(anchor.point, &rings) <= 0.5)
                    })
                    .map(|(index, _)| index)
                    .collect(),
            };
            (!owners.is_empty()).then_some((owners, rings))
        })
        .collect()
}

/// Where a drawing can reach on the stage: its anchors and handles hold
/// every curve.
fn stage_bounds(vector: &VectorAsset, affine: Affine) -> Bounds {
    let points: Vec<Vec2> = vector
        .paths
        .iter()
        .flat_map(|path| &path.anchors)
        .flat_map(|anchor| [Some(anchor.point), anchor.in_handle, anchor.out_handle])
        .flatten()
        .collect();
    let (min_x, min_y, max_x, max_y) = bounds_of(points.iter());
    let corners = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ]
    .map(|corner| affine.apply(corner));
    bounds_of(corners.iter())
}

fn bounds_of<'a>(points: impl Iterator<Item = &'a Vec2>) -> Bounds {
    points.fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(min_x, min_y, max_x, max_y), p| {
            (
                min_x.min(p.x),
                min_y.min(p.y),
                max_x.max(p.x),
                max_y.max(p.y),
            )
        },
    )
}

/// Whether two boxes come within `reach` of each other.
fn overlaps(a: Bounds, b: Bounds, reach: f32) -> bool {
    a.0 <= b.2 + reach && b.0 <= a.2 + reach && a.1 <= b.3 + reach && b.1 <= a.3 + reach
}

/// Whether `line` runs along the edge of `region` for most of its length.
fn runs_along(line: &Line, region: &Region) -> bool {
    let reach = line.reach;
    // Most lines are nowhere near this region.
    if line.points.len() < 2 || !overlaps(line.bounds, region.bounds, reach) {
        return false;
    }
    let (min_x, min_y, max_x, max_y) = region.bounds;
    let near_box = |p: &Vec2| {
        p.x >= min_x - reach && p.x <= max_x + reach && p.y >= min_y - reach && p.y <= max_y + reach
    };
    let mut total = 0.0;
    let mut on_edge = 0.0;
    for pair in line.points.windows(2) {
        let a = pair[0];
        let dx = pair[1].x - a.x;
        let dy = pair[1].y - a.y;
        let length = dx.hypot(dy);
        total += length;
        // A crossing can have both ends on the edge while its entire
        // middle runs through the fill. Measure along the line itself.
        let steps = (length / reach.max(0.5)).ceil().clamp(1.0, 32.0) as usize;
        for step in 0..steps {
            let t = (step as f32 + 0.5) / steps as f32;
            let point = Vec2::new(a.x + dx * t, a.y + dy * t);
            if near_box(&point) && distance_to_rings(point, &region.rings) <= reach {
                on_edge += length / steps as f32;
            }
        }
    }
    total > 0.0 && on_edge >= ALONG * total
}

fn distance_to_rings(point: Vec2, rings: &[Vec<Vec2>]) -> f32 {
    let mut nearest = f32::MAX;
    for ring in rings {
        for pair in ring.windows(2) {
            nearest = nearest.min(distance_to_segment(point, pair[0], pair[1]));
        }
    }
    nearest
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx * dx + dy * dy;
    let t = if length <= 1.0e-12 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0)
    };
    (p.x - (a.x + dx * t)).hypot(p.y - (a.y + dy * t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::geom::{CapShape, JoinShape};
    use q0s_format::v2::{Anchor, Placement, Rgba, Stroke, Transform2D, Tween};

    fn path(points: &[(f32, f32)], closed: bool) -> VPath {
        VPath {
            anchors: points
                .iter()
                .map(|&(x, y)| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed,
        }
    }

    fn square(x: f32, y: f32) -> VPath {
        path(
            &[(x, y), (x + 10.0, y), (x + 10.0, y + 10.0), (x, y + 10.0)],
            true,
        )
    }

    /// Imported drawings often build a blob from outlines that cross. Two
    /// such blobs share one fill drawing, with a line along each one's
    /// left edge in a line drawing.
    #[test]
    fn a_click_picks_only_the_blob_under_it_when_outlines_cross() {
        let mut app = crate::app::EditorApp::default();
        let project = &mut app.state.project;
        project.assets.clear();
        let red = Rgba {
            r: 200,
            g: 0,
            b: 0,
            a: 255,
        };
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            // Each blob's second square starts outside its first.
            paths: vec![
                square(0.0, 0.0),
                path(&[(15.0, 15.0), (5.0, 15.0), (5.0, 5.0), (15.0, 5.0)], true),
                square(100.0, 0.0),
                path(
                    &[(115.0, 15.0), (105.0, 15.0), (105.0, 5.0), (115.0, 5.0)],
                    true,
                ),
            ],
            fill: Some(red),
            stroke: None,
        }));
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 2,
            paths: vec![
                path(&[(0.0, 0.0), (0.0, 10.0)], false),
                path(&[(100.0, 0.0), (100.0, 10.0)], false),
            ],
            fill: None,
            stroke: Some(Stroke {
                color: red,
                width: 1.0,
                cap: CapShape::Round,
                join: JoinShape::Round,
            }),
        }));
        let q0rg_id = project.q0rgs[0].q0rg_id;
        let layer = &mut project.q0rgs[0].layers[0];
        let layer_id = layer.layer_id;
        layer.placements.clear();
        for asset_id in [1, 2] {
            layer.placements.push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                pivot: None,
                tween: Tween::None,
                fx: Default::default(),
            });
        }
        let at = |placement_idx, path_idx| PathRef {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        };
        let shape = shape_of(
            &app.state.project,
            &mut app.textures,
            0,
            &Picked::Fill(vec![at(0, 0)]),
        )
        .expect("the blob has a line along it");
        let mut fills = shape.fills.clone();
        fills.sort();
        assert_eq!(fills, vec![at(0, 0), at(0, 1)], "just the left blob");
        assert_eq!(shape.strokes, vec![at(1, 0)], "just the left blob's line");

        let shape = shape_of(
            &app.state.project,
            &mut app.textures,
            0,
            &Picked::Line(at(1, 1)),
        )
        .expect("the line runs along a blob");
        let mut fills = shape.fills.clone();
        fills.sort();
        assert_eq!(fills, vec![at(0, 2), at(0, 3)], "just the right blob");
        assert_eq!(shape.strokes, vec![at(1, 1)]);
    }
}
