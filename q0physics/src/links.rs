//! How links look: the lines a rope, chain or spring is drawn with, shared
//! by q0editor's bake and q0player's live worlds.

use q0s_format::physics::LinkLook;

/// One line to draw: its points, whether it closes on itself, and how
/// thick it is in stage pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkStroke {
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
    pub width: f32,
}

/// The strokes that draw a link running along `lines` (from
/// `World::link_lines`), `width` stage pixels thick.
pub fn link_strokes(lines: &[Vec<[f32; 2]>], look: LinkLook, width: f32) -> Vec<LinkStroke> {
    let width = width.max(0.1);
    let mut strokes = Vec::new();
    for line in lines.iter().filter(|line| line.len() >= 2) {
        match look {
            LinkLook::Hidden => {}
            LinkLook::Line => strokes.push(LinkStroke {
                points: smooth(line),
                closed: false,
                width,
            }),
            LinkLook::Chain => chain(&smooth(line), width, &mut strokes),
            LinkLook::Coil => strokes.push(LinkStroke {
                points: coil(line, width),
                closed: false,
                width: (width * 0.5).max(0.1),
            }),
        }
    }
    strokes
}

/// Fit the visible link to the attachments actually drawn this frame. Each
/// run says whether its first/last point is still tied to the original body;
/// cut ends stay free, even when their old coordinates coincide with a pin.
/// Deform along arc length so a large endpoint correction stretches the whole
/// run instead of leaving a disconnected tip or a single long last segment.
pub fn fit_link_lines(
    lines: &[Vec<[f32; 2]>],
    attachments: &[[bool; 2]],
    from: Option<[f32; 2]>,
    to: Option<[f32; 2]>,
) -> Vec<Vec<[f32; 2]>> {
    let finite = |point: [f32; 2]| point.into_iter().all(f32::is_finite);
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if line.len() < 2 {
                return line.clone();
            }
            let attached = attachments.get(index).copied().unwrap_or([false; 2]);
            let first = line[0];
            let last = line[line.len() - 1];
            let from = from
                .filter(|point| attached[0] && finite(*point))
                .unwrap_or(first);
            let to = to
                .filter(|point| attached[1] && finite(*point))
                .unwrap_or(last);
            if first == from && last == to {
                return line.clone();
            }
            let lengths = Walk::new(line).lengths;
            let total = lengths.last().copied().unwrap_or(0.0);
            let delta_from = [from[0] - first[0], from[1] - first[1]];
            let delta_to = [to[0] - last[0], to[1] - last[1]];
            let mut fitted = line
                .iter()
                .enumerate()
                .map(|(at, point)| {
                    let t = if total.is_finite() && total > 1.0e-6 {
                        lengths[at] / total
                    } else {
                        at as f32 / (line.len() - 1) as f32
                    };
                    if finite(*point) && total.is_finite() {
                        [
                            point[0] + delta_from[0] * (1.0 - t) + delta_to[0] * t,
                            point[1] + delta_from[1] * (1.0 - t) + delta_to[1] * t,
                        ]
                    } else {
                        [
                            from[0] + (to[0] - from[0]) * t,
                            from[1] + (to[1] - from[1]) * t,
                        ]
                    }
                })
                .collect::<Vec<_>>();
            // Endpoint identity is exact, independent of interpolation roundoff.
            fitted[0] = from;
            *fitted.last_mut().unwrap() = to;
            fitted
        })
        .collect()
}

/// Rounds the corners where rope pieces meet, keeping both ends.
fn smooth(line: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut points = line.to_vec();
    for _ in 0..2 {
        if points.len() < 3 {
            break;
        }
        let mut rounded = Vec::with_capacity(points.len() * 2);
        rounded.push(points[0]);
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            rounded.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            rounded.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        rounded.push(points[points.len() - 1]);
        points = rounded;
    }
    points
}

/// Walks a line by arc length.
struct Walk<'a> {
    line: &'a [[f32; 2]],
    /// Arc length at each point.
    lengths: Vec<f32>,
}

impl<'a> Walk<'a> {
    fn new(line: &'a [[f32; 2]]) -> Self {
        let mut lengths = Vec::with_capacity(line.len());
        let mut total = 0.0;
        lengths.push(0.0);
        for pair in line.windows(2) {
            total += (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
            lengths.push(total);
        }
        Self { line, lengths }
    }

    fn total(&self) -> f32 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    /// The point `distance` along, and the way the line runs there.
    fn at(&self, distance: f32) -> ([f32; 2], [f32; 2]) {
        let distance = distance.clamp(0.0, self.total());
        let index = self
            .lengths
            .partition_point(|length| *length < distance)
            .clamp(1, self.line.len() - 1);
        let (a, b) = (self.line[index - 1], self.line[index]);
        let span = (self.lengths[index] - self.lengths[index - 1]).max(1.0e-6);
        let t = ((distance - self.lengths[index - 1]) / span).clamp(0.0, 1.0);
        let along = [(b[0] - a[0]) / span, (b[1] - a[1]) / span];
        ([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], along)
    }
}

/// Oval links along `line`, every other one seen edge on.
fn chain(line: &[[f32; 2]], width: f32, strokes: &mut Vec<LinkStroke>) {
    let walk = Walk::new(line);
    let total = walk.total();
    if total <= 1.0e-3 {
        return;
    }
    let pitch = (width * 2.6).max(3.0);
    let count = (total / pitch).ceil().max(1.0) as usize;
    let pitch = total / count as f32;
    let wire = (width * 0.35).max(0.1);
    for index in 0..count {
        let (middle, along) = walk.at((index as f32 + 0.5) * pitch);
        // Links overlap their neighbours a little, the way chains hang.
        let half_length = pitch * 0.5 + wire;
        if index % 2 == 0 {
            let half_width = width * 0.75;
            let ring = (0..14)
                .map(|step| {
                    let angle = step as f32 * std::f32::consts::TAU / 14.0;
                    let (sin, cos) = angle.sin_cos();
                    let (u, v) = (cos * half_length, sin * half_width);
                    [
                        middle[0] + along[0] * u - along[1] * v,
                        middle[1] + along[1] * u + along[0] * v,
                    ]
                })
                .collect();
            strokes.push(LinkStroke {
                points: ring,
                closed: true,
                width: wire,
            });
        } else {
            strokes.push(LinkStroke {
                points: vec![
                    [
                        middle[0] - along[0] * half_length,
                        middle[1] - along[1] * half_length,
                    ],
                    [
                        middle[0] + along[0] * half_length,
                        middle[1] + along[1] * half_length,
                    ],
                ],
                closed: false,
                width: wire * 1.6,
            });
        }
    }
}

/// A zigzag along `line`, with short straight ends.
fn coil(line: &[[f32; 2]], width: f32) -> Vec<[f32; 2]> {
    let walk = Walk::new(line);
    let total = walk.total();
    let end = (width * 1.5).min(total * 0.15);
    let pitch = (width * 1.6).max(3.0);
    let turns = ((total - 2.0 * end) / pitch).floor().max(1.0) as usize;
    let pitch = (total - 2.0 * end) / turns as f32;
    let reach = width * 1.8;
    let mut points = vec![walk.at(0.0).0, walk.at(end).0];
    for turn in 0..turns {
        for (half, side) in [(0.25, 1.0), (0.75, -1.0)] {
            let (point, along) = walk.at(end + (turn as f32 + half) * pitch);
            points.push([
                point[0] - along[1] * reach * side,
                point[1] + along[0] * reach * side,
            ]);
        }
    }
    points.push(walk.at(total - end).0);
    points.push(walk.at(total).0);
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_keeps_its_ends_and_rounds_its_bends() {
        let bent = vec![vec![[0.0, 0.0], [50.0, 0.0], [50.0, 50.0]]];
        let strokes = link_strokes(&bent, LinkLook::Line, 4.0);
        assert_eq!(strokes.len(), 1);
        let points = &strokes[0].points;
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [50.0, 50.0]);
        assert!(!points.contains(&[50.0, 0.0]), "the corner is cut");
        assert!(link_strokes(&bent, LinkLook::Hidden, 4.0).is_empty());
    }

    #[test]
    fn a_chain_alternates_rings_and_edges_along_its_length() {
        let straight = vec![vec![[0.0, 0.0], [100.0, 0.0]]];
        let strokes = link_strokes(&straight, LinkLook::Chain, 4.0);
        assert!(strokes.len() >= 8, "{} links", strokes.len());
        assert!(strokes[0].closed && !strokes[1].closed);
        let reach = strokes
            .iter()
            .flat_map(|stroke| stroke.points.iter())
            .fold(0.0_f32, |reach, point| reach.max(point[1].abs()));
        assert!(
            reach <= 3.01,
            "rings stay about as wide as the chain: {reach}"
        );
    }

    #[test]
    fn a_coil_zigzags_between_straight_ends() {
        let straight = vec![vec![[0.0, 0.0], [0.0, 100.0]]];
        let strokes = link_strokes(&straight, LinkLook::Coil, 4.0);
        let points = &strokes[0].points;
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [0.0, 100.0]);
        assert!(points.iter().any(|point| point[0] > 5.0));
        assert!(points.iter().any(|point| point[0] < -5.0));
    }
}
