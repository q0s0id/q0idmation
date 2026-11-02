//! A smooth surface over a liquid's drops, built the same way in every
//! renderer: the drops add up to a field, and naive surface nets lay a skin
//! where the field crosses a level. Normals come from the field's slope, so
//! the skin shades round even where its faces are coarse.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::draw::{DrawItem, MeshDraw};
use super::math::{Mat4, V3};
use super::model::{Material, Mesh, ModelData, RenderMesh, Shading};
use super::scene::Liquid3d;

/// Where the skin lies: a lone drop's field peaks at 1.
const LEVEL: f32 = 0.35;
/// How far a drop reaches, in its spacing.
const REACH: f32 = 1.6;
/// The grid never gets finer than this many cells across.
const MAX_CELLS: f32 = 96.0;
/// Surfaces kept for drawing again.
const CACHE: usize = 96;

/// The skin over `drops`, which lie `spacing` apart, in stage space.
pub fn surface(drops: &[[i16; 3]], spacing: f32) -> Mesh {
    let mut mesh = Mesh {
        name: "liquid".into(),
        ..Default::default()
    };
    if drops.is_empty() {
        return mesh;
    }
    let reach = spacing.max(1.0) * REACH;
    let mut lo = V3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = V3::new(f32::MIN, f32::MIN, f32::MIN);
    for d in drops {
        let p = V3::new(f32::from(d[0]), f32::from(d[1]), f32::from(d[2]));
        lo = lo.min(p);
        hi = hi.max(p);
    }
    lo -= V3::new(reach, reach, reach);
    hi += V3::new(reach, reach, reach);
    let size = hi - lo;
    let cell = (spacing * 0.5)
        .max(size.x.max(size.y).max(size.z) / MAX_CELLS)
        .max(0.5);
    let n = [
        (size.x / cell).ceil() as usize + 1,
        (size.y / cell).ceil() as usize + 1,
        (size.z / cell).ceil() as usize + 1,
    ];
    let index = |i: usize, j: usize, k: usize| (k * n[1] + j) * n[0] + i;
    let mut field = vec![0.0f32; n[0] * n[1] * n[2]];
    let span = (reach / cell).ceil() as i64;
    for d in drops {
        let p = V3::new(f32::from(d[0]), f32::from(d[1]), f32::from(d[2])) - lo;
        let (ci, cj, ck) = (
            (p.x / cell) as i64,
            (p.y / cell) as i64,
            (p.z / cell) as i64,
        );
        for k in (ck - span).max(0)..=(ck + span).min(n[2] as i64 - 1) {
            for j in (cj - span).max(0)..=(cj + span).min(n[1] as i64 - 1) {
                for i in (ci - span).max(0)..=(ci + span).min(n[0] as i64 - 1) {
                    let at = V3::new(i as f32, j as f32, k as f32) * cell;
                    let r2 = (at - p).length_sq() / (reach * reach);
                    if r2 < 1.0 {
                        let w = 1.0 - r2;
                        field[index(i as usize, j as usize, k as usize)] += w * w * w;
                    }
                }
            }
        }
    }
    let value = |i: usize, j: usize, k: usize| field[index(i, j, k)];
    let point = |i: usize, j: usize, k: usize| lo + V3::new(i as f32, j as f32, k as f32) * cell;

    // A vertex in every cell the skin passes through, at the middle of
    // where it crosses the cell's edges.
    let cells = [n[0] - 1, n[1] - 1, n[2] - 1];
    let cell_index = |i: usize, j: usize, k: usize| (k * cells[1] + j) * cells[0] + i;
    let mut vertex_of = vec![u32::MAX; cells[0] * cells[1] * cells[2]];
    const CORNERS: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [0, 1, 0],
        [1, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [0, 1, 1],
        [1, 1, 1],
    ];
    const EDGES: [[usize; 2]; 12] = [
        [0, 1],
        [2, 3],
        [4, 5],
        [6, 7],
        [0, 2],
        [1, 3],
        [4, 6],
        [5, 7],
        [0, 4],
        [1, 5],
        [2, 6],
        [3, 7],
    ];
    let mut normals = Vec::new();
    for k in 0..cells[2] {
        for j in 0..cells[1] {
            for i in 0..cells[0] {
                let v = CORNERS.map(|[a, b, c]| value(i + a, j + b, k + c));
                let inside = v.iter().filter(|value| **value >= LEVEL).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = V3::ZERO;
                let mut crossings = 0.0;
                for [a, b] in EDGES {
                    if (v[a] >= LEVEL) != (v[b] >= LEVEL) {
                        let t = (LEVEL - v[a]) / (v[b] - v[a]);
                        let pa = point(i + CORNERS[a][0], j + CORNERS[a][1], k + CORNERS[a][2]);
                        let pb = point(i + CORNERS[b][0], j + CORNERS[b][1], k + CORNERS[b][2]);
                        sum += pa + (pb - pa) * t;
                        crossings += 1.0;
                    }
                }
                // The field falls outward: its slope points in.
                let slope = V3::new(
                    (v[1] + v[3] + v[5] + v[7]) - (v[0] + v[2] + v[4] + v[6]),
                    (v[2] + v[3] + v[6] + v[7]) - (v[0] + v[1] + v[4] + v[5]),
                    (v[4] + v[5] + v[6] + v[7]) - (v[0] + v[1] + v[2] + v[3]),
                );
                vertex_of[cell_index(i, j, k)] = mesh.positions.len() as u32;
                mesh.positions.push(sum / crossings);
                normals.push(-slope.normalized());
            }
        }
    }
    // A quad across every grid edge the skin crosses, joining the four
    // cells around it, facing out of the liquid.
    let quad = |cells_around: [(usize, usize, usize); 4], flip: bool, mesh: &mut Mesh| {
        let mut ids = [0u32; 4];
        for (slot, (i, j, k)) in cells_around.iter().enumerate() {
            let id = vertex_of[cell_index(*i, *j, *k)];
            if id == u32::MAX {
                return;
            }
            ids[slot] = id;
        }
        if flip {
            ids.reverse();
        }
        mesh.push_face(&ids, None, 0, true);
    };
    // Each quad's corners go round its edge's axis, so it faces along the
    // axis; it is turned over when the outside lies the other way.
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                let here = value(i, j, k) >= LEVEL;
                let (inner_i, inner_j, inner_k) = (
                    (1..cells[0]).contains(&i),
                    (1..cells[1]).contains(&j),
                    (1..cells[2]).contains(&k),
                );
                if i + 1 < n[0] && inner_j && inner_k && here != (value(i + 1, j, k) >= LEVEL) {
                    quad(
                        [(i, j - 1, k - 1), (i, j, k - 1), (i, j, k), (i, j - 1, k)],
                        !here,
                        &mut mesh,
                    );
                }
                if j + 1 < n[1] && inner_i && inner_k && here != (value(i, j + 1, k) >= LEVEL) {
                    quad(
                        [(i - 1, j, k - 1), (i - 1, j, k), (i, j, k), (i, j, k - 1)],
                        !here,
                        &mut mesh,
                    );
                }
                if k + 1 < n[2] && inner_i && inner_j && here != (value(i, j, k + 1) >= LEVEL) {
                    quad(
                        [(i - 1, j - 1, k), (i, j - 1, k), (i, j, k), (i - 1, j, k)],
                        !here,
                        &mut mesh,
                    );
                }
            }
        }
    }
    mesh.corner_normals = mesh
        .corner_verts
        .iter()
        .map(|v| normals[*v as usize])
        .collect();
    mesh
}

type Built = (Arc<ModelData>, Arc<RenderMesh>);

fn cache() -> &'static Mutex<HashMap<u64, Built>> {
    static CACHE_MAP: OnceLock<Mutex<HashMap<u64, Built>>> = OnceLock::new();
    CACHE_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key_of(liquid: &Liquid3d, drops: &[[i16; 3]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(&liquid.spacing.to_le_bytes());
    eat(&[
        liquid.color.r,
        liquid.color.g,
        liquid.color.b,
        liquid.color.a,
        u8::from(liquid.clear),
    ]);
    eat(&(drops.len() as u64).to_le_bytes());
    for d in drops {
        for axis in d {
            eat(&axis.to_le_bytes());
        }
    }
    hash
}

/// The material a liquid is drawn in.
pub fn material_of(liquid: &Liquid3d) -> Material {
    let mut material = Material {
        name: liquid.name.clone(),
        color: liquid.color,
        double_sided: true,
        roughness: if liquid.clear { 0.05 } else { 0.35 },
        ..Material::default()
    };
    if liquid.clear {
        material.shading = Shading::Glass;
    } else {
        material.shading = Shading::Lit;
        material.color.a = 255;
    }
    material
}

/// The liquid on `frame`, ready to draw with the 3D items, if any of it is
/// there.
pub fn liquid_item(liquid: &Liquid3d, frame: u16) -> Option<DrawItem> {
    let drops = liquid.drops_at(frame)?;
    if drops.is_empty() {
        return None;
    }
    let key = key_of(liquid, drops);
    let built = cache().lock().ok().and_then(|map| map.get(&key).cloned());
    let (model, render) = match built {
        Some(built) => built,
        None => {
            let mesh = surface(drops, liquid.spacing);
            if mesh.faces.is_empty() {
                return None;
            }
            let render = Arc::new(RenderMesh::build(&mesh));
            let model = Arc::new(ModelData {
                meshes: vec![mesh],
                materials: vec![material_of(liquid)],
                scale: 1.0,
                ..ModelData::default()
            });
            if let Ok(mut map) = cache().lock() {
                if map.len() >= CACHE {
                    map.clear();
                }
                map.insert(key, (model.clone(), render.clone()));
            }
            (model, render)
        }
    };
    Some(DrawItem::Mesh(MeshDraw {
        model,
        mesh: 0,
        render,
        posed: None,
        matrix: Mat4::IDENTITY,
        opacity: 1.0,
        lit: true,
        outline: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_drop_gets_a_round_skin() {
        let mesh = surface(&[[0, 0, 0]], 20.0);
        assert!(mesh.faces.len() > 20, "{} faces", mesh.faces.len());
        // Every vertex sits about a drop's spacing from its middle, with
        // its normal pointing away from it.
        for (index, p) in mesh.positions.iter().enumerate() {
            let distance = p.length();
            assert!((10.0..30.0).contains(&distance), "{distance}");
            let corner = mesh
                .corner_verts
                .iter()
                .position(|v| *v as usize == index)
                .unwrap();
            assert!(mesh.corner_normals[corner].dot(p.normalized()) > 0.7);
        }
    }

    #[test]
    fn a_pool_of_drops_makes_one_skin() {
        let mut drops = Vec::new();
        for x in 0..10 {
            for z in 0..10 {
                drops.push([x * 14, 0, z * 14]);
            }
        }
        let mesh = surface(&drops, 14.0);
        let (lo, hi) = mesh.bounds().unwrap();
        assert!(hi.x - lo.x > 140.0 && hi.z - lo.z > 140.0);
        assert!(hi.y - lo.y < 40.0, "a flat pool stays flat: {lo:?} {hi:?}");
    }
}
