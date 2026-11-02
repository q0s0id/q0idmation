//! Wavefront OBJ import, with MTL materials and their colour textures.

use std::collections::HashMap;
use std::path::Path;

use q0s_format::three::model::{Material, Mesh, ModelData, Node, Texture};
use q0s_format::three::V3;
use q0s_format::v2::Rgba;

use crate::Imported;

fn rgb(parts: &[&str], alpha: f32) -> Rgba {
    let c = |i: usize| {
        let v: f32 = parts.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.8);
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Rgba {
        r: c(0),
        g: c(1),
        b: c(2),
        a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

fn load_mtl(
    text: &str,
    base: Option<&Path>,
    model: &mut ModelData,
    warnings: &mut Vec<String>,
) -> HashMap<String, u16> {
    let mut out = HashMap::new();
    let mut current: Option<Material> = None;
    let mut alpha = 1.0;
    let finish = |material: Option<Material>,
                  alpha: f32,
                  model: &mut ModelData,
                  out: &mut HashMap<String, u16>| {
        if let Some(mut m) = material {
            m.color.a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.insert(m.name.clone(), model.materials.len() as u16);
            model.materials.push(m);
        }
    };
    for line in text.lines() {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        let Some(tag) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        match tag {
            "newmtl" => {
                finish(current.take(), alpha, model, &mut out);
                alpha = 1.0;
                current = Some(Material::with_color(rest.join(" "), rgb(&[], 1.0)));
            }
            "Kd" => {
                if let Some(m) = &mut current {
                    m.color = rgb(&rest, 1.0);
                }
            }
            "Ke" => {
                if let Some(m) = &mut current {
                    m.emissive = rgb(&rest, 1.0);
                }
            }
            "Ns" => {
                if let (Some(m), Some(ns)) = (
                    &mut current,
                    rest.first().and_then(|s| s.parse::<f32>().ok()),
                ) {
                    m.roughness = (1.0 - (ns.max(0.0) / 1000.0).sqrt()).clamp(0.05, 1.0);
                }
            }
            "d" => alpha = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            "Tr" => {
                alpha = 1.0
                    - rest
                        .first()
                        .and_then(|s| s.parse::<f32>().ok())
                        .unwrap_or(0.0)
            }
            "map_Kd" => {
                let Some(m) = &mut current else { continue };
                let file = rest.last().copied().unwrap_or("");
                let bytes = base.and_then(|b| std::fs::read(b.join(file.replace('\\', "/"))).ok());
                match bytes.and_then(|b| image::load_from_memory(&b).ok()) {
                    Some(image) => {
                        let image = image.to_rgba8();
                        model.textures.push(Texture::new(
                            file,
                            image.width(),
                            image.height(),
                            image.into_raw(),
                        ));
                        m.texture = Some((model.textures.len() - 1) as u16);
                        m.color = Rgba {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                        };
                    }
                    None => warnings.push(format!("texture {file} was not found")),
                }
            }
            _ => {}
        }
    }
    finish(current.take(), alpha, model, &mut out);
    out
}

/// Reads an OBJ file; `base_dir` is where its MTL and textures sit.
pub fn import(bytes: &[u8], base_dir: Option<&Path>) -> Result<Imported, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut warnings = Vec::new();
    let mut model = ModelData {
        materials: vec![Material::default()],
        ..ModelData::default()
    };
    let mut positions: Vec<V3> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<V3> = Vec::new();
    let mut materials: HashMap<String, u16> = HashMap::new();
    let mut material = 0u16;
    let mut smooth = true;
    struct Part {
        name: String,
        mesh: Mesh,
        remap: HashMap<usize, u32>,
        has_normals: bool,
    }
    let new_part = |name: String| Part {
        name,
        mesh: Mesh::default(),
        remap: HashMap::new(),
        has_normals: true,
    };
    let mut parts: Vec<Part> = vec![new_part("Object".into())];
    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_whitespace();
        let Some(tag) = words.next() else { continue };
        let rest: Vec<&str> = words.collect();
        let float = |i: usize| {
            rest.get(i)
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0)
        };
        match tag {
            "v" => positions.push(V3::new(float(0), float(1), float(2))),
            "vt" => uvs.push([float(0), 1.0 - float(1)]),
            "vn" => normals.push(V3::new(float(0), float(1), float(2)).normalized()),
            "o" | "g" => {
                let name = rest.join(" ");
                if parts.last().is_some_and(|p| p.mesh.faces.is_empty()) {
                    parts.last_mut().unwrap().name = name;
                } else {
                    parts.push(new_part(name));
                }
            }
            "s" => smooth = !matches!(rest.first().copied(), Some("off") | Some("0")),
            "mtllib" => {
                for file in &rest {
                    if let Some(text) =
                        base_dir.and_then(|b| std::fs::read_to_string(b.join(file)).ok())
                    {
                        let loaded = load_mtl(&text, base_dir, &mut model, &mut warnings);
                        materials.extend(loaded);
                    } else {
                        warnings.push(format!("material library {file} was not found"));
                    }
                }
            }
            "usemtl" => material = materials.get(&rest.join(" ")).copied().unwrap_or(0),
            "f" => {
                let part = parts.last_mut().unwrap();
                let mut verts = Vec::new();
                let mut face_uvs = Vec::new();
                let mut face_normals = Vec::new();
                for corner in &rest {
                    let mut fields = corner.split('/');
                    let index = |field: Option<&str>, len: usize| -> Option<usize> {
                        let v: i64 = field.filter(|f| !f.is_empty())?.parse().ok()?;
                        let i = if v < 0 { len as i64 + v } else { v - 1 };
                        (0..len as i64).contains(&i).then_some(i as usize)
                    };
                    let Some(p) = index(fields.next(), positions.len()) else {
                        continue;
                    };
                    let t = index(fields.next(), uvs.len());
                    let n = index(fields.next(), normals.len());
                    let local = *part.remap.entry(p).or_insert_with(|| {
                        part.mesh.positions.push(positions[p]);
                        (part.mesh.positions.len() - 1) as u32
                    });
                    verts.push(local);
                    face_uvs.push(t.map_or([0.0, 0.0], |t| uvs[t]));
                    match n {
                        Some(n) => face_normals.push(normals[n]),
                        None => part.has_normals = false,
                    }
                }
                if verts.len() < 3 {
                    continue;
                }
                let start = part.mesh.corner_verts.len() as u32;
                part.mesh.corner_verts.extend_from_slice(&verts);
                part.mesh.corner_uvs.extend(face_uvs);
                part.mesh.corner_normals.extend(face_normals);
                part.mesh.faces.push(q0s_format::three::Face {
                    start,
                    len: verts.len() as u32,
                    material,
                    smooth,
                });
            }
            _ => {}
        }
    }
    for part in parts {
        if part.mesh.faces.is_empty() {
            continue;
        }
        let mut mesh = part.mesh;
        if !part.has_normals || mesh.corner_normals.len() != mesh.corner_verts.len() {
            mesh.corner_normals.clear();
        }
        if uvs.is_empty() {
            mesh.corner_uvs.clear();
        }
        let mut node = Node::new(part.name.clone());
        node.parent = None;
        mesh.name = part.name;
        mesh.node = Some(model.nodes.len() as u16);
        model.nodes.push(node);
        model.meshes.push(mesh);
    }
    if model.meshes.is_empty() {
        return Err("the OBJ file has no faces".into());
    }
    model.sanitize();
    Ok(Imported {
        model,
        name: "OBJ model".into(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_obj_quad_with_negative_indices_reads() {
        let text = "o Plane\nv -1 0 -1\nv 1 0 -1\nv 1 0 1\nv -1 0 1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf -4/1 -3/2 -2/3 -1/4\n";
        let imported = import(text.as_bytes(), None).unwrap();
        let mesh = &imported.model.meshes[0];
        assert_eq!(mesh.name, "Plane");
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.corner_uvs[2], [1.0, 0.0]);
    }
}
