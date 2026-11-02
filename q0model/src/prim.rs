//! Ready-made shapes to start a model from. All are y-up, centred on the
//! origin, with faces wound counter-clockwise seen from outside.

use std::f32::consts::{PI, TAU};

use q0s_format::three::model::{newell_normal, Mesh};
use q0s_format::three::V3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Cube,
    Plane,
    Sphere,
    Cylinder,
    Cone,
    Torus,
    Capsule,
}

impl Primitive {
    pub const ALL: [Self; 7] = [
        Self::Cube,
        Self::Sphere,
        Self::Cylinder,
        Self::Cone,
        Self::Torus,
        Self::Capsule,
        Self::Plane,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Plane => "Plane",
            Self::Sphere => "Sphere",
            Self::Cylinder => "Cylinder",
            Self::Cone => "Cone",
            Self::Torus => "Torus",
            Self::Capsule => "Capsule",
        }
    }

    /// The shape, `size` units across.
    pub fn build(self, size: f32, detail: u32) -> Mesh {
        let r = size * 0.5;
        let detail = detail.clamp(3, 128);
        let mut mesh = match self {
            Self::Cube => cube(r),
            Self::Plane => plane(r),
            Self::Sphere => sphere(r, detail * 2, detail),
            Self::Cylinder => lathe(
                &[(0.0, -r), (r, -r), (r, r), (0.0, r)],
                detail * 2,
                &[false, true, false],
            ),
            Self::Cone => lathe(&[(0.0, -r), (r, -r), (0.0, r)], detail * 2, &[false, true]),
            Self::Torus => torus(r * 0.7, r * 0.3, detail * 2, detail),
            Self::Capsule => capsule(r * 0.5, r, detail * 2, detail),
        };
        mesh.name = self.label().to_string();
        mesh
    }
}

fn face(mesh: &mut Mesh, verts: &[u32], uvs: &[[f32; 2]], smooth: bool, outward: Option<V3>) {
    let mut verts = verts.to_vec();
    let mut uvs = uvs.to_vec();
    if let Some(outward) = outward {
        let normal = newell_normal(verts.iter().map(|v| mesh.positions[*v as usize]));
        if normal.dot(outward) < 0.0 {
            verts.reverse();
            uvs.reverse();
        }
    }
    mesh.push_face(&verts, Some(&uvs), 0, smooth);
}

fn cube(r: f32) -> Mesh {
    let mut mesh = Mesh::default();
    for z in [-r, r] {
        for y in [-r, r] {
            for x in [-r, r] {
                mesh.positions.push(V3::new(x, y, z));
            }
        }
    }
    let quads: [[u32; 4]; 6] = [
        [4, 5, 7, 6],
        [1, 0, 2, 3],
        [5, 1, 3, 7],
        [0, 4, 6, 2],
        [6, 7, 3, 2],
        [0, 1, 5, 4],
    ];
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    for quad in quads {
        let centre = quad
            .iter()
            .fold(V3::ZERO, |s, v| s + mesh.positions[*v as usize])
            * 0.25;
        face(&mut mesh, &quad, &uv, false, Some(centre));
    }
    mesh
}

fn plane(r: f32) -> Mesh {
    let mut mesh = Mesh {
        positions: vec![
            V3::new(-r, 0.0, r),
            V3::new(r, 0.0, r),
            V3::new(r, 0.0, -r),
            V3::new(-r, 0.0, -r),
        ],
        ..Mesh::default()
    };
    face(
        &mut mesh,
        &[0, 1, 2, 3],
        &[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        false,
        Some(V3::Y),
    );
    mesh
}

fn sphere(r: f32, segments: u32, rings: u32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.positions.push(V3::new(0.0, r, 0.0));
    for ring in 1..rings {
        let phi = PI * ring as f32 / rings as f32;
        for s in 0..segments {
            let theta = TAU * s as f32 / segments as f32;
            mesh.positions.push(V3::new(
                r * phi.sin() * theta.cos(),
                r * phi.cos(),
                -r * phi.sin() * theta.sin(),
            ));
        }
    }
    mesh.positions.push(V3::new(0.0, -r, 0.0));
    let bottom = mesh.positions.len() as u32 - 1;
    let at = |ring: u32, s: u32| 1 + (ring - 1) * segments + s % segments;
    let uv = |ring: u32, s: u32| [s as f32 / segments as f32, ring as f32 / rings as f32];
    for s in 0..segments {
        let verts = [0, at(1, s), at(1, s + 1)];
        let centre = verts
            .iter()
            .fold(V3::ZERO, |a, v| a + mesh.positions[*v as usize]);
        face(
            &mut mesh,
            &verts,
            &[
                [(s as f32 + 0.5) / segments as f32, 0.0],
                uv(1, s),
                uv(1, s + 1),
            ],
            true,
            Some(centre),
        );
    }
    for ring in 1..rings - 1 {
        for s in 0..segments {
            let verts = [
                at(ring, s),
                at(ring + 1, s),
                at(ring + 1, s + 1),
                at(ring, s + 1),
            ];
            let centre = verts
                .iter()
                .fold(V3::ZERO, |a, v| a + mesh.positions[*v as usize]);
            face(
                &mut mesh,
                &verts,
                &[
                    uv(ring, s),
                    uv(ring + 1, s),
                    uv(ring + 1, s + 1),
                    uv(ring, s + 1),
                ],
                true,
                Some(centre),
            );
        }
    }
    for s in 0..segments {
        let verts = [bottom, at(rings - 1, s + 1), at(rings - 1, s)];
        let centre = verts
            .iter()
            .fold(V3::ZERO, |a, v| a + mesh.positions[*v as usize]);
        face(
            &mut mesh,
            &verts,
            &[
                [(s as f32 + 0.5) / segments as f32, 1.0],
                uv(rings - 1, s + 1),
                uv(rings - 1, s),
            ],
            true,
            Some(centre),
        );
    }
    mesh
}

/// A profile of (radius, height) points turned about the y axis. `smooth`
/// says which spans between points are smooth.
fn lathe(profile: &[(f32, f32)], segments: u32, smooth: &[bool]) -> Mesh {
    let mut mesh = Mesh::default();
    let mut rows: Vec<Vec<u32>> = Vec::new();
    for (radius, y) in profile {
        if *radius <= 1.0e-6 {
            mesh.positions.push(V3::new(0.0, *y, 0.0));
            rows.push(vec![mesh.positions.len() as u32 - 1]);
        } else {
            let mut row = Vec::new();
            for s in 0..segments {
                let theta = TAU * s as f32 / segments as f32;
                mesh.positions
                    .push(V3::new(radius * theta.cos(), *y, -radius * theta.sin()));
                row.push(mesh.positions.len() as u32 - 1);
            }
            rows.push(row);
        }
    }
    let axis_point = |mesh: &Mesh, verts: &[u32]| {
        let c = verts
            .iter()
            .fold(V3::ZERO, |a, v| a + mesh.positions[*v as usize])
            / verts.len() as f32;
        V3::new(c.x, 0.0, c.z)
    };
    for span in 0..rows.len() - 1 {
        let (a, b) = (&rows[span], &rows[span + 1]);
        let smooth = smooth.get(span).copied().unwrap_or(true);
        let v_a = span as f32 / (rows.len() - 1) as f32;
        let v_b = (span + 1) as f32 / (rows.len() - 1) as f32;
        match (a.len(), b.len()) {
            (1, 1) => {}
            (1, _) | (_, 1) => {
                // A cap: one polygon when flat, else a fan to the tip.
                let (tip, ring, tip_v, ring_v) = if a.len() == 1 {
                    (a[0], b, v_a, v_b)
                } else {
                    (b[0], a, v_b, v_a)
                };
                let flat = (mesh.positions[tip as usize].y - mesh.positions[ring[0] as usize].y)
                    .abs()
                    < 1.0e-6;
                if flat {
                    let outward = V3::new(0.0, mesh.positions[tip as usize].y.signum(), 0.0);
                    let uvs: Vec<[f32; 2]> = ring
                        .iter()
                        .map(|v| {
                            let p = mesh.positions[*v as usize];
                            [0.5 + p.x * 0.5, 0.5 + p.z * 0.5]
                        })
                        .collect();
                    let ring = ring.clone();
                    face(&mut mesh, &ring, &uvs, false, Some(outward));
                } else {
                    for s in 0..segments {
                        let verts = [tip, ring[s as usize], ring[((s + 1) % segments) as usize]];
                        let out = axis_point(&mesh, &verts[1..]);
                        let uvs = [
                            [(s as f32 + 0.5) / segments as f32, tip_v],
                            [s as f32 / segments as f32, ring_v],
                            [(s + 1) as f32 / segments as f32, ring_v],
                        ];
                        face(&mut mesh, &verts, &uvs, smooth, Some(out));
                    }
                }
            }
            _ => {
                for s in 0..segments as usize {
                    let n = (s + 1) % segments as usize;
                    let verts = [a[s], b[s], b[n], a[n]];
                    let out = axis_point(&mesh, &verts);
                    let (u0, u1) = (s as f32 / segments as f32, (s + 1) as f32 / segments as f32);
                    face(
                        &mut mesh,
                        &verts,
                        &[[u0, v_a], [u0, v_b], [u1, v_b], [u1, v_a]],
                        smooth,
                        Some(out),
                    );
                }
            }
        }
    }
    mesh
}

fn torus(major: f32, minor: f32, segments: u32, sides: u32) -> Mesh {
    let mut mesh = Mesh::default();
    for s in 0..segments {
        let theta = TAU * s as f32 / segments as f32;
        for k in 0..sides {
            let phi = TAU * k as f32 / sides as f32;
            let ring = major + minor * phi.cos();
            mesh.positions.push(V3::new(
                ring * theta.cos(),
                minor * phi.sin(),
                -ring * theta.sin(),
            ));
        }
    }
    let at = |s: u32, k: u32| (s % segments) * sides + k % sides;
    for s in 0..segments {
        let theta = TAU * (s as f32 + 0.5) / segments as f32;
        let ring_centre = V3::new(major * theta.cos(), 0.0, -major * theta.sin());
        for k in 0..sides {
            let verts = [at(s, k), at(s + 1, k), at(s + 1, k + 1), at(s, k + 1)];
            let centre = verts
                .iter()
                .fold(V3::ZERO, |a, v| a + mesh.positions[*v as usize])
                * 0.25;
            let (u0, u1) = (s as f32 / segments as f32, (s + 1) as f32 / segments as f32);
            let (v0, v1) = (k as f32 / sides as f32, (k + 1) as f32 / sides as f32);
            face(
                &mut mesh,
                &verts,
                &[[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
                true,
                Some(centre - ring_centre),
            );
        }
    }
    mesh
}

fn capsule(radius: f32, half: f32, segments: u32, rings: u32) -> Mesh {
    let cap = (rings / 2).max(2);
    let mut profile = Vec::new();
    profile.push((0.0, -half));
    for i in 1..=cap {
        let a = PI * 0.5 * i as f32 / cap as f32;
        profile.push((radius * a.sin(), -half + radius - radius * a.cos()));
    }
    for i in 0..=cap {
        let a = PI * 0.5 * i as f32 / cap as f32;
        profile.push((radius * a.cos(), half - radius + radius * a.sin()));
    }
    profile.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0e-6 && (a.1 - b.1).abs() < 1.0e-6);
    if profile.last().is_some_and(|p| p.0 > 1.0e-6) {
        profile.push((0.0, half));
    }
    let smooth = vec![true; profile.len()];
    lathe(&profile, segments, &smooth)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every face of a closed convex shape faces away from its middle.
    fn faces_outward(mesh: &Mesh) -> bool {
        mesh.faces.iter().all(|f| {
            let n = mesh.face_normal(f);
            let c = mesh.face_center(f);
            n.dot(c) > -1.0e-4
        })
    }

    #[test]
    fn shapes_are_closed_and_face_out() {
        for prim in [
            Primitive::Cube,
            Primitive::Sphere,
            Primitive::Cylinder,
            Primitive::Cone,
            Primitive::Capsule,
        ] {
            let mesh = prim.build(2.0, 8);
            assert!(faces_outward(&mesh), "{prim:?}");
            // Closed: every edge is shared by exactly two faces.
            let mut edges = std::collections::HashMap::new();
            for f in &mesh.faces {
                let v = mesh.face_verts(f);
                for i in 0..v.len() {
                    let (a, b) = (v[i], v[(i + 1) % v.len()]);
                    *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
            assert!(edges.values().all(|n| *n == 2), "{prim:?} is open");
        }
        let torus = Primitive::Torus.build(2.0, 8);
        assert_eq!(torus.faces.len(), 16 * 8);
    }
}
