//! Mesh editing: the operations of the model editor. They work on face
//! and vertex selections given as index sets, and leave texture positions
//! in step with the faces. Imported normals are dropped by anything that
//! moves geometry, so the mesh is shaded from its new shape.

use std::collections::{BTreeSet, HashMap, HashSet};

use q0s_format::three::model::{newell_normal, Face, Mesh, ModelData, Skin};
use q0s_format::three::{Mat4, V3};

/// One polygon, easy to edit.
#[derive(Debug, Clone, PartialEq)]
pub struct Poly {
    pub verts: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    pub material: u16,
    pub smooth: bool,
}

pub fn polys(mesh: &Mesh) -> Vec<Poly> {
    mesh.faces
        .iter()
        .map(|face| {
            let start = face.start as usize;
            let end = start + face.len as usize;
            Poly {
                verts: mesh.corner_verts[start..end].to_vec(),
                uvs: if mesh.corner_uvs.len() == mesh.corner_verts.len() {
                    mesh.corner_uvs[start..end].to_vec()
                } else {
                    vec![[0.0, 0.0]; face.len as usize]
                },
                material: face.material,
                smooth: face.smooth,
            }
        })
        .collect()
}

/// Writes `polys` back into `mesh`, dropping imported normals.
pub fn set_polys(mesh: &mut Mesh, polys: Vec<Poly>) {
    let had_uvs = !mesh.corner_uvs.is_empty()
        || polys
            .iter()
            .any(|p| p.uvs.iter().any(|uv| *uv != [0.0, 0.0]));
    mesh.faces.clear();
    mesh.corner_verts.clear();
    mesh.corner_uvs.clear();
    mesh.corner_normals.clear();
    for poly in polys {
        if poly.verts.len() < 3 {
            continue;
        }
        let start = mesh.corner_verts.len() as u32;
        mesh.corner_verts.extend_from_slice(&poly.verts);
        if had_uvs {
            mesh.corner_uvs.extend_from_slice(&poly.uvs);
        }
        mesh.faces.push(Face {
            start,
            len: poly.verts.len() as u32,
            material: poly.material,
            smooth: poly.smooth,
        });
    }
}

fn centroid(mesh: &Mesh, verts: impl IntoIterator<Item = u32>) -> V3 {
    let mut sum = V3::ZERO;
    let mut n = 0.0;
    for v in verts {
        sum += mesh.positions[v as usize];
        n += 1.0;
    }
    if n > 0.0 {
        sum / n
    } else {
        V3::ZERO
    }
}

/// The vertices of these faces.
pub fn face_verts(mesh: &Mesh, faces: &BTreeSet<usize>) -> BTreeSet<u32> {
    faces
        .iter()
        .filter_map(|f| mesh.faces.get(*f))
        .flat_map(|f| mesh.face_verts(f).iter().copied())
        .collect()
}

/// Faces whose every vertex is in `verts`.
pub fn faces_of_verts(mesh: &Mesh, verts: &BTreeSet<u32>) -> BTreeSet<usize> {
    mesh.faces
        .iter()
        .enumerate()
        .filter(|(_, f)| mesh.face_verts(f).iter().all(|v| verts.contains(v)))
        .map(|(i, _)| i)
        .collect()
}

/// Every edge once, as sorted vertex pairs.
pub fn edges(mesh: &Mesh) -> Vec<[u32; 2]> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in &mesh.faces {
        let v = mesh.face_verts(f);
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            let key = [a.min(b), a.max(b)];
            if seen.insert(key) {
                out.push(key);
            }
        }
    }
    out
}

/// Moves vertices by a matrix (about whatever point it was built for).
pub fn transform_verts(mesh: &mut Mesh, verts: &BTreeSet<u32>, matrix: &Mat4) {
    for v in verts {
        if let Some(p) = mesh.positions.get_mut(*v as usize) {
            *p = matrix.transform_point(*p);
        }
    }
    mesh.corner_normals.clear();
}

/// Pulls each region of selected faces out along its normal by `distance`,
/// with new side walls. Returns the faces now on top.
pub fn extrude_faces(mesh: &mut Mesh, faces: &BTreeSet<usize>, distance: f32) -> BTreeSet<usize> {
    if faces.is_empty() {
        return BTreeSet::new();
    }
    let mut list = polys(mesh);
    // Edges used once inside the selection are its boundary.
    let mut edge_use: HashMap<(u32, u32), usize> = HashMap::new();
    for f in faces {
        let v = &list[*f].verts;
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            *edge_use.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let selected_verts: BTreeSet<u32> = faces
        .iter()
        .flat_map(|f| list[*f].verts.iter().copied())
        .collect();
    // Vertex normals of the region, for the push.
    let mut push: HashMap<u32, V3> = HashMap::new();
    for f in faces {
        let n = newell_normal(list[*f].verts.iter().map(|v| mesh.positions[*v as usize]));
        for v in &list[*f].verts {
            *push.entry(*v).or_insert(V3::ZERO) += n;
        }
    }
    let mut copy: HashMap<u32, u32> = HashMap::new();
    for v in &selected_verts {
        let p = mesh.positions[*v as usize];
        mesh.positions.push(p);
        if let Some(skin) = &mut mesh.skin {
            let (i, w) = (skin.influences[*v as usize], skin.weights[*v as usize]);
            skin.influences.push(i);
            skin.weights.push(w);
        }
        if !mesh.colors.is_empty() {
            let c = mesh.colors[*v as usize];
            mesh.colors.push(c);
        }
        copy.insert(*v, (mesh.positions.len() - 1) as u32);
    }
    let mut walls = Vec::new();
    for f in faces {
        let poly = list[*f].clone();
        let n = poly.verts.len();
        for i in 0..n {
            let (a, b) = (poly.verts[i], poly.verts[(i + 1) % n]);
            if edge_use[&(a.min(b), a.max(b))] == 1 {
                let (ua, ub) = (poly.uvs[i], poly.uvs[(i + 1) % n]);
                walls.push(Poly {
                    verts: vec![a, b, copy[&b], copy[&a]],
                    uvs: vec![ua, ub, ub, ua],
                    material: poly.material,
                    smooth: poly.smooth,
                });
            }
        }
        for v in &mut list[*f].verts {
            *v = copy[v];
        }
    }
    for (v, new) in &copy {
        let n = push[v].normalized();
        mesh.positions[*new as usize] += n * distance;
    }
    let first_wall = list.len();
    list.extend(walls);
    let _ = first_wall;
    set_polys(mesh, list);
    faces.clone()
}

/// Each face shrinks toward its middle by `amount` (0..1), with a ring of
/// new faces around it. Returns the inner faces.
pub fn inset_faces(mesh: &mut Mesh, faces: &BTreeSet<usize>, amount: f32) -> BTreeSet<usize> {
    let mut list = polys(mesh);
    let amount = amount.clamp(0.0, 0.95);
    let mut ring = Vec::new();
    for f in faces {
        let poly = list[*f].clone();
        let centre = centroid(mesh, poly.verts.iter().copied());
        let uv_centre = poly
            .uvs
            .iter()
            .fold([0.0, 0.0], |s, uv| [s[0] + uv[0], s[1] + uv[1]]);
        let uv_centre = [
            uv_centre[0] / poly.uvs.len() as f32,
            uv_centre[1] / poly.uvs.len() as f32,
        ];
        let mut inner = Vec::new();
        let mut inner_uvs = Vec::new();
        for (v, uv) in poly.verts.iter().zip(&poly.uvs) {
            let p = mesh.positions[*v as usize];
            mesh.positions.push(p + (centre - p) * amount);
            if let Some(skin) = &mut mesh.skin {
                let (i, w) = (skin.influences[*v as usize], skin.weights[*v as usize]);
                skin.influences.push(i);
                skin.weights.push(w);
            }
            if !mesh.colors.is_empty() {
                let c = mesh.colors[*v as usize];
                mesh.colors.push(c);
            }
            inner.push((mesh.positions.len() - 1) as u32);
            inner_uvs.push([
                uv[0] + (uv_centre[0] - uv[0]) * amount,
                uv[1] + (uv_centre[1] - uv[1]) * amount,
            ]);
        }
        let n = poly.verts.len();
        for i in 0..n {
            let j = (i + 1) % n;
            ring.push(Poly {
                verts: vec![poly.verts[i], poly.verts[j], inner[j], inner[i]],
                uvs: vec![poly.uvs[i], poly.uvs[j], inner_uvs[j], inner_uvs[i]],
                material: poly.material,
                smooth: poly.smooth,
            });
        }
        list[*f].verts = inner;
        list[*f].uvs = inner_uvs;
    }
    list.extend(ring);
    set_polys(mesh, list);
    faces.clone()
}

/// Removes faces, then any vertex no face uses.
pub fn delete_faces(mesh: &mut Mesh, faces: &BTreeSet<usize>) {
    let list: Vec<Poly> = polys(mesh)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !faces.contains(i))
        .map(|(_, p)| p)
        .collect();
    set_polys(mesh, list);
    remove_loose(mesh);
}

/// Removes vertices and every face that uses one.
pub fn delete_verts(mesh: &mut Mesh, verts: &BTreeSet<u32>) {
    let list: Vec<Poly> = polys(mesh)
        .into_iter()
        .filter(|p| !p.verts.iter().any(|v| verts.contains(v)))
        .collect();
    set_polys(mesh, list);
    remove_loose(mesh);
}

/// Drops vertices no face uses, renumbering the rest.
pub fn remove_loose(mesh: &mut Mesh) {
    let used: BTreeSet<u32> = mesh.corner_verts.iter().copied().collect();
    let mut map = vec![u32::MAX; mesh.positions.len()];
    let mut next = 0u32;
    for v in 0..mesh.positions.len() as u32 {
        if used.contains(&v) {
            map[v as usize] = next;
            next += 1;
        }
    }
    fn keep<T>(list: &mut Vec<T>, used: &BTreeSet<u32>) {
        let mut i = 0;
        list.retain(|_| {
            let k = used.contains(&(i as u32));
            i += 1;
            k
        });
    }
    keep(&mut mesh.positions, &used);
    if mesh.colors.len() == map.len() {
        keep(&mut mesh.colors, &used);
    }
    if let Some(skin) = &mut mesh.skin {
        if skin.influences.len() == map.len() {
            keep(&mut skin.influences, &used);
            keep(&mut skin.weights, &used);
        }
    }
    for v in &mut mesh.corner_verts {
        *v = map[*v as usize];
    }
}

/// Merges vertices into one at their middle.
pub fn merge_verts(mesh: &mut Mesh, verts: &BTreeSet<u32>) -> Option<u32> {
    let first = *verts.iter().next()?;
    let centre = centroid(mesh, verts.iter().copied());
    mesh.positions[first as usize] = centre;
    let mut list = polys(mesh);
    for poly in &mut list {
        for v in &mut poly.verts {
            if verts.contains(v) {
                *v = first;
            }
        }
        dedup_ring(poly);
    }
    set_polys(mesh, list);
    remove_loose(mesh);
    Some(first)
}

/// Removes repeated neighbours in a polygon's ring.
fn dedup_ring(poly: &mut Poly) {
    let mut i = 0;
    while poly.verts.len() > 1 && i < poly.verts.len() {
        let j = (i + 1) % poly.verts.len();
        if poly.verts[i] == poly.verts[j] {
            poly.verts.remove(j);
            poly.uvs.remove(j.min(poly.uvs.len() - 1));
        } else {
            i += 1;
        }
    }
}

/// Welds vertices closer than `distance`. Returns how many went.
pub fn merge_by_distance(mesh: &mut Mesh, distance: f32) -> usize {
    let cell = distance.max(1.0e-6);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut target: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    for (i, p) in mesh.positions.iter().enumerate() {
        let key = (
            (p.x / cell).floor() as i64,
            (p.y / cell).floor() as i64,
            (p.z / cell).floor() as i64,
        );
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&(key.0 + dx, key.1 + dy, key.2 + dz)) {
                        for other in list {
                            if mesh.positions[*other as usize].distance(*p) <= distance {
                                found = Some(*other);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        match found {
            Some(other) => target[i] = other,
            None => grid.entry(key).or_default().push(i as u32),
        }
    }
    let merged = target
        .iter()
        .enumerate()
        .filter(|(i, t)| *i as u32 != **t)
        .count();
    if merged == 0 {
        return 0;
    }
    let mut list = polys(mesh);
    for poly in &mut list {
        for v in &mut poly.verts {
            *v = target[*v as usize];
        }
        dedup_ring(poly);
    }
    set_polys(mesh, list);
    remove_loose(mesh);
    merged
}

/// Reverses faces, so their front turns the other way.
pub fn flip_faces(mesh: &mut Mesh, faces: &BTreeSet<usize>) {
    let mut list = polys(mesh);
    for f in faces {
        if let Some(p) = list.get_mut(*f) {
            p.verts.reverse();
            p.uvs.reverse();
        }
    }
    set_polys(mesh, list);
}

pub fn set_smooth(mesh: &mut Mesh, faces: &BTreeSet<usize>, smooth: bool) {
    for f in faces {
        if let Some(face) = mesh.faces.get_mut(*f) {
            face.smooth = smooth;
        }
    }
    mesh.corner_normals.clear();
}

pub fn set_material(mesh: &mut Mesh, faces: &BTreeSet<usize>, material: u16) {
    for f in faces {
        if let Some(face) = mesh.faces.get_mut(*f) {
            face.material = material;
        }
    }
}

/// A new face over the selected vertices, ordered around their middle.
pub fn fill(mesh: &mut Mesh, verts: &BTreeSet<u32>) -> Option<usize> {
    if verts.len() < 3 {
        return None;
    }
    let points: Vec<V3> = verts.iter().map(|v| mesh.positions[*v as usize]).collect();
    let centre = points.iter().fold(V3::ZERO, |s, p| s + *p) / points.len() as f32;
    // Best-fit plane normal from the spread of points.
    let mut normal = V3::ZERO;
    for i in 0..points.len() {
        normal += (points[i] - centre).cross(points[(i + 1) % points.len()] - centre);
    }
    if normal.length_sq() < 1.0e-12 {
        normal = newell_normal(points.iter().copied());
    }
    let normal = normal.normalized();
    let u = normal.any_perpendicular();
    let w = normal.cross(u);
    let mut ordered: Vec<(f32, u32)> = verts
        .iter()
        .map(|v| {
            let d = mesh.positions[*v as usize] - centre;
            (d.dot(w).atan2(d.dot(u)), *v)
        })
        .collect();
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let ring: Vec<u32> = ordered.into_iter().map(|(_, v)| v).collect();
    let mut list = polys(mesh);
    let material = list.first().map_or(0, |p| p.material);
    list.push(Poly {
        uvs: vec![[0.0, 0.0]; ring.len()],
        verts: ring,
        material,
        smooth: false,
    });
    let index = list.len() - 1;
    set_polys(mesh, list);
    Some(index)
}

/// Splits polygons into triangles.
pub fn triangulate(mesh: &mut Mesh, faces: &BTreeSet<usize>) {
    let mut out = Vec::new();
    for (i, poly) in polys(mesh).into_iter().enumerate() {
        if !faces.contains(&i) || poly.verts.len() <= 3 {
            out.push(poly);
            continue;
        }
        let points: Vec<V3> = poly
            .verts
            .iter()
            .map(|v| mesh.positions[*v as usize])
            .collect();
        let normal = newell_normal(points.iter().copied());
        for [a, b, c] in q0s_format::three::model::triangulate(&points, normal) {
            out.push(Poly {
                verts: vec![poly.verts[a], poly.verts[b], poly.verts[c]],
                uvs: vec![poly.uvs[a], poly.uvs[b], poly.uvs[c]],
                material: poly.material,
                smooth: poly.smooth,
            });
        }
    }
    set_polys(mesh, out);
}

/// Cuts every selected face into quads around its middle; with `smooth`,
/// moves points as Catmull-Clark subdivision does, rounding the shape.
pub fn subdivide(mesh: &mut Mesh, faces: &BTreeSet<usize>, smooth: bool) -> BTreeSet<usize> {
    let list = polys(mesh);
    let all = faces.len() == list.len();
    let original = mesh.positions.len();
    let old_positions = mesh.positions.clone();
    let mut edge_points: HashMap<(u32, u32), u32> = HashMap::new();
    let mut face_points: Vec<Option<u32>> = vec![None; list.len()];
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, p) in list.iter().enumerate() {
        for k in 0..p.verts.len() {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % p.verts.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let push_vertex = |mesh: &mut Mesh, p: V3, from: &[u32]| -> u32 {
        mesh.positions.push(p);
        if let Some(skin) = &mut mesh.skin {
            let mut pairs: HashMap<u16, f32> = HashMap::new();
            for v in from {
                for k in 0..4 {
                    *pairs.entry(skin.influences[*v as usize][k]).or_insert(0.0) +=
                        skin.weights[*v as usize][k] / from.len() as f32;
                }
            }
            let at = mesh.positions.len() - 1;
            skin.set_vertex(at, pairs.into_iter().collect());
        }
        if !mesh.colors.is_empty() {
            let mut sum = [0u32; 4];
            for v in from {
                let c = mesh.colors[*v as usize];
                for (s, x) in sum.iter_mut().zip([c.r, c.g, c.b, c.a]) {
                    *s += u32::from(x);
                }
            }
            let n = from.len().max(1) as u32;
            mesh.colors.push(q0s_format::v2::Rgba {
                r: (sum[0] / n) as u8,
                g: (sum[1] / n) as u8,
                b: (sum[2] / n) as u8,
                a: (sum[3] / n) as u8,
            });
        }
        (mesh.positions.len() - 1) as u32
    };
    for f in faces {
        let p = &list[*f];
        let c = p
            .verts
            .iter()
            .fold(V3::ZERO, |s, v| s + old_positions[*v as usize])
            / p.verts.len() as f32;
        face_points[*f] = Some(push_vertex(mesh, c, &p.verts));
    }
    let mut out = Vec::new();
    let mut new_faces = BTreeSet::new();
    for (i, p) in list.into_iter().enumerate() {
        let Some(centre) = face_points[i] else {
            out.push(p);
            continue;
        };
        let n = p.verts.len();
        let uv_centre = p.uvs.iter().fold([0.0, 0.0], |s, uv| {
            [s[0] + uv[0] / n as f32, s[1] + uv[1] / n as f32]
        });
        let mut mids = Vec::with_capacity(n);
        let mut mid_uvs = Vec::with_capacity(n);
        for k in 0..n {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
            let key = (a.min(b), a.max(b));
            let mid = match edge_points.get(&key) {
                Some(m) => *m,
                None => {
                    let pos = (old_positions[a as usize] + old_positions[b as usize]) * 0.5;
                    let m = push_vertex(mesh, pos, &[a, b]);
                    edge_points.insert(key, m);
                    m
                }
            };
            mids.push(mid);
            let (ua, ub) = (p.uvs[k], p.uvs[(k + 1) % n]);
            mid_uvs.push([(ua[0] + ub[0]) * 0.5, (ua[1] + ub[1]) * 0.5]);
        }
        for k in 0..n {
            let prev = (k + n - 1) % n;
            new_faces.insert(out.len());
            out.push(Poly {
                verts: vec![p.verts[k], mids[k], centre, mids[prev]],
                uvs: vec![p.uvs[k], mid_uvs[k], uv_centre, mid_uvs[prev]],
                material: p.material,
                smooth: p.smooth || smooth,
            });
        }
    }
    if smooth && all {
        // Catmull-Clark: edge points to the average of their ends and the
        // face points beside them; old points to (F + 2R + (n-3)P) / n.
        let face_point = |f: usize| face_points[f].map(|v| mesh.positions[v as usize]);
        let mut edge_targets = Vec::new();
        for ((a, b), m) in &edge_points {
            let beside = &edge_faces[&(*a, *b)];
            if beside.len() == 2 {
                let (f0, f1) = (face_point(beside[0]), face_point(beside[1]));
                if let (Some(f0), Some(f1)) = (f0, f1) {
                    let p =
                        (old_positions[*a as usize] + old_positions[*b as usize] + f0 + f1) * 0.25;
                    edge_targets.push((*m, p));
                }
            }
        }
        let mut vertex_faces: Vec<Vec<usize>> = vec![Vec::new(); original];
        let mut vertex_edges: Vec<Vec<u32>> = vec![Vec::new(); original];
        for ((a, b), fs) in &edge_faces {
            vertex_edges[*a as usize].push(*b);
            vertex_edges[*b as usize].push(*a);
            for f in fs {
                for v in [*a, *b] {
                    if !vertex_faces[v as usize].contains(f) {
                        vertex_faces[v as usize].push(*f);
                    }
                }
            }
        }
        let mut moved = Vec::new();
        for v in 0..original {
            let fs = &vertex_faces[v];
            let es = &vertex_edges[v];
            let boundary = es
                .iter()
                .any(|o| edge_faces[&((v as u32).min(*o), (v as u32).max(*o))].len() != 2);
            if fs.is_empty() || boundary {
                continue;
            }
            let n = fs.len() as f32;
            let f = fs
                .iter()
                .filter_map(|f| face_point(*f))
                .fold(V3::ZERO, |s, p| s + p)
                / n;
            let r = es
                .iter()
                .map(|o| (old_positions[v] + old_positions[*o as usize]) * 0.5)
                .fold(V3::ZERO, |s, p| s + p)
                / es.len() as f32;
            moved.push((v, (f + r * 2.0 + old_positions[v] * (n - 3.0)) / n));
        }
        for (m, p) in edge_targets {
            mesh.positions[m as usize] = p;
        }
        for (v, p) in moved {
            mesh.positions[v] = p;
        }
    }
    set_polys(mesh, out);
    new_faces
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn unit(self) -> V3 {
        match self {
            Self::X => V3::X,
            Self::Y => V3::Y,
            Self::Z => V3::Z,
        }
    }
}

/// Adds a mirrored copy across the plane through the origin, welding
/// points that lie on it.
pub fn mirror(mesh: &mut Mesh, axis: Axis) {
    let flip = |p: V3| match axis {
        Axis::X => V3::new(-p.x, p.y, p.z),
        Axis::Y => V3::new(p.x, -p.y, p.z),
        Axis::Z => V3::new(p.x, p.y, -p.z),
    };
    let count = mesh.positions.len() as u32;
    let mut list = polys(mesh);
    let copies: Vec<V3> = mesh.positions.iter().map(|p| flip(*p)).collect();
    mesh.positions.extend(copies);
    if !mesh.colors.is_empty() {
        let c = mesh.colors.clone();
        mesh.colors.extend(c);
    }
    if let Some(skin) = &mut mesh.skin {
        let (i, w) = (skin.influences.clone(), skin.weights.clone());
        skin.influences.extend(i);
        skin.weights.extend(w);
    }
    let mirrored: Vec<Poly> = list
        .iter()
        .map(|p| Poly {
            verts: p.verts.iter().rev().map(|v| v + count).collect(),
            uvs: p.uvs.iter().rev().copied().collect(),
            material: p.material,
            smooth: p.smooth,
        })
        .collect();
    list.extend(mirrored);
    set_polys(mesh, list);
    merge_by_distance(mesh, 1.0e-5);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// Each face from the side it faces most.
    Box,
    /// Straight down from the front.
    Front,
    /// Around the vertical axis.
    Cylinder,
}

/// New texture positions for faces, fitted to 0..1 over the selection.
pub fn project_uvs(mesh: &mut Mesh, faces: &BTreeSet<usize>, projection: Projection) {
    let Some((lo, hi)) = mesh.bounds() else {
        return;
    };
    let size = (hi - lo).max(V3::splat(1.0e-6));
    let mut list = polys(mesh);
    for f in faces {
        let Some(p) = list.get_mut(*f) else { continue };
        let normal = newell_normal(p.verts.iter().map(|v| mesh.positions[*v as usize]));
        p.uvs = p
            .verts
            .iter()
            .map(|v| {
                let q = (mesh.positions[*v as usize] - lo).mul_elem(V3::new(
                    1.0 / size.x,
                    1.0 / size.y,
                    1.0 / size.z,
                ));
                match projection {
                    Projection::Front => [q.x, 1.0 - q.y],
                    Projection::Cylinder => {
                        let c = mesh.positions[*v as usize] - (lo + hi) * 0.5;
                        [0.5 + c.x.atan2(c.z) / std::f32::consts::TAU, 1.0 - q.y]
                    }
                    Projection::Box => {
                        let (ax, ay, az) = (normal.x.abs(), normal.y.abs(), normal.z.abs());
                        if ax >= ay && ax >= az {
                            [if normal.x > 0.0 { 1.0 - q.z } else { q.z }, 1.0 - q.y]
                        } else if ay >= az {
                            [q.x, if normal.y > 0.0 { q.z } else { 1.0 - q.z }]
                        } else {
                            [if normal.z > 0.0 { q.x } else { 1.0 - q.x }, 1.0 - q.y]
                        }
                    }
                }
            })
            .collect();
    }
    set_polys(mesh, list);
}

/// Turns faces so neighbours agree, then so closed parts face outward.
pub fn fix_normals(mesh: &mut Mesh) {
    let mut list = polys(mesh);
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, p) in list.iter().enumerate() {
        for k in 0..p.verts.len() {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % p.verts.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let has_edge = |p: &Poly, a: u32, b: u32| {
        (0..p.verts.len()).any(|k| p.verts[k] == a && p.verts[(k + 1) % p.verts.len()] == b)
    };
    let mut done = vec![false; list.len()];
    for start in 0..list.len() {
        if done[start] {
            continue;
        }
        let mut part = vec![start];
        let mut queue = vec![start];
        done[start] = true;
        while let Some(f) = queue.pop() {
            let verts = list[f].verts.clone();
            for k in 0..verts.len() {
                let (a, b) = (verts[k], verts[(k + 1) % verts.len()]);
                for other in &edge_faces[&(a.min(b), a.max(b))] {
                    if done[*other] {
                        continue;
                    }
                    // A neighbour that agrees runs the shared edge b->a.
                    if has_edge(&list[*other], a, b) {
                        list[*other].verts.reverse();
                        list[*other].uvs.reverse();
                    }
                    done[*other] = true;
                    part.push(*other);
                    queue.push(*other);
                }
            }
        }
        // Signed volume says whether the part faces in.
        let mut volume = 0.0;
        for f in &part {
            let p = &list[*f];
            let a = mesh.positions[p.verts[0] as usize];
            for k in 1..p.verts.len() - 1 {
                let b = mesh.positions[p.verts[k] as usize];
                let c = mesh.positions[p.verts[k + 1] as usize];
                volume += a.dot(b.cross(c));
            }
        }
        if volume < 0.0 {
            for f in &part {
                list[*f].verts.reverse();
                list[*f].uvs.reverse();
            }
        }
    }
    set_polys(mesh, list);
}

/// Faces joined to these through shared edges.
pub fn linked_faces(mesh: &Mesh, seeds: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut by_vertex: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, f) in mesh.faces.iter().enumerate() {
        for v in mesh.face_verts(f) {
            by_vertex.entry(*v).or_default().push(i);
        }
    }
    let mut out = seeds.clone();
    let mut queue: Vec<usize> = seeds.iter().copied().collect();
    while let Some(f) = queue.pop() {
        for v in mesh.face_verts(&mesh.faces[f]) {
            for other in &by_vertex[v] {
                if out.insert(*other) {
                    queue.push(*other);
                }
            }
        }
    }
    out
}

/// Copies faces (with their own new vertices). Returns the copies.
pub fn duplicate_faces(mesh: &mut Mesh, faces: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut list = polys(mesh);
    let mut copy: HashMap<u32, u32> = HashMap::new();
    let mut new_faces = BTreeSet::new();
    for f in faces {
        let mut p = list[*f].clone();
        for v in &mut p.verts {
            let original = *v;
            *v = *copy.entry(original).or_insert_with(|| {
                mesh.positions.push(mesh.positions[original as usize]);
                if !mesh.colors.is_empty() {
                    mesh.colors.push(mesh.colors[original as usize]);
                }
                if let Some(skin) = &mut mesh.skin {
                    skin.influences.push(skin.influences[original as usize]);
                    skin.weights.push(skin.weights[original as usize]);
                }
                (mesh.positions.len() - 1) as u32
            });
        }
        new_faces.insert(list.len());
        list.push(p);
    }
    set_polys(mesh, list);
    new_faces
}

/// Bone weights from distance: each vertex leans on the bones nearest it,
/// fading over a reach that grows with the bone's length. Replaces any
/// skin the mesh had. `bones` are node indices.
pub fn auto_weights(model: &mut ModelData, mesh_index: usize, bones: &[u16]) {
    let world = model.rest_world();
    let Some(mesh) = model.meshes.get(mesh_index) else {
        return;
    };
    let mesh_world = q0s_format::three::model::mesh_matrix(mesh, &world);
    let segments: Vec<(u16, V3, V3, f32)> = bones
        .iter()
        .filter_map(|b| {
            let node = model.nodes.get(usize::from(*b))?;
            let m = world[usize::from(*b)];
            let head = m.translation_part();
            let tail = m.transform_point(V3::new(0.0, node.length.max(1.0e-4), 0.0));
            Some((*b, head, tail, node.length.max(1.0e-4)))
        })
        .collect();
    if segments.is_empty() {
        return;
    }
    let mut skin = Skin::default();
    for (b, _, _, _) in &segments {
        skin.joints.push(*b);
        skin.inverse_bind
            .push(world[usize::from(*b)].inverse().unwrap_or(Mat4::IDENTITY) * mesh_world);
    }
    let positions: Vec<V3> = mesh
        .positions
        .iter()
        .map(|p| mesh_world.transform_point(*p))
        .collect();
    for (v, p) in positions.iter().enumerate() {
        let mut pairs: Vec<(u16, f32)> = segments
            .iter()
            .enumerate()
            .map(|(j, (_, head, tail, length))| {
                let d = distance_to_segment(*p, *head, *tail);
                let reach = length * 0.6;
                (j as u16, 1.0 / (1.0e-4 + (d / reach).powi(4)))
            })
            .collect();
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(4);
        skin.set_vertex(v, pairs);
    }
    // Skinned positions now come from the joints: mesh space is the bind.
    let mesh = &mut model.meshes[mesh_index];
    mesh.skin = Some(skin);
}

/// The ring of edges across quads from `edge`, each oriented the same way
/// (its first point on the same side of the ring), and the quads between
/// them. Stops at a face that is not a quad, at the mesh's edge, or where
/// the ring closes on itself.
pub fn edge_ring(mesh: &Mesh, edge: [u32; 2]) -> (Vec<[u32; 2]>, Vec<usize>) {
    let list = polys(mesh);
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (f, p) in list.iter().enumerate() {
        for k in 0..p.verts.len() {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % p.verts.len()]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(f);
        }
    }
    let key = |e: [u32; 2]| (e[0].min(e[1]), e[0].max(e[1]));
    if !by_edge.contains_key(&key(edge)) {
        return (Vec::new(), Vec::new());
    }
    // Across quad `f` from oriented edge `e`: the opposite edge, oriented so
    // its first point sits beside `e`'s first point.
    let across = |f: usize, e: [u32; 2]| -> Option<[u32; 2]> {
        let v = &list[f].verts;
        if v.len() != 4 {
            return None;
        }
        let i = v.iter().position(|x| *x == e[0])?;
        let next = v[(i + 1) % 4];
        let prev = v[(i + 3) % 4];
        if next == e[1] {
            Some([prev, v[(i + 2) % 4]])
        } else if prev == e[1] {
            Some([next, v[(i + 2) % 4]])
        } else {
            None
        }
    };
    let mut edges = vec![edge];
    let mut faces = Vec::new();
    let mut seen_faces = HashSet::new();
    let mut closed = false;
    // One way from the start edge, then the other.
    for first_side in [true, false] {
        let mut current = edge;
        let start_faces = by_edge[&key(edge)].clone();
        let Some(&start) = start_faces.get(if first_side { 0 } else { 1 }) else {
            continue;
        };
        let mut face = start;
        loop {
            if seen_faces.contains(&face) {
                closed = true;
                break;
            }
            let Some(next) = across(face, current) else {
                break;
            };
            seen_faces.insert(face);
            if first_side {
                faces.push(face);
                edges.push(next);
            } else {
                faces.insert(0, face);
                edges.insert(0, next);
            }
            if key(next) == key(edge) {
                closed = true;
                if first_side {
                    edges.pop();
                } else {
                    edges.remove(0);
                }
                break;
            }
            let came_from = Some(face);
            let Some(other) = by_edge[&key(next)]
                .iter()
                .copied()
                .find(|f| Some(*f) != came_from)
            else {
                break;
            };
            current = next;
            face = other;
        }
        if closed {
            break;
        }
    }
    (edges, faces)
}

/// Cuts a new loop of edges across the ring of quads through `edge`, at
/// `t` (0..1) along each crossed edge. Returns the new points.
pub fn loop_cut(mesh: &mut Mesh, edge: [u32; 2], t: f32) -> BTreeSet<u32> {
    let (ring, quads) = edge_ring(mesh, edge);
    if quads.is_empty() {
        return BTreeSet::new();
    }
    let t = t.clamp(0.02, 0.98);
    let mut list = polys(mesh);
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    let mut fresh = BTreeSet::new();
    for e in &ring {
        let key = (e[0].min(e[1]), e[0].max(e[1]));
        if cut.contains_key(&key) {
            continue;
        }
        let p = mesh.positions[e[0] as usize].lerp(mesh.positions[e[1] as usize], t);
        mesh.positions.push(p);
        let id = (mesh.positions.len() - 1) as u32;
        if !mesh.colors.is_empty() {
            let c = mesh.colors[e[0] as usize];
            mesh.colors.push(c);
        }
        if let Some(skin) = &mut mesh.skin {
            let (a, b) = (e[0] as usize, e[1] as usize);
            let mut pairs: HashMap<u16, f32> = HashMap::new();
            for k in 0..4 {
                *pairs.entry(skin.influences[a][k]).or_insert(0.0) +=
                    skin.weights[a][k] * (1.0 - t);
                *pairs.entry(skin.influences[b][k]).or_insert(0.0) += skin.weights[b][k] * t;
            }
            let at = mesh.positions.len() - 1;
            skin.set_vertex(at, pairs.into_iter().collect());
        }
        cut.insert(key, id);
        fresh.insert(id);
    }
    // The point on an edge, and its texture position, `t` from `from`.
    let ring_point =
        |from: u32, to: u32| -> Option<u32> { cut.get(&(from.min(to), from.max(to))).copied() };
    let quad_set: HashSet<usize> = quads.iter().copied().collect();
    let mut out = Vec::with_capacity(list.len() + quads.len());
    for (f, poly) in list.drain(..).enumerate() {
        if quad_set.contains(&f) {
            // Two of the quad's edges are cut: split it between them.
            let v = &poly.verts;
            let k = (0..4).find(|k| {
                ring_point(v[*k], v[(k + 1) % 4]).is_some()
                    && ring_point(v[(k + 2) % 4], v[(k + 3) % 4]).is_some()
            });
            let Some(k) = k else {
                out.push(poly);
                continue;
            };
            let (p0, p1, p2, p3) = (v[k], v[(k + 1) % 4], v[(k + 2) % 4], v[(k + 3) % 4]);
            let (u0, u1, u2, u3) = (
                poly.uvs[k],
                poly.uvs[(k + 1) % 4],
                poly.uvs[(k + 2) % 4],
                poly.uvs[(k + 3) % 4],
            );
            let a = ring_point(p0, p1).unwrap();
            let b = ring_point(p2, p3).unwrap();
            // Texture positions follow where the points really fell.
            let along = |from: u32, to: u32, uf: [f32; 2], ut: [f32; 2]| {
                let pf = mesh.positions[from as usize];
                let pt = mesh.positions[to as usize];
                let pm = mesh.positions[ring_point(from, to).unwrap() as usize];
                let s = if (pt - pf).length_sq() > 0.0 {
                    (pm - pf).length() / (pt - pf).length()
                } else {
                    0.5
                };
                [uf[0] + (ut[0] - uf[0]) * s, uf[1] + (ut[1] - uf[1]) * s]
            };
            let ua = along(p0, p1, u0, u1);
            let ub = along(p3, p2, u3, u2);
            out.push(Poly {
                verts: vec![p0, a, b, p3],
                uvs: vec![u0, ua, ub, u3],
                material: poly.material,
                smooth: poly.smooth,
            });
            out.push(Poly {
                verts: vec![a, p1, p2, b],
                uvs: vec![ua, u1, u2, ub],
                material: poly.material,
                smooth: poly.smooth,
            });
        } else {
            // A neighbour on a cut edge takes the new point too, so the
            // surface stays closed.
            let mut verts = Vec::with_capacity(poly.verts.len() + 2);
            let mut uvs = Vec::with_capacity(poly.verts.len() + 2);
            for k in 0..poly.verts.len() {
                let (a, b) = (poly.verts[k], poly.verts[(k + 1) % poly.verts.len()]);
                verts.push(a);
                uvs.push(poly.uvs[k]);
                if let Some(m) = ring_point(a, b) {
                    let ub = poly.uvs[(k + 1) % poly.verts.len()];
                    verts.push(m);
                    uvs.push([
                        (poly.uvs[k][0] + ub[0]) * 0.5,
                        (poly.uvs[k][1] + ub[1]) * 0.5,
                    ]);
                }
            }
            out.push(Poly { verts, uvs, ..poly });
        }
    }
    set_polys(mesh, out);
    fresh
}

pub fn distance_to_segment(p: V3, a: V3, b: V3) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t).distance(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim::Primitive;

    fn closed(mesh: &Mesh) -> bool {
        let mut edges = HashMap::new();
        for f in &mesh.faces {
            let v = mesh.face_verts(f);
            for i in 0..v.len() {
                let (a, b) = (v[i], v[(i + 1) % v.len()]);
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        edges.values().all(|n| *n == 2)
    }

    #[test]
    fn extruding_a_cube_face_keeps_it_closed() {
        let mut cube = Primitive::Cube.build(2.0, 4);
        let top = (0..cube.faces.len())
            .find(|f| cube.face_normal(&cube.faces[*f]).y > 0.9)
            .unwrap();
        let before = cube.faces.len();
        extrude_faces(&mut cube, &BTreeSet::from([top]), 1.0);
        assert_eq!(cube.faces.len(), before + 4);
        assert!(closed(&cube));
        let highest = cube.positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((highest - 2.0).abs() < 1.0e-4, "{highest}");
    }

    #[test]
    fn inset_and_subdivide_keep_faces_and_shape() {
        let mut cube = Primitive::Cube.build(2.0, 4);
        inset_faces(&mut cube, &BTreeSet::from([0]), 0.3);
        assert_eq!(cube.faces.len(), 6 + 4);
        assert!(closed(&cube));
        let mut cube = Primitive::Cube.build(2.0, 4);
        let all: BTreeSet<usize> = (0..6).collect();
        subdivide(&mut cube, &all, true);
        assert_eq!(cube.faces.len(), 24);
        assert!(closed(&cube));
        // Smoothed corners come in from the cube's.
        let far = cube
            .positions
            .iter()
            .map(|p| p.length())
            .fold(0.0, f32::max);
        assert!(far < 3f32.sqrt() - 0.1, "{far}");
    }

    #[test]
    fn merging_and_mirroring_weld_points() {
        let mut plane = Primitive::Plane.build(2.0, 4);
        // Move it so one edge lies on x = 0, then mirror.
        for p in &mut plane.positions {
            p.x += 1.0;
        }
        mirror(&mut plane, Axis::X);
        assert_eq!(plane.faces.len(), 2);
        assert_eq!(plane.positions.len(), 6);
        let removed = merge_by_distance(&mut plane, 0.01);
        assert_eq!(removed, 0);
    }

    #[test]
    fn a_loop_cut_rings_a_cube() {
        let mut cube = Primitive::Cube.build(2.0, 4);
        let top = (0..cube.faces.len())
            .find(|f| cube.face_normal(&cube.faces[*f]).y > 0.9)
            .unwrap();
        let v = cube.face_verts(&cube.faces[top]).to_vec();
        let (ring, quads) = edge_ring(&cube, [v[0], v[1]]);
        assert_eq!(quads.len(), 4, "around four sides");
        assert_eq!(ring.len(), 4);
        let fresh = loop_cut(&mut cube, [v[0], v[1]], 0.5);
        assert_eq!(fresh.len(), 4);
        assert_eq!(cube.faces.len(), 10);
        assert!(closed(&cube));
        assert!(cube
            .faces
            .iter()
            .all(|f| cube.face_normal(f).dot(cube.face_center(f)) > 0.0));
    }

    #[test]
    fn a_loop_cut_stops_at_the_edge_of_a_strip() {
        // Two quads side by side.
        let mut mesh = Mesh {
            positions: vec![
                V3::new(0.0, 0.0, 0.0),
                V3::new(1.0, 0.0, 0.0),
                V3::new(2.0, 0.0, 0.0),
                V3::new(0.0, 1.0, 0.0),
                V3::new(1.0, 1.0, 0.0),
                V3::new(2.0, 1.0, 0.0),
            ],
            ..Mesh::default()
        };
        mesh.push_face(&[0, 1, 4, 3], None, 0, false);
        mesh.push_face(&[1, 2, 5, 4], None, 0, false);
        let fresh = loop_cut(&mut mesh, [0, 3], 0.5);
        assert_eq!(fresh.len(), 3, "across both quads");
        assert_eq!(mesh.faces.len(), 4);
    }

    #[test]
    fn flipped_faces_are_turned_back_out() {
        let mut cube = Primitive::Cube.build(2.0, 4);
        flip_faces(&mut cube, &BTreeSet::from([0, 3]));
        fix_normals(&mut cube);
        assert!(cube
            .faces
            .iter()
            .all(|f| cube.face_normal(f).dot(cube.face_center(f)) > 0.0));
    }

    #[test]
    fn weights_follow_the_nearest_bone() {
        let mut model = ModelData::default();
        let mut a = q0s_format::three::Node::new("a");
        a.bone = true;
        a.length = 1.0;
        let mut b = q0s_format::three::Node::new("b");
        b.bone = true;
        b.length = 1.0;
        b.parent = Some(0);
        b.translation = V3::new(0.0, 1.0, 0.0);
        model.nodes = vec![a, b];
        let mut mesh = Primitive::Capsule.build(2.0, 6);
        for p in &mut mesh.positions {
            p.y += 1.0;
            p.x *= 0.3;
            p.z *= 0.3;
        }
        model.meshes.push(mesh);
        auto_weights(&mut model, 0, &[0, 1]);
        let skin = model.meshes[0].skin.as_ref().unwrap();
        let low = (0..model.meshes[0].positions.len())
            .min_by(|x, y| {
                model.meshes[0].positions[*x]
                    .y
                    .total_cmp(&model.meshes[0].positions[*y].y)
            })
            .unwrap();
        assert_eq!(skin.joints[usize::from(skin.influences[low][0])], 0);
        assert!(skin.weights[low][0] > 0.9);
    }
}
