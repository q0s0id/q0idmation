//! glTF 2.0 import, from `.gltf` (with its buffers and images beside it or
//! embedded as data) and `.glb` files.

use std::path::Path;

use q0s_format::three::model::{
    Channel, ChannelTarget, Clip, Face, Material, Mesh, ModelData, Node, Shading, Skin, Texture,
};
use q0s_format::three::{Mat4, Quat, V3};
use q0s_format::v2::Rgba;
use serde_json::Value;

use crate::Imported;

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn load_uri(uri: &str, base: Option<&Path>) -> Option<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,")?;
        return base64_decode(data);
    }
    let decoded = percent_decode(uri);
    std::fs::read(base?.join(decoded)).ok()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Doc {
    json: Value,
    buffers: Vec<Vec<u8>>,
}

impl Doc {
    fn list(&self, name: &str) -> &[Value] {
        self.json[name].as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    fn view(&self, index: usize) -> Option<(&[u8], usize)> {
        let view = self.list("bufferViews").get(index)?;
        let buffer = self.buffers.get(view["buffer"].as_u64()? as usize)?;
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = view["byteLength"].as_u64()? as usize;
        let stride = view["byteStride"].as_u64().unwrap_or(0) as usize;
        Some((buffer.get(offset..offset.checked_add(length)?)?, stride))
    }

    /// An accessor's values as floats, `width` per element.
    fn floats(&self, index: usize) -> Option<(Vec<f32>, usize)> {
        let accessor = self.list("accessors").get(index)?;
        let count = accessor["count"].as_u64()? as usize;
        let width = match accessor["type"].as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            _ => return None,
        };
        let component = accessor["componentType"].as_u64()?;
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return None,
        };
        let mut out = vec![0.0f32; count * width];
        if let Some(view) = accessor["bufferView"].as_u64() {
            let (data, stride) = self.view(view as usize)?;
            let offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
            let stride = if stride > 0 { stride } else { size * width };
            for e in 0..count {
                for c in 0..width {
                    let at = offset + e * stride + c * size;
                    let bytes = data.get(at..at + size)?;
                    let v = match component {
                        5120 => {
                            let v = f32::from(bytes[0] as i8);
                            if normalized {
                                (v / 127.0).max(-1.0)
                            } else {
                                v
                            }
                        }
                        5121 => {
                            let v = f32::from(bytes[0]);
                            if normalized {
                                v / 255.0
                            } else {
                                v
                            }
                        }
                        5122 => {
                            let v = f32::from(i16::from_le_bytes([bytes[0], bytes[1]]));
                            if normalized {
                                (v / 32767.0).max(-1.0)
                            } else {
                                v
                            }
                        }
                        5123 => {
                            let v = f32::from(u16::from_le_bytes([bytes[0], bytes[1]]));
                            if normalized {
                                v / 65535.0
                            } else {
                                v
                            }
                        }
                        5125 => u32::from_le_bytes(bytes.try_into().ok()?) as f32,
                        _ => f32::from_le_bytes(bytes.try_into().ok()?),
                    };
                    out[e * width + c] = v;
                }
            }
        }
        Some((out, width))
    }

    fn indices(&self, index: usize) -> Option<Vec<u32>> {
        let accessor = self.list("accessors").get(index)?;
        let count = accessor["count"].as_u64()? as usize;
        let component = accessor["componentType"].as_u64()?;
        let size = match component {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return None,
        };
        let (data, stride) = self.view(accessor["bufferView"].as_u64()? as usize)?;
        let offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = if stride > 0 { stride } else { size };
        (0..count)
            .map(|e| {
                let at = offset + e * stride;
                let b = data.get(at..at + size)?;
                Some(match size {
                    1 => u32::from(b[0]),
                    2 => u32::from(u16::from_le_bytes([b[0], b[1]])),
                    _ => u32::from_le_bytes(b.try_into().ok()?),
                })
            })
            .collect()
    }
}

fn color(v: &Value, default: [f32; 4]) -> [f32; 4] {
    let mut out = default;
    if let Some(a) = v.as_array() {
        for (i, x) in a.iter().take(4).enumerate() {
            out[i] = x.as_f64().unwrap_or(f64::from(default[i])) as f32;
        }
    }
    out
}

fn to_rgba(c: [f32; 4]) -> Rgba {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba {
        r: b(c[0]),
        g: b(c[1]),
        b: b(c[2]),
        a: b(c[3]),
    }
}

/// Reads a glTF or GLB file.
pub fn import(bytes: &[u8], base_dir: Option<&Path>) -> Result<Imported, String> {
    let mut warnings = Vec::new();
    let (json, bin) = if bytes.starts_with(b"glTF") {
        let mut json = None;
        let mut bin = None;
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = bytes
                .get(at + 8..at + 8 + len)
                .ok_or("GLB chunk runs past the end")?;
            match kind {
                b"JSON" => json = Some(data),
                b"BIN\0" => bin = Some(data.to_vec()),
                _ => {}
            }
            at += 8 + len;
        }
        (json.ok_or("GLB has no JSON")?, bin)
    } else {
        (bytes, None)
    };
    let json: Value = serde_json::from_slice(json).map_err(|e| format!("glTF JSON: {e}"))?;
    let mut buffers = Vec::new();
    for (i, buffer) in json["buffers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        let data = match buffer["uri"].as_str() {
            Some(uri) => {
                load_uri(uri, base_dir).ok_or_else(|| format!("glTF buffer {uri} was not found"))?
            }
            None if i == 0 => bin.clone().ok_or("GLB has no binary chunk")?,
            None => Vec::new(),
        };
        buffers.push(data);
    }
    let doc = Doc { json, buffers };
    let mut model = ModelData {
        materials: Vec::new(),
        ..ModelData::default()
    };

    // Textures.
    let mut texture_index: Vec<Option<u16>> = Vec::new();
    for texture in doc.list("textures") {
        let image = texture["source"]
            .as_u64()
            .and_then(|s| doc.list("images").get(s as usize));
        let bytes = image.and_then(|image| match image["uri"].as_str() {
            Some(uri) => load_uri(uri, base_dir),
            None => image["bufferView"]
                .as_u64()
                .and_then(|v| doc.view(v as usize))
                .map(|(data, _)| data.to_vec()),
        });
        let nearest = texture["sampler"]
            .as_u64()
            .and_then(|s| doc.list("samplers").get(s as usize))
            .is_some_and(|s| s["magFilter"].as_u64() == Some(9728));
        let decoded = bytes.and_then(|b| image::load_from_memory(&b).ok());
        match decoded {
            Some(img) => {
                let img = img.to_rgba8();
                let name = image
                    .and_then(|i| i["name"].as_str().or_else(|| i["uri"].as_str()))
                    .unwrap_or("texture")
                    .to_string();
                let mut t = Texture::new(name, img.width(), img.height(), img.into_raw());
                t.pixelated = nearest;
                model.textures.push(t);
                texture_index.push(Some((model.textures.len() - 1) as u16));
            }
            None => {
                warnings.push("a glTF texture could not be read".into());
                texture_index.push(None);
            }
        }
    }

    // Materials.
    for m in doc.list("materials") {
        let pbr = &m["pbrMetallicRoughness"];
        let mut material = Material::with_color(
            m["name"].as_str().unwrap_or("Material"),
            to_rgba(color(&pbr["baseColorFactor"], [1.0; 4])),
        );
        material.texture = pbr["baseColorTexture"]["index"]
            .as_u64()
            .and_then(|t| texture_index.get(t as usize).copied().flatten());
        material.metallic = pbr["metallicFactor"].as_f64().unwrap_or(1.0) as f32;
        material.roughness = pbr["roughnessFactor"].as_f64().unwrap_or(1.0) as f32;
        let e = color(&m["emissiveFactor"], [0.0, 0.0, 0.0, 1.0]);
        material.emissive = to_rgba([e[0], e[1], e[2], 1.0]);
        material.double_sided = m["doubleSided"].as_bool().unwrap_or(false);
        match m["alphaMode"].as_str() {
            Some("MASK") => {
                material.alpha_cutoff = Some(m["alphaCutoff"].as_f64().unwrap_or(0.5) as f32)
            }
            Some("BLEND") => {}
            _ => material.color.a = 255,
        }
        if m["extensions"]["KHR_materials_unlit"].is_object() {
            material.shading = Shading::Unlit;
        }
        // An opaque material ignores the alpha of its texture too.
        if m["alphaMode"].as_str().is_none_or(|mode| mode == "OPAQUE") {
            if let Some(t) = material
                .texture
                .and_then(|t| model.textures.get_mut(usize::from(t)))
            {
                t.has_alpha = false;
            }
        }
        model.materials.push(material);
    }
    if model.materials.is_empty() {
        model.materials.push(Material::default());
    }

    // Nodes, parents first.
    let nodes = doc.list("nodes");
    let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for child in node["children"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if let Some(c) = child
                .as_u64()
                .map(|c| c as usize)
                .filter(|c| *c < nodes.len())
            {
                parent[c] = Some(i);
            }
        }
    }
    let mut order = Vec::with_capacity(nodes.len());
    let mut placed = vec![false; nodes.len()];
    fn visit(
        i: usize,
        parent: &[Option<usize>],
        placed: &mut [bool],
        order: &mut Vec<usize>,
        depth: usize,
    ) {
        if placed[i] || depth > 512 {
            return;
        }
        if let Some(p) = parent[i] {
            visit(p, parent, placed, order, depth + 1);
        }
        if !placed[i] {
            placed[i] = true;
            order.push(i);
        }
    }
    for i in 0..nodes.len() {
        visit(i, &parent, &mut placed, &mut order, 0);
    }
    let mut index_of = vec![0u16; nodes.len()];
    for (new, old) in order.iter().enumerate() {
        index_of[*old] = new as u16;
    }
    let skinned_joints: std::collections::HashSet<usize> = doc
        .list("skins")
        .iter()
        .flat_map(|s| s["joints"].as_array().cloned().unwrap_or_default())
        .filter_map(|j| j.as_u64().map(|j| j as usize))
        .collect();
    for old in &order {
        let n = &nodes[*old];
        let mut node = Node::new(n["name"].as_str().unwrap_or("node"));
        node.parent = parent[*old].map(|p| index_of[p]);
        if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
            let matrix = Mat4 {
                m: std::array::from_fn(|i| m[i].as_f64().unwrap_or(0.0) as f32),
            };
            let (t, r, s) = matrix.decompose();
            node.translation = t;
            node.rotation = r;
            node.scale = s;
        } else {
            let t = color(&n["translation"], [0.0, 0.0, 0.0, 0.0]);
            let r = color(&n["rotation"], [0.0, 0.0, 0.0, 1.0]);
            let s = color(&n["scale"], [1.0, 1.0, 1.0, 0.0]);
            node.translation = V3::new(t[0], t[1], t[2]);
            node.rotation = Quat::new(r[0], r[1], r[2], r[3]).normalized();
            node.scale = V3::new(s[0], s[1], s[2]);
        }
        node.bone = skinned_joints.contains(old);
        model.nodes.push(node);
    }

    // Meshes.
    for old in &order {
        let n = &nodes[*old];
        let Some(mesh_index) = n["mesh"].as_u64() else {
            continue;
        };
        let Some(source) = doc.list("meshes").get(mesh_index as usize) else {
            continue;
        };
        let mut mesh = Mesh {
            name: source["name"]
                .as_str()
                .or_else(|| n["name"].as_str())
                .unwrap_or("mesh")
                .to_string(),
            node: Some(index_of[*old]),
            ..Mesh::default()
        };
        let mut joints_all: Vec<[u16; 4]> = Vec::new();
        let mut weights_all: Vec<[f32; 4]> = Vec::new();
        let mut has_normals = true;
        let mut has_uvs = false;
        let mut has_colors = false;
        for primitive in source["primitives"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let mode = primitive["mode"].as_u64().unwrap_or(4);
            if !matches!(mode, 4..=6) {
                continue;
            }
            let attributes = &primitive["attributes"];
            let Some((positions, 3)) = attributes["POSITION"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize))
            else {
                continue;
            };
            let base = mesh.positions.len() as u32;
            let count = positions.len() / 3;
            mesh.positions.extend(
                positions
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| V3::new(p[0], p[1], p[2])),
            );
            let normals = attributes["NORMAL"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize));
            if normals.is_none() {
                has_normals = false;
            }
            let uvs = attributes["TEXCOORD_0"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize));
            has_uvs |= uvs.is_some();
            if let Some((colors, width)) = attributes["COLOR_0"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize))
            {
                has_colors = true;
                mesh.colors.resize(
                    base as usize,
                    Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    },
                );
                for e in 0..count {
                    let c = &colors[e * width..e * width + width];
                    mesh.colors.push(to_rgba([
                        c[0],
                        c[1],
                        c[2],
                        if width == 4 { c[3] } else { 1.0 },
                    ]));
                }
            } else if has_colors {
                mesh.colors.resize(
                    mesh.positions.len(),
                    Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    },
                );
            }
            let joints = attributes["JOINTS_0"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize));
            let weights = attributes["WEIGHTS_0"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize));
            joints_all.resize(base as usize, [0; 4]);
            weights_all.resize(base as usize, [0.0; 4]);
            for e in 0..count {
                let j = joints.as_ref().map_or([0; 4], |(v, _)| {
                    std::array::from_fn(|k| v[e * 4 + k] as u16)
                });
                let w = weights
                    .as_ref()
                    .map_or([0.0; 4], |(v, _)| std::array::from_fn(|k| v[e * 4 + k]));
                joints_all.push(j);
                weights_all.push(w);
            }
            let mut list: Vec<u32> = match primitive["indices"].as_u64() {
                Some(a) => doc.indices(a as usize).unwrap_or_default(),
                None => (0..count as u32).collect(),
            };
            if mode == 5 {
                list = (2..list.len())
                    .flat_map(|i| {
                        if i % 2 == 0 {
                            [list[i - 2], list[i - 1], list[i]]
                        } else {
                            [list[i - 1], list[i - 2], list[i]]
                        }
                    })
                    .collect();
            } else if mode == 6 {
                list = (2..list.len())
                    .flat_map(|i| [list[0], list[i - 1], list[i]])
                    .collect();
            }
            let material = primitive["material"].as_u64().map_or(0, |m| m as u16);
            for tri in list.as_chunks::<3>().0.iter() {
                if tri.iter().any(|v| *v as usize >= count) {
                    continue;
                }
                let start = mesh.corner_verts.len() as u32;
                for v in tri {
                    mesh.corner_verts.push(base + v);
                    let at = *v as usize;
                    mesh.corner_uvs.push(
                        uvs.as_ref()
                            .map_or([0.0, 0.0], |(u, _)| [u[at * 2], u[at * 2 + 1]]),
                    );
                    mesh.corner_normals
                        .push(normals.as_ref().map_or(V3::Y, |(n, _)| {
                            V3::new(n[at * 3], n[at * 3 + 1], n[at * 3 + 2]).normalized()
                        }));
                }
                mesh.faces.push(Face {
                    start,
                    len: 3,
                    material,
                    smooth: true,
                });
            }
        }
        if mesh.faces.is_empty() {
            continue;
        }
        if !has_normals {
            mesh.corner_normals.clear();
        }
        if !has_uvs {
            mesh.corner_uvs.clear();
        }
        if has_colors {
            mesh.colors.resize(
                mesh.positions.len(),
                Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
            );
        }
        if let Some(skin) = n["skin"]
            .as_u64()
            .and_then(|s| doc.list("skins").get(s as usize))
        {
            let joints: Vec<u16> = skin["joints"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|j| {
                    j.as_u64()
                        .filter(|j| (*j as usize) < nodes.len())
                        .map(|j| index_of[j as usize])
                })
                .collect();
            let inverse: Vec<Mat4> = skin["inverseBindMatrices"]
                .as_u64()
                .and_then(|a| doc.floats(a as usize))
                .map(|(m, _)| {
                    m.as_chunks::<16>()
                        .0
                        .iter()
                        .map(|c| Mat4 {
                            m: std::array::from_fn(|i| c[i]),
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
            if !joints.is_empty() && inverse.len() == joints.len() {
                let mut s = Skin {
                    joints,
                    inverse_bind: inverse,
                    ..Skin::default()
                };
                for (v, (j, w)) in joints_all.iter().zip(&weights_all).enumerate() {
                    s.set_vertex(v, (0..4).map(|k| (j[k], w[k])).collect());
                }
                mesh.skin = Some(s);
            }
        }
        model.meshes.push(mesh);
    }

    // Animations.
    for (index, animation) in doc.list("animations").iter().enumerate() {
        let mut clip = Clip {
            name: animation["name"]
                .as_str()
                .map_or_else(|| format!("Animation {}", index + 1), str::to_string),
            ..Clip::default()
        };
        let samplers = animation["samplers"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for channel in animation["channels"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let target = &channel["target"];
            let (Some(node), Some(path)) = (target["node"].as_u64(), target["path"].as_str())
            else {
                continue;
            };
            let kind = match path {
                "translation" => ChannelTarget::Translation,
                "rotation" => ChannelTarget::Rotation,
                "scale" => ChannelTarget::Scale,
                _ => continue,
            };
            let Some(sampler) = channel["sampler"]
                .as_u64()
                .and_then(|s| samplers.get(s as usize))
            else {
                continue;
            };
            let (Some((times, _)), Some((values, width))) = (
                sampler["input"]
                    .as_u64()
                    .and_then(|a| doc.floats(a as usize)),
                sampler["output"]
                    .as_u64()
                    .and_then(|a| doc.floats(a as usize)),
            ) else {
                continue;
            };
            let cubic = sampler["interpolation"].as_str() == Some("CUBICSPLINE");
            let values: Vec<f32> = if cubic {
                values
                    .chunks_exact(width * 3)
                    .flat_map(|c| c[width..width * 2].to_vec())
                    .collect()
            } else {
                values
            };
            if width != kind.width()
                || values.len() != times.len() * width
                || (node as usize) >= nodes.len()
            {
                continue;
            }
            clip.channels.push(Channel {
                node: index_of[node as usize],
                target: kind,
                times,
                values,
                step: sampler["interpolation"].as_str() == Some("STEP"),
            });
        }
        if !clip.channels.is_empty() {
            model.clips.push(clip);
        }
    }

    model.sanitize();
    if model.meshes.is_empty() {
        return Err("the glTF file has no meshes".into());
    }
    let rest = model.rest_world();
    for i in 0..model.nodes.len() {
        if !model.nodes[i].bone {
            continue;
        }
        let here = rest[i].translation_part();
        let child = (0..model.nodes.len())
            .find(|c| model.nodes[*c].parent == Some(i as u16) && model.nodes[*c].bone)
            .map(|c| rest[c].translation_part());
        model.nodes[i].length = child.map_or(0.1, |c| (c - here).length().max(0.01));
    }
    Ok(Imported {
        model,
        name: doc.json["asset"]["generator"]
            .as_str()
            .map_or("glTF model", |_| "glTF model")
            .to_string(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gltf_triangle_with_data_uri_reads() {
        // Three float positions, then three u16 indices (and padding).
        let mut bin = Vec::new();
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        for i in [0u16, 1, 2, 0] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let mut b64 = String::new();
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in bin.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for k in 0..4 {
                if k <= chunk.len() {
                    b64.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
                } else {
                    b64.push('=');
                }
            }
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":6}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
            "nodes":[{{"mesh":0,"translation":[1,2,3]}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0,1,0,1]}}}}]}}"#,
            bin.len(),
            b64
        );
        let imported = import(json.as_bytes(), None).unwrap();
        let model = &imported.model;
        assert_eq!(model.meshes[0].faces.len(), 1);
        assert_eq!(model.meshes[0].positions[1], V3::new(1.0, 0.0, 0.0));
        assert_eq!(model.materials[0].color.g, 255);
        assert_eq!(model.nodes[0].translation, V3::new(1.0, 2.0, 3.0));
    }
}
