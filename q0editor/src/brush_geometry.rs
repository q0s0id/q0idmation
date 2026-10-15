//! Reconstruct boolean boundaries from their authored segments. Fitting a new
//! curve to an entire edited fill moves boundaries shared with other colours.
use std::collections::HashMap;

use geo::{LineString, MultiPolygon};
use lyon_path::{geom::CubicBezierSegment, math::point};
use q0s_format::v2::{Anchor, Path, Vec2};

pub(crate) const FLATNESS: f32 = 0.1;
type Cubic = CubicBezierSegment<f32>;

fn cubic(a: &Anchor, b: &Anchor) -> Cubic {
    let p1 = a.out_handle.unwrap_or(a.point);
    let p2 = b.in_handle.unwrap_or(b.point);
    Cubic {
        from: point(a.point.x, a.point.y),
        ctrl1: point(p1.x, p1.y),
        ctrl2: point(p2.x, p2.y),
        to: point(b.point.x, b.point.y),
    }
}

pub(crate) fn flatten_segment(a: &Anchor, b: &Anchor, mut emit: impl FnMut(Vec2, Vec2, f32, f32)) {
    if a.out_handle.is_none() && b.in_handle.is_none() {
        emit(a.point, b.point, 0.0, 1.0);
    } else {
        cubic(a, b).for_each_flattened_with_t(FLATNESS, &mut |line, t| {
            emit(
                Vec2::new(line.from.x, line.from.y),
                Vec2::new(line.to.x, line.to.y),
                t.start,
                t.end,
            );
        });
    }
}

struct Segment {
    start: Anchor,
    end: Anchor,
    edges: std::ops::Range<usize>,
}
struct Edge {
    a: Vec2,
    b: Vec2,
    segment: usize,
    t0: f32,
    t1: f32,
}
#[derive(Clone, Copy)]
struct Span {
    segment: usize,
    t0: f32,
    t1: f32,
}

pub(crate) struct BoundarySources {
    segments: Vec<Segment>,
    edges: Vec<Edge>,
    cells: HashMap<(i64, i64), Vec<usize>>,
    long_edges: Vec<usize>,
    cell_size: f64,
    epsilon: f64,
}

impl BoundarySources {
    pub(crate) fn new<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut result = Self {
            segments: Vec::new(),
            edges: Vec::new(),
            cells: HashMap::new(),
            long_edges: Vec::new(),
            cell_size: 1.0,
            epsilon: 1.0e-5,
        };
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for path in paths {
            if !path.closed {
                continue;
            }
            for (a, b) in path
                .anchors
                .iter()
                .zip(path.anchors.iter().cycle().skip(1))
                .take(path.anchors.len())
            {
                let segment = result.segments.len();
                result.segments.push(Segment {
                    start: a.clone(),
                    end: b.clone(),
                    edges: result.edges.len()..result.edges.len(),
                });
                flatten_segment(a, b, |a, b, t0, t1| {
                    for p in [a, b] {
                        min[0] = min[0].min(f64::from(p.x));
                        min[1] = min[1].min(f64::from(p.y));
                        max[0] = max[0].max(f64::from(p.x));
                        max[1] = max[1].max(f64::from(p.y));
                    }
                    if a != b {
                        result.edges.push(Edge {
                            a,
                            b,
                            segment,
                            t0,
                            t1,
                        });
                    }
                });
                result.segments[segment].edges.end = result.edges.len();
            }
        }
        if result.edges.is_empty() {
            return result;
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        result.cell_size = (extent / (result.edges.len() as f64).sqrt()).max(0.01);
        // Account only for f32/overlay rounding, never artistic simplification.
        result.epsilon += min.into_iter().chain(max).map(f64::abs).fold(0.0, f64::max)
            * f64::from(f32::EPSILON)
            * 2.0;
        for (index, edge) in result.edges.iter().enumerate() {
            let first = result.cell(
                f64::from(edge.a.x.min(edge.b.x)) - result.epsilon,
                f64::from(edge.a.y.min(edge.b.y)) - result.epsilon,
            );
            let last = result.cell(
                f64::from(edge.a.x.max(edge.b.x)) + result.epsilon,
                f64::from(edge.a.y.max(edge.b.y)) + result.epsilon,
            );
            if (last.0 - first.0 + 1).saturating_mul(last.1 - first.1 + 1) > 256 {
                result.long_edges.push(index);
                continue;
            }
            for x in first.0..=last.0 {
                for y in first.1..=last.1 {
                    result.cells.entry((x, y)).or_default().push(index);
                }
            }
        }
        result
    }

    fn cell(&self, x: f64, y: f64) -> (i64, i64) {
        (
            (x / self.cell_size).floor() as i64,
            (y / self.cell_size).floor() as i64,
        )
    }

    fn match_edge(&self, a: Vec2, b: Vec2) -> Option<Span> {
        let cell = self.cell(
            (f64::from(a.x) + f64::from(b.x)) * 0.5,
            (f64::from(a.y) + f64::from(b.y)) * 0.5,
        );
        let mut best = None;
        let mut best_error = f64::INFINITY;
        for &index in self
            .cells
            .get(&cell)
            .into_iter()
            .flatten()
            .chain(&self.long_edges)
        {
            let edge = &self.edges[index];
            let project = |p: Vec2| {
                let dx = f64::from(edge.b.x) - f64::from(edge.a.x);
                let dy = f64::from(edge.b.y) - f64::from(edge.a.y);
                let x = f64::from(p.x) - f64::from(edge.a.x);
                let y = f64::from(p.y) - f64::from(edge.a.y);
                let t = if x.hypot(y) <= self.epsilon {
                    0.0
                } else if (x - dx).hypot(y - dy) <= self.epsilon {
                    1.0
                } else {
                    ((x * dx + y * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0)
                };
                (t as f32, (x - t * dx).hypot(y - t * dy))
            };
            let (ta, ea) = project(a);
            let (tb, eb) = project(b);
            let error = ea.max(eb);
            if error <= self.epsilon && error < best_error && ta != tb {
                best_error = error;
                let parameter = |t| {
                    if t == 0.0 {
                        edge.t0
                    } else if t == 1.0 {
                        edge.t1
                    } else {
                        edge.t0 + t * (edge.t1 - edge.t0)
                    }
                };
                best = Some(Span {
                    segment: edge.segment,
                    t0: parameter(ta),
                    t1: parameter(tb),
                });
            }
        }
        if best.is_some() {
            return best;
        }
        // Overlay can drop collinear subdivision vertices. An output edge can
        // therefore span several samples of one authored cubic. Recover it only
        // when the complete intervening polyline follows that same edge.
        let mut visited = Vec::new();
        for &index in self
            .cells
            .get(&cell)
            .into_iter()
            .flatten()
            .chain(&self.long_edges)
        {
            let segment = self.edges[index].segment;
            if visited.contains(&segment) {
                continue;
            }
            visited.push(segment);
            let source = &self.segments[segment];
            let parameter = |p: Vec2| {
                self.edges[source.edges.clone()]
                    .iter()
                    .filter_map(|edge| {
                        let (t, error) = line_projection(edge.a, edge.b, p);
                        if error > self.epsilon {
                            return None;
                        }
                        let value = if distance(p, edge.a) <= self.epsilon {
                            edge.t0
                        } else if distance(p, edge.b) <= self.epsilon {
                            edge.t1
                        } else {
                            edge.t0 + t * (edge.t1 - edge.t0)
                        };
                        Some((value, error))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(t, _)| t)
            };
            let (Some(t0), Some(t1)) = (parameter(a), parameter(b)) else {
                continue;
            };
            if t0 == t1 {
                continue;
            }
            let follows_edge = self.edges[source.edges.clone()]
                .iter()
                .filter(|edge| edge.t1 > t0.min(t1) && edge.t1 < t0.max(t1))
                .all(|edge| line_projection(a, b, edge.b).1 <= self.epsilon);
            if follows_edge {
                return Some(Span { segment, t0, t1 });
            }
        }
        None
    }

    pub(crate) fn paths(&self, coverage: &MultiPolygon<f64>) -> Vec<Path> {
        coverage
            .0
            .iter()
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
            .filter_map(|ring| self.ring(ring))
            .collect()
    }

    fn ring(&self, ring: &LineString<f64>) -> Option<Path> {
        let mut points: Vec<_> = ring
            .0
            .iter()
            .map(|p| Vec2::new(p.x as f32, p.y as f32))
            .collect();
        points.dedup();
        if points.first() == points.last() {
            points.pop();
        }
        if points.len() < 3 {
            return None;
        }
        let mut spans: Vec<_> = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
            .map(|(&a, &b)| self.match_edge(a, b))
            .collect();
        let joins = |a: Option<Span>, b: Option<Span>| {
            matches!((a, b), (Some(a), Some(b))
            if a.segment == b.segment && (a.t1 - b.t0).abs() < 1.0e-5 && (a.t1 - a.t0) * (b.t1 - b.t0) > 0.0)
        };
        // Never leave an otherwise intact cubic split at the arbitrary ring start.
        let start = (0..points.len())
            .find(|&i| !joins(spans[(i + points.len() - 1) % points.len()], spans[i]))
            .unwrap_or(0);
        points.rotate_left(start);
        spans.rotate_left(start);
        let mut chunks = Vec::new();
        let mut i = 0;
        while i < points.len() {
            let first = i;
            let mut span = spans[i];
            while i + 1 < points.len() && joins(span, spans[i + 1]) {
                i += 1;
                span.as_mut().unwrap().t1 = spans[i].unwrap().t1;
            }
            chunks.push((points[first], span));
            i += 1;
        }
        // A clipped cubic and its closing chord can form a valid two-segment
        // cap. Raw-fill consumers require at least three anchors: split that
        // cubic exactly instead of dropping the cap on the next edit.
        while chunks.len() < 3 {
            let (i, span) = chunks
                .iter()
                .enumerate()
                .find_map(|(i, (_, span))| span.map(|s| (i, s)))?;
            let middle = (span.t0 + span.t1) * 0.5;
            let source = &self.segments[span.segment];
            let p = cubic(&source.start, &source.end).sample(middle);
            chunks[i].1.as_mut().unwrap().t1 = middle;
            chunks.insert(
                i + 1,
                (Vec2::new(p.x, p.y), Some(Span { t0: middle, ..span })),
            );
        }
        let endpoint = |span: Span, t: f32| {
            let source = &self.segments[span.segment];
            if t == 0.0 {
                Some(source.start.point)
            } else if t == 1.0 {
                Some(source.end.point)
            } else {
                None
            }
        };
        let mut anchors: Vec<_> = chunks
            .iter()
            .enumerate()
            .map(|(i, &(point, span))| {
                let previous = chunks[(i + chunks.len() - 1) % chunks.len()].1;
                let original = span
                    .and_then(|s| endpoint(s, s.t0))
                    .or_else(|| previous.and_then(|s| endpoint(s, s.t1)));
                Anchor {
                    point: original.unwrap_or(point),
                    in_handle: None,
                    out_handle: None,
                }
            })
            .collect();
        for (i, &(_, span)) in chunks.iter().enumerate() {
            let Some(span) = span else {
                continue;
            };
            let source = &self.segments[span.segment];
            let next = (i + 1) % anchors.len();
            if span.t0 == 0.0 && span.t1 == 1.0 {
                anchors[i].out_handle = source.start.out_handle;
                anchors[next].in_handle = source.end.in_handle;
                continue;
            }
            if span.t0 == 1.0 && span.t1 == 0.0 {
                anchors[i].out_handle = source.end.in_handle;
                anchors[next].in_handle = source.start.out_handle;
                continue;
            }
            if source.start.out_handle.is_none() && source.end.in_handle.is_none() {
                continue;
            }
            let mut curve = cubic(&source.start, &source.end);
            if span.t0 > span.t1 {
                curve = curve.split_range(span.t1..span.t0).flip();
            } else if span.t0 != 0.0 || span.t1 != 1.0 {
                curve = curve.split_range(span.t0..span.t1);
            }
            anchors[i].out_handle = Some(Vec2::new(
                curve.ctrl1.x + (anchors[i].point.x - curve.from.x),
                curve.ctrl1.y + (anchors[i].point.y - curve.from.y),
            ));
            anchors[next].in_handle = Some(Vec2::new(
                curve.ctrl2.x + (anchors[next].point.x - curve.to.x),
                curve.ctrl2.y + (anchors[next].point.y - curve.to.y),
            ));
        }
        Some(Path {
            anchors,
            closed: true,
        })
    }
}

fn distance(a: Vec2, b: Vec2) -> f64 {
    (f64::from(a.x) - f64::from(b.x)).hypot(f64::from(a.y) - f64::from(b.y))
}

fn line_projection(a: Vec2, b: Vec2, p: Vec2) -> (f32, f64) {
    let dx = f64::from(b.x) - f64::from(a.x);
    let dy = f64::from(b.y) - f64::from(a.y);
    let x = f64::from(p.x) - f64::from(a.x);
    let y = f64::from(p.y) - f64::from(a.y);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq == 0.0 {
        0.0
    } else {
        ((x * dx + y * dy) / length_sq).clamp(0.0, 1.0)
    };
    (t as f32, (x - t * dx).hypot(y - t * dy))
}
