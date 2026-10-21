//! The shapes behind hull and outline colliders: the points an object's
//! artwork is drawn with, in the object's own space, and its filled areas
//! and drawn lines cut into convex parts the physics can collide with.

use std::collections::HashMap;

use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{FillOptions, FillRule, FillTessellator};
use q0s_format::geom::{flatten_path, stroke_polygons, CapShape, JoinShape};
use q0s_format::physics::ColliderShape;
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, ProjectV2, Target, Vec2, VectorAsset};

/// Symbols inside symbols are followed this deep.
const DEPTH_LIMIT: u8 = 8;
/// Points per curve segment when flattening paths.
const SAMPLES: usize = 8;
/// Outlines are simplified until they hold at most this many points.
const MAX_POINTS: usize = 400;
/// At most this many convex parts per collider.
const MAX_PARTS: usize = 256;

/// One loop of artwork in the object's own space.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtLoop {
    pub points: Vec<[f32; 2]>,
    /// Closed loops enclose an area; open ones only add points to a hull.
    pub closed: bool,
    /// The line drawn along the points, if there is one.
    pub stroke: Option<ArtStroke>,
}

/// How a line is drawn along an [`ArtLoop`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArtStroke {
    /// Half the line's width in the object's space.
    pub half_width: f32,
    /// Whether the line runs on from the last point back to the first.
    pub closed: bool,
    pub cap: CapShape,
    pub join: JoinShape,
}

/// The loops object `instance_id` is drawn with on `frame` of `q0rg_id`, in
/// its own space: shape tweens in between and symbols on their own frames.
pub fn object_loops(
    project: &ProjectV2,
    q0rg_id: u16,
    instance_id: u32,
    frame: u16,
) -> Vec<ArtLoop> {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    for layer in &q0rg.layers {
        for state in q0s_format::raster::active_placement_states_at(layer, frame) {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            if placement.instance_id != instance_id {
                continue;
            }
            let mut loops = Vec::new();
            let morph = q0s_format::shape_tween::project_layer_morph(project, layer, frame);
            if let Some(shapes) = morph
                .as_ref()
                .and_then(|morph| morph.for_placement(state.index))
            {
                for shape in shapes {
                    vector_loops(shape, Affine::IDENTITY, &mut loops);
                }
                return loops;
            }
            let child_frame = match placement.target {
                Target::Q0rg(child_id) => q0s_format::v2::resolve_q0rg_playback_frame(
                    project,
                    child_id,
                    frame,
                    state.playback_start_frame,
                    state.fx.q0rg_playback,
                ),
                Target::Asset(_) => 0,
            };
            target_loops(
                project,
                placement.target,
                child_frame,
                Affine::IDENTITY,
                0,
                &mut loops,
            );
            return loops;
        }
    }
    Vec::new()
}

fn vector_loops(vector: &VectorAsset, affine: Affine, out: &mut Vec<ArtLoop>) {
    let half_width = vector
        .stroke
        .as_ref()
        .filter(|stroke| stroke.color.a > 0)
        .map(|stroke| stroke.width * 0.5 * affine.uniform_scale())
        .filter(|half_width| half_width.is_finite() && *half_width > 0.0);
    for path in &vector.paths {
        let points = flatten_path(path, SAMPLES)
            .into_iter()
            .map(|point| {
                let placed = affine.apply(point);
                [placed.x, placed.y]
            })
            .collect::<Vec<_>>();
        let stroke = vector
            .stroke
            .as_ref()
            .zip(half_width)
            .map(|(stroke, half_width)| ArtStroke {
                half_width,
                closed: path.closed,
                cap: stroke.cap,
                join: stroke.join,
            });
        // A lone point only shows when a nib draws it.
        if points.len() >= 2 || (!points.is_empty() && stroke.is_some()) {
            out.push(ArtLoop {
                points,
                closed: path.closed && vector.fill.is_some(),
                stroke,
            });
        }
    }
}

fn rect_loop(min: [f32; 2], max: [f32; 2], affine: Affine) -> ArtLoop {
    let points = [
        [min[0], min[1]],
        [max[0], min[1]],
        [max[0], max[1]],
        [min[0], max[1]],
    ]
    .into_iter()
    .map(|[x, y]| {
        let placed = affine.apply(Vec2::new(x, y));
        [placed.x, placed.y]
    })
    .collect();
    ArtLoop {
        points,
        closed: true,
        stroke: None,
    }
}

fn target_loops(
    project: &ProjectV2,
    target: Target,
    frame: u16,
    affine: Affine,
    depth: u8,
    out: &mut Vec<ArtLoop>,
) {
    if depth > DEPTH_LIMIT {
        return;
    }
    match target {
        Target::Asset(id) => match project.assets.iter().find(|asset| asset.id() == id) {
            Some(Asset::Vector(vector)) => vector_loops(vector, affine, out),
            Some(Asset::Rig(_)) | None => {}
            Some(_) => {
                if let Some((min_x, min_y, max_x, max_y)) =
                    crate::render::target_local_bbox(project, target, None)
                {
                    out.push(rect_loop([min_x, min_y], [max_x, max_y], affine));
                }
            }
        },
        Target::Q0rg(child_id) => {
            let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == child_id) else {
                return;
            };
            let local_frame = if q0rg.frame_count > 0 {
                frame % q0rg.frame_count
            } else {
                0
            };
            for layer in &q0rg.layers {
                if !project.layer_is_visible(child_id, layer.layer_id)
                    || project.layer_metadata(child_id, layer.layer_id).kind
                        == q0s_format::v2::LayerKind::Mask
                {
                    continue;
                }
                let morph =
                    q0s_format::shape_tween::project_layer_morph(project, layer, local_frame);
                for state in q0s_format::raster::active_placement_states_at(layer, local_frame) {
                    let Some(placement) = layer.placements.get(state.index) else {
                        continue;
                    };
                    let placed = Affine::compose(affine, Affine::from_transform(state.transform));
                    if let Some(shapes) = morph
                        .as_ref()
                        .and_then(|morph| morph.for_placement(state.index))
                    {
                        for shape in shapes {
                            vector_loops(shape, placed, out);
                        }
                        continue;
                    }
                    let grandchild_frame = match placement.target {
                        Target::Q0rg(grandchild) if grandchild != child_id => {
                            q0s_format::v2::resolve_q0rg_playback_frame(
                                project,
                                grandchild,
                                local_frame,
                                state.playback_start_frame,
                                state.fx.q0rg_playback,
                            )
                        }
                        Target::Q0rg(_) => continue,
                        Target::Asset(_) => 0,
                    };
                    target_loops(
                        project,
                        placement.target,
                        grandchild_frame,
                        placed,
                        depth + 1,
                        out,
                    );
                }
            }
        }
    }
}

/// The parts a `shape` collider is made of, from the artwork `loops`; empty
/// for shapes that do not follow the artwork.
pub fn collider_parts(shape: ColliderShape, loops: &[ArtLoop]) -> Vec<Vec<[f32; 2]>> {
    match shape {
        ColliderShape::Hull => {
            // Lines count with their thickness, ends and corners.
            let lines = stroke_parts(loops, usize::MAX);
            let hull = q0physics::convex_hull(
                loops
                    .iter()
                    .flat_map(|art| art.points.iter().copied())
                    .chain(lines.into_iter().flatten()),
            );
            if hull.len() >= 3 {
                vec![hull]
            } else {
                Vec::new()
            }
        }
        ColliderShape::Outline => {
            let mut parts = convex_parts(loops);
            parts.truncate(MAX_PARTS / 2);
            let lines = stroke_parts(loops, MAX_PARTS - parts.len());
            parts.extend(lines);
            if parts.is_empty() {
                // Nothing filled or drawn: wrap what is there instead.
                collider_parts(ColliderShape::Hull, loops)
            } else {
                parts.truncate(MAX_PARTS);
                parts
            }
        }
        _ => Vec::new(),
    }
}

/// The lines drawn along `loops`, as thick as they are drawn, cut into
/// about `budget` convex parts at most: one band per stretch of a line,
/// wrapped together with the corners and ends that touch it.
pub fn stroke_parts(loops: &[ArtLoop], budget: usize) -> Vec<Vec<[f32; 2]>> {
    let lines = loops
        .iter()
        .filter_map(|art| art.stroke.map(|stroke| (&art.points, stroke)))
        .collect::<Vec<_>>();
    if lines.is_empty() || budget == 0 {
        return Vec::new();
    }
    let size = lines.iter().flat_map(|(points, _)| points.iter()).fold(
        [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
        |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        },
    );
    let span = (size[2] - size[0]).max(size[3] - size[1]).max(1.0);
    // Straighter, fewer stretches until the parts fit the budget.
    let mut tolerance = span * 0.002;
    loop {
        let simplified = lines
            .iter()
            .map(|(points, stroke)| {
                let points = if stroke.closed && points.len() >= 3 {
                    q0physics::contour::simplify_loop(points, tolerance)
                } else {
                    simplify_open(points, tolerance)
                };
                (points, *stroke)
            })
            .collect::<Vec<_>>();
        let stretches = simplified
            .iter()
            .map(|(points, stroke)| {
                if stroke.closed {
                    points.len()
                } else {
                    points.len().saturating_sub(1).max(1)
                }
            })
            .sum::<usize>();
        if stretches <= budget || tolerance > span {
            return simplified
                .iter()
                .flat_map(|(points, stroke)| line_parts(points, *stroke))
                .collect();
        }
        tolerance *= 1.6;
    }
}

/// An open line with fewer points that strays no more than `tolerance`
/// from `points`; its ends stay put.
fn simplify_open(points: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((from, to)) = stack.pop() {
        if to <= from + 1 {
            continue;
        }
        let (worst, off) = (from + 1..to)
            .map(|index| {
                (
                    index,
                    segment_distance(points[index], points[from], points[to]),
                )
            })
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .expect("a point between");
        if off > tolerance {
            keep[worst] = true;
            stack.push((from, worst));
            stack.push((worst, to));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

/// How far `point` is from the segment `a`-`b`.
fn segment_distance(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let line = [b[0] - a[0], b[1] - a[1]];
    let length_sq = line[0] * line[0] + line[1] * line[1];
    let t = if length_sq <= 1.0e-12 {
        0.0
    } else {
        (((point[0] - a[0]) * line[0] + (point[1] - a[1]) * line[1]) / length_sq).clamp(0.0, 1.0)
    };
    (point[0] - a[0] - line[0] * t).hypot(point[1] - a[1] - line[1] * t)
}

/// One line's convex parts: the bands `stroke_polygons` lays along each
/// stretch, each wrapped together with the corners and ends nearest it.
fn line_parts(points: &[[f32; 2]], stroke: ArtStroke) -> Vec<Vec<[f32; 2]>> {
    // The points stroke_polygons keeps, so its first polygons are exactly
    // the bands of these stretches.
    let mut kept: Vec<Vec2> = Vec::with_capacity(points.len());
    for &[x, y] in points {
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        if kept
            .last()
            .is_none_or(|last| (last.x - x).hypot(last.y - y) > 1.0e-4)
        {
            kept.push(Vec2::new(x, y));
        }
    }
    let closed = stroke.closed && kept.len() >= 3;
    if closed {
        let (first, last) = (kept[0], kept[kept.len() - 1]);
        if (first.x - last.x).hypot(first.y - last.y) <= 1.0e-4 {
            kept.pop();
        }
    }
    let polygons = stroke_polygons(&kept, closed, stroke.half_width, stroke.cap, stroke.join)
        .into_iter()
        .map(|polygon| polygon.into_iter().map(|p| [p.x, p.y]).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let n = kept.len();
    let stretches = match n {
        0 | 1 => 0,
        _ if closed => n,
        _ => n - 1,
    };
    if stretches == 0 || polygons.len() < stretches {
        return polygons;
    }
    let mut parts = polygons[..stretches].to_vec();
    for extra in &polygons[stretches..] {
        let count = extra.len().max(1) as f32;
        let middle = extra.iter().fold([0.0, 0.0], |sum, p| {
            [sum[0] + p[0] / count, sum[1] + p[1] / count]
        });
        let along = |i: usize| {
            let (from, to) = (kept[i], kept[(i + 1) % n]);
            segment_distance(middle, [from.x, from.y], [to.x, to.y])
        };
        let nearest = (0..stretches)
            .min_by(|a, b| along(*a).total_cmp(&along(*b)))
            .expect("a stretch");
        parts[nearest].extend(extra.iter().copied());
    }
    parts
        .into_iter()
        .map(q0physics::convex_hull)
        .filter(|part| part.len() >= 3)
        .collect()
}

/// The closed `loops`, filled even-odd like the renderer fills them, cut
/// into convex parts.
pub fn convex_parts(loops: &[ArtLoop]) -> Vec<Vec<[f32; 2]>> {
    let closed = loops
        .iter()
        .filter(|art| art.closed && art.points.len() >= 3)
        .map(|art| art.points.clone())
        .collect::<Vec<_>>();
    if closed.is_empty() {
        return Vec::new();
    }
    let closed = simplified(closed);
    let Some((positions, indices)) = triangulate(&closed) else {
        return Vec::new();
    };
    let mut parts = merge_triangles(&positions, &indices);
    parts.truncate(MAX_PARTS);
    parts
}

/// Fewer points, the same look: the tolerance grows until the outlines
/// hold at most `MAX_POINTS` points between them.
fn simplified(loops: Vec<Vec<[f32; 2]>>) -> Vec<Vec<[f32; 2]>> {
    let size = loops
        .iter()
        .flatten()
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        });
    let span = (size[2] - size[0]).max(size[3] - size[1]).max(1.0);
    let mut tolerance = span * 0.002;
    loop {
        let reduced = loops
            .iter()
            .map(|points| q0physics::contour::simplify_loop(points, tolerance))
            .filter(|points| points.len() >= 3)
            .collect::<Vec<_>>();
        let total = reduced.iter().map(Vec::len).sum::<usize>();
        if total <= MAX_POINTS || tolerance > span {
            return reduced;
        }
        tolerance *= 1.6;
    }
}

fn triangulate(loops: &[Vec<[f32; 2]>]) -> Option<(Vec<[f32; 2]>, Vec<u32>)> {
    let mut builder = lyon_path::Path::builder();
    for points in loops {
        let mut points = points.iter();
        let first = points.next()?;
        builder.begin(lyon_point(first[0], first[1]));
        for point in points {
            builder.line_to(lyon_point(point[0], point[1]));
        }
        builder.end(true);
    }
    let path = builder.build();
    let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &path,
            &FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut buffers, Positions),
        )
        .ok()?;
    let positions = buffers
        .vertices
        .iter()
        .map(|point| [point.x, point.y])
        .collect();
    Some((positions, buffers.indices))
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn is_convex(points: &[[f32; 2]], loop_of: &[u32]) -> bool {
    let n = loop_of.len();
    (0..n).all(|i| {
        let a = points[loop_of[i] as usize];
        let b = points[loop_of[(i + 1) % n] as usize];
        let c = points[loop_of[(i + 2) % n] as usize];
        cross(a, b, c) >= -1.0e-4
    })
}

/// Joins neighbouring triangles while the result stays convex (after
/// Hertel and Mehlhorn), so a collider holds a few parts, not hundreds.
fn merge_triangles(points: &[[f32; 2]], indices: &[u32]) -> Vec<Vec<[f32; 2]>> {
    let mut polygons: Vec<Option<Vec<u32>>> = indices
        .chunks_exact(3)
        .filter_map(|triangle| {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let area = cross(points[a as usize], points[b as usize], points[c as usize]);
            if area.abs() <= 1.0e-6 {
                None
            } else if area > 0.0 {
                Some(Some(vec![a, b, c]))
            } else {
                Some(Some(vec![a, c, b]))
            }
        })
        .collect();
    loop {
        // Which polygon owns each directed edge.
        let mut edges = HashMap::new();
        for (id, polygon) in polygons.iter().enumerate() {
            let Some(polygon) = polygon else { continue };
            for i in 0..polygon.len() {
                edges.insert((polygon[i], polygon[(i + 1) % polygon.len()]), id);
            }
        }
        let mut merged_any = false;
        for id in 0..polygons.len() {
            let Some(polygon) = polygons[id].clone() else {
                continue;
            };
            for i in 0..polygon.len() {
                let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                let Some(&other) = edges.get(&(b, a)) else {
                    continue;
                };
                if other == id {
                    continue;
                }
                let Some(neighbour) = polygons[other].clone() else {
                    continue;
                };
                // This polygon from b round to a, then the neighbour from a
                // round to b, without repeating the shared edge.
                let start = (i + 1) % polygon.len();
                let mut joined = (0..polygon.len())
                    .map(|k| polygon[(start + k) % polygon.len()])
                    .collect::<Vec<_>>();
                let at = neighbour.iter().position(|v| *v == a).expect("shared edge");
                joined.extend(
                    (1..neighbour.len() - 1).map(|k| neighbour[(at + k) % neighbour.len()]),
                );
                if is_convex(points, &joined) {
                    polygons[id] = Some(joined);
                    polygons[other] = None;
                    merged_any = true;
                    break;
                }
            }
            if merged_any {
                break;
            }
        }
        if !merged_any {
            break;
        }
    }
    polygons
        .into_iter()
        .flatten()
        .map(|polygon| {
            polygon
                .into_iter()
                .map(|index| points[index as usize])
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(points: &[[f32; 2]]) -> ArtLoop {
        ArtLoop {
            points: points.to_vec(),
            closed: true,
            stroke: None,
        }
    }

    fn line(points: &[[f32; 2]], width: f32) -> ArtLoop {
        ArtLoop {
            points: points.to_vec(),
            closed: false,
            stroke: Some(ArtStroke {
                half_width: width * 0.5,
                closed: false,
                cap: CapShape::Butt,
                join: JoinShape::Miter,
            }),
        }
    }

    fn inside(parts: &[Vec<[f32; 2]>], point: [f32; 2]) -> bool {
        parts.iter().any(|part| {
            let turns = (0..part.len())
                .map(|i| cross(part[i], part[(i + 1) % part.len()], point))
                .collect::<Vec<_>>();
            turns.iter().all(|t| *t >= -1.0e-3) || turns.iter().all(|t| *t <= 1.0e-3)
        })
    }

    fn area(part: &[[f32; 2]]) -> f32 {
        part.iter()
            .zip(part.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f32>()
            .abs()
            * 0.5
    }

    #[test]
    fn a_square_stays_one_part() {
        let parts = convex_parts(&[closed(&[
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
        ])]);
        assert_eq!(parts.len(), 1);
        assert!((area(&parts[0]) - 100.0).abs() < 0.01);
    }

    #[test]
    fn an_l_shape_splits_into_convex_parts_covering_it() {
        let l = closed(&[
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 20.0],
            [30.0, 20.0],
            [30.0, 30.0],
            [0.0, 30.0],
        ]);
        let parts = convex_parts(&[l]);
        assert!(
            parts.len() >= 2 && parts.len() <= 4,
            "{} parts",
            parts.len()
        );
        let total = parts.iter().map(|part| area(part)).sum::<f32>();
        assert!((total - 500.0).abs() < 0.5, "covers the L: {total}");
        for part in &parts {
            let indices = (0..part.len() as u32).collect::<Vec<_>>();
            assert!(is_convex(part, &indices));
        }
    }

    #[test]
    fn a_ring_leaves_its_hole_empty() {
        let square = |half: f32, reversed: bool| {
            let mut points = vec![[-half, -half], [half, -half], [half, half], [-half, half]];
            if reversed {
                points.reverse();
            }
            closed(&points)
        };
        let parts = convex_parts(&[square(20.0, false), square(10.0, true)]);
        let total = parts.iter().map(|part| area(part)).sum::<f32>();
        assert!((total - 1200.0).abs() < 0.5, "40² - 20²: {total}");
    }

    #[test]
    fn a_hull_takes_every_point_and_open_paths_too() {
        let open = ArtLoop {
            points: vec![[0.0, 0.0], [10.0, 10.0]],
            closed: false,
            stroke: None,
        };
        let more = closed(&[[0.0, 10.0], [5.0, 20.0], [-5.0, 15.0]]);
        let parts = collider_parts(ColliderShape::Hull, &[open.clone(), more]);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].len() >= 4);
        // An outline of nothing closed falls back to the hull.
        assert!(collider_parts(ColliderShape::Outline, &[open]).is_empty());
    }

    #[test]
    fn an_outline_of_a_bent_line_follows_the_line_not_a_lasso() {
        // An L drawn as one 4 wide line: the corner it wraps stays empty.
        let bent = line(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]], 4.0);
        let parts = collider_parts(ColliderShape::Outline, &[bent]);
        assert!(!parts.is_empty());
        assert!(!inside(&parts, [50.0, 50.0]), "no lasso across the bend");
        assert!(!inside(&parts, [80.0, 20.0]));
        for along in [
            [10.0, 0.0],
            [50.0, 1.5],
            [99.0, 0.0],
            [100.0, 50.0],
            [101.5, 90.0],
        ] {
            assert!(inside(&parts, along), "covers the line at {along:?}");
        }
        // A sharp corner reaches out to its point.
        assert!(inside(&parts, [101.8, -1.8]));
        let total = parts.iter().map(|part| area(part)).sum::<f32>();
        assert!(
            total < 2.0 * 100.0 * 4.0 + 20.0,
            "about the line's own area: {total}"
        );
    }

    #[test]
    fn a_long_wavy_line_stays_within_the_part_budget() {
        let points = (0..2000)
            .map(|i| {
                let x = i as f32 * 0.5;
                [x, (x * 0.05).sin() * 40.0]
            })
            .collect::<Vec<_>>();
        let parts = collider_parts(ColliderShape::Outline, &[line(&points, 6.0)]);
        assert!(parts.len() <= MAX_PARTS, "{} parts", parts.len());
        assert!(parts.len() >= 8);
        for x in [0.0f32, 200.0, 500.0, 999.0] {
            let y = (x * 0.05).sin() * 40.0;
            assert!(inside(&parts, [x, y]), "on the line at x = {x}");
        }
    }

    #[test]
    fn a_hull_counts_a_line_with_its_thickness() {
        let parts = collider_parts(
            ColliderShape::Hull,
            &[line(&[[0.0, 0.0], [100.0, 0.0]], 10.0)],
        );
        assert_eq!(parts.len(), 1);
        assert!((area(&parts[0]) - 1000.0).abs() < 1.0);
    }
}
