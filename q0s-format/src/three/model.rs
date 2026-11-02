//! 3D models: a node tree (bones are nodes), polygon meshes, materials,
//! textures and animation clips.
//!
//! Model space is y-up, right-handed, with the front facing +z, like glTF.
//! A model placed on stage is turned into stage space (y down, z away from
//! the viewer) and scaled by [`ModelData::scale`] stage pixels per unit.

use std::sync::Arc;

use super::math::{Mat4, Quat, V3};
use crate::v2::Rgba;

/// A model in the library.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelAsset {
    pub asset_id: u16,
    /// Shared between copies of the project until one of them edits it.
    #[cfg_attr(feature = "serde", serde(with = "super::wire::serde_model"))]
    pub data: Arc<ModelData>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelData {
    /// Parents always come before their children.
    pub nodes: Vec<Node>,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub textures: Vec<Texture>,
    pub clips: Vec<Clip>,
    /// Stage pixels per model unit.
    pub scale: f32,
    /// A cartoon outline drawn around the whole model.
    pub outline: Option<Outline>,
}

impl ModelAsset {
    /// The rest pose seen from the front, in the placement's own
    /// coordinates (x right, y down): min and max corners.
    pub fn flat_bounds(&self) -> Option<(crate::v2::Vec2, crate::v2::Vec2)> {
        let (lo, hi) = self.data.bounds()?;
        let s = self.data.scale;
        Some((
            crate::v2::Vec2::new(lo.x * s, -hi.y * s),
            crate::v2::Vec2::new(hi.x * s, -lo.y * s),
        ))
    }
}

impl Default for ModelData {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            meshes: Vec::new(),
            materials: vec![Material::default()],
            textures: Vec::new(),
            clips: Vec::new(),
            scale: 1.0,
            outline: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Outline {
    /// Stage pixels.
    pub width: f32,
    pub color: Rgba,
}

/// A transform in the tree. Bones are nodes with `bone` set.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Node {
    pub name: String,
    pub parent: Option<u16>,
    pub translation: V3,
    pub rotation: Quat,
    pub scale: V3,
    pub bone: bool,
    /// How long the bone is drawn, along its local +y.
    pub length: f32,
}

impl Node {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parent: None,
            translation: V3::ZERO,
            rotation: Quat::IDENTITY,
            scale: V3::ONE,
            bone: false,
            length: 0.0,
        }
    }

    pub fn local(&self) -> Mat4 {
        Mat4::from_trs(self.translation, self.rotation, self.scale)
    }
}

/// One polygon: `len` corners starting at `start` in the mesh's corner
/// lists.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Face {
    pub start: u32,
    pub len: u32,
    pub material: u16,
    pub smooth: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mesh {
    pub name: String,
    /// The node it moves with. A skinned mesh follows its bones instead.
    pub node: Option<u16>,
    pub positions: Vec<V3>,
    pub faces: Vec<Face>,
    /// The vertex of every face corner.
    pub corner_verts: Vec<u32>,
    /// Empty, or a texture position for every corner.
    pub corner_uvs: Vec<[f32; 2]>,
    /// Empty, or an imported normal for every corner.
    pub corner_normals: Vec<V3>,
    /// Empty, or a colour for every vertex.
    pub colors: Vec<Rgba>,
    pub skin: Option<Skin>,
    pub hidden: bool,
}

impl Mesh {
    pub fn face_verts(&self, face: &Face) -> &[u32] {
        let start = face.start as usize;
        &self.corner_verts[start..start + face.len as usize]
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.faces
            .iter()
            .map(|face| face.len.saturating_sub(2) as usize)
            .sum()
    }

    /// Geometric normal of a face (Newell's method), unit length.
    pub fn face_normal(&self, face: &Face) -> V3 {
        newell_normal(
            self.face_verts(face)
                .iter()
                .map(|v| self.positions[*v as usize]),
        )
    }

    pub fn face_center(&self, face: &Face) -> V3 {
        let verts = self.face_verts(face);
        let mut sum = V3::ZERO;
        for v in verts {
            sum += self.positions[*v as usize];
        }
        sum / verts.len().max(1) as f32
    }

    /// Appends a polygon over `verts`, with optional per-corner uvs.
    pub fn push_face(
        &mut self,
        verts: &[u32],
        uvs: Option<&[[f32; 2]]>,
        material: u16,
        smooth: bool,
    ) {
        let start = self.corner_verts.len() as u32;
        self.corner_verts.extend_from_slice(verts);
        let has_uvs = !self.corner_uvs.is_empty() || uvs.is_some();
        if has_uvs {
            self.corner_uvs.resize(start as usize, [0.0, 0.0]);
            match uvs {
                Some(uvs) => self.corner_uvs.extend_from_slice(uvs),
                None => self.corner_uvs.extend(verts.iter().map(|_| [0.0, 0.0])),
            }
        }
        if !self.corner_normals.is_empty() {
            let normal = newell_normal(verts.iter().map(|v| self.positions[*v as usize]));
            self.corner_normals.extend(verts.iter().map(|_| normal));
        }
        self.faces.push(Face {
            start,
            len: verts.len() as u32,
            material,
            smooth,
        });
    }

    pub fn bounds(&self) -> Option<(V3, V3)> {
        let mut iter = self.positions.iter();
        let first = *iter.next()?;
        Some(iter.fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }
}

pub fn newell_normal(points: impl Iterator<Item = V3> + Clone) -> V3 {
    let mut normal = V3::ZERO;
    let mut iter = points.clone().peekable();
    let Some(first) = iter.peek().copied() else {
        return V3::Y;
    };
    let mut previous = None;
    for p in points {
        if let Some(a) = previous {
            normal += newell_term(a, p);
        }
        previous = Some(p);
    }
    if let Some(last) = previous {
        normal += newell_term(last, first);
    }
    let normal = normal.normalized();
    if normal == V3::ZERO {
        V3::Y
    } else {
        normal
    }
}

fn newell_term(a: V3, b: V3) -> V3 {
    V3::new(
        (a.y - b.y) * (a.z + b.z),
        (a.z - b.z) * (a.x + b.x),
        (a.x - b.x) * (a.y + b.y),
    )
}

/// How a mesh bends with bones: up to four bones per vertex.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Skin {
    /// The node of every joint.
    pub joints: Vec<u16>,
    /// Model space to each joint's space in the bind pose.
    pub inverse_bind: Vec<Mat4>,
    /// Joint indices per vertex.
    pub influences: Vec<[u16; 4]>,
    /// Weights per vertex, summing to one.
    pub weights: Vec<[f32; 4]>,
}

impl Skin {
    /// Keeps the four strongest influences and makes them sum to one.
    pub fn set_vertex(&mut self, vertex: usize, mut pairs: Vec<(u16, f32)>) {
        pairs.retain(|(_, weight)| *weight > 1.0e-5 && weight.is_finite());
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(4);
        let total: f32 = pairs.iter().map(|(_, w)| *w).sum();
        let mut joints = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for (slot, (joint, weight)) in pairs.iter().enumerate() {
            joints[slot] = *joint;
            weights[slot] = if total > 0.0 { weight / total } else { 0.0 };
        }
        if vertex >= self.influences.len() {
            self.influences.resize(vertex + 1, [0; 4]);
            self.weights.resize(vertex + 1, [0.0; 4]);
        }
        self.influences[vertex] = joints;
        self.weights[vertex] = weights;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Shading {
    /// Smooth light and highlights.
    #[default]
    Lit,
    /// Flat bands of light, like cel animation.
    Toon,
    /// Its own colour, whatever the light.
    Unlit,
    /// Clear like glass or water: it lets what is behind show through,
    /// tinted by its colour, and shines and mirrors its surroundings more
    /// the more it is seen edge-on. Its colour's opacity is how much of the
    /// glass itself shows.
    Glass,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Material {
    pub name: String,
    pub color: Rgba,
    /// Index into the model's textures.
    pub texture: Option<u16>,
    pub emissive: Rgba,
    /// 0 is a mirror shine, 1 is chalk.
    pub roughness: f32,
    pub metallic: f32,
    pub shading: Shading,
    /// Light bands for toon shading.
    pub toon_bands: u8,
    pub double_sided: bool,
    /// Pixels less opaque than this are cut away; `None` blends them.
    pub alpha_cutoff: Option<f32>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "Material".to_string(),
            color: Rgba {
                r: 200,
                g: 200,
                b: 205,
                a: 255,
            },
            texture: None,
            emissive: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            roughness: 0.6,
            metallic: 0.0,
            shading: Shading::Lit,
            toon_bands: 3,
            double_sided: false,
            alpha_cutoff: None,
        }
    }
}

impl Material {
    pub fn with_color(name: impl Into<String>, color: Rgba) -> Self {
        Self {
            name: name.into(),
            color,
            ..Self::default()
        }
    }

    /// Does anything of it let the background through?
    pub fn blends(&self, textures: &[Texture]) -> bool {
        if self.shading == Shading::Glass {
            return true;
        }
        if self.alpha_cutoff.is_some() {
            return false;
        }
        self.color.a < 255
            || self
                .texture
                .and_then(|index| textures.get(usize::from(index)))
                .is_some_and(|texture| texture.has_alpha)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Straight (not premultiplied) RGBA8.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_support::b64_arc"))]
    pub rgba: Arc<Vec<u8>>,
    /// Any pixel less than opaque.
    pub has_alpha: bool,
    /// Nearest-pixel sampling, for pixel art.
    pub pixelated: bool,
}

impl Texture {
    pub fn new(name: impl Into<String>, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let has_alpha = rgba.as_chunks::<4>().0.iter().any(|px| px[3] < 255);
        Self {
            name: name.into(),
            width,
            height,
            rgba: Arc::new(rgba),
            has_alpha,
            pixelated: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ChannelTarget {
    Translation,
    Rotation,
    Scale,
}

impl ChannelTarget {
    pub fn width(self) -> usize {
        match self {
            Self::Rotation => 4,
            Self::Translation | Self::Scale => 3,
        }
    }
}

/// Keys of one node property.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Channel {
    pub node: u16,
    pub target: ChannelTarget,
    /// Seconds, rising.
    pub times: Vec<f32>,
    /// `target.width()` numbers per key: xyz, or a quaternion xyzw.
    pub values: Vec<f32>,
    /// Jump between keys instead of easing.
    pub step: bool,
}

impl Channel {
    pub fn key_count(&self) -> usize {
        self.times.len()
    }

    fn value(&self, key: usize) -> &[f32] {
        let width = self.target.width();
        &self.values[key * width..key * width + width]
    }

    /// The value at `time`, held before the first key and after the last.
    pub fn sample(&self, time: f32) -> [f32; 4] {
        let width = self.target.width();
        let mut out = [0.0, 0.0, 0.0, 1.0];
        if self.times.is_empty() || self.values.len() < self.times.len() * width {
            return out;
        }
        let next = self.times.partition_point(|t| *t <= time);
        let (a, b, t) = if next == 0 {
            (0, 0, 0.0)
        } else if next >= self.times.len() {
            let last = self.times.len() - 1;
            (last, last, 0.0)
        } else {
            let (ta, tb) = (self.times[next - 1], self.times[next]);
            let t = if tb > ta {
                (time - ta) / (tb - ta)
            } else {
                0.0
            };
            (next - 1, next, if self.step { 0.0 } else { t })
        };
        let (va, vb) = (self.value(a), self.value(b));
        if self.target == ChannelTarget::Rotation {
            let qa = Quat::new(va[0], va[1], va[2], va[3]);
            let qb = Quat::new(vb[0], vb[1], vb[2], vb[3]);
            let q = qa.slerp(qb, t);
            out = [q.x, q.y, q.z, q.w];
        } else {
            for i in 0..3 {
                out[i] = va[i] + (vb[i] - va[i]) * t;
            }
        }
        out
    }

    /// Sets the value at `time`, adding a key or replacing one there.
    pub fn set_key(&mut self, time: f32, value: &[f32]) {
        let width = self.target.width();
        let position = self.times.iter().position(|t| (t - time).abs() < 1.0e-4);
        match position {
            Some(index) => {
                self.values[index * width..index * width + width].copy_from_slice(&value[..width])
            }
            None => {
                let index = self.times.partition_point(|t| *t < time);
                self.times.insert(index, time);
                let at = index * width;
                for (offset, v) in value[..width].iter().enumerate() {
                    self.values.insert(at + offset, *v);
                }
            }
        }
    }

    pub fn remove_key(&mut self, index: usize) {
        if index >= self.times.len() {
            return;
        }
        let width = self.target.width();
        self.times.remove(index);
        self.values.drain(index * width..index * width + width);
    }
}

/// A named animation, like "walk" or "jump".
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Clip {
    pub name: String,
    /// Seconds.
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl Clip {
    pub fn channel_mut(&mut self, node: u16, target: ChannelTarget) -> &mut Channel {
        let index = match self
            .channels
            .iter()
            .position(|channel| channel.node == node && channel.target == target)
        {
            Some(index) => index,
            None => {
                self.channels.push(Channel {
                    node,
                    target,
                    times: Vec::new(),
                    values: Vec::new(),
                    step: false,
                });
                self.channels.len() - 1
            }
        };
        &mut self.channels[index]
    }

    /// Every key time in the clip, rising and without repeats.
    pub fn key_times(&self) -> Vec<f32> {
        let mut times: Vec<f32> = self
            .channels
            .iter()
            .flat_map(|channel| channel.times.iter().copied())
            .collect();
        times.sort_by(f32::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1.0e-4);
        times
    }
}

/// A node's local transform in a pose.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Trs {
    pub translation: V3,
    pub rotation: Quat,
    pub scale: V3,
}

impl Trs {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_trs(self.translation, self.rotation, self.scale)
    }

    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

impl ModelData {
    pub fn rest_locals(&self) -> Vec<Trs> {
        self.nodes
            .iter()
            .map(|node| Trs {
                translation: node.translation,
                rotation: node.rotation,
                scale: node.scale,
            })
            .collect()
    }

    /// Local transforms of every node at `time` seconds into `clip`.
    pub fn sample_clip(&self, clip: Option<usize>, time: f32) -> Vec<Trs> {
        let mut locals = self.rest_locals();
        if let Some(clip) = clip.and_then(|index| self.clips.get(index)) {
            for channel in &clip.channels {
                let Some(local) = locals.get_mut(usize::from(channel.node)) else {
                    continue;
                };
                let v = channel.sample(time);
                match channel.target {
                    ChannelTarget::Translation => local.translation = V3::new(v[0], v[1], v[2]),
                    ChannelTarget::Rotation => {
                        local.rotation = Quat::new(v[0], v[1], v[2], v[3]).normalized()
                    }
                    ChannelTarget::Scale => local.scale = V3::new(v[0], v[1], v[2]),
                }
            }
        }
        locals
    }

    /// Model-space matrices of every node for these local transforms.
    pub fn world_matrices(&self, locals: &[Trs]) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            let local = locals
                .get(index)
                .map(Trs::matrix)
                .unwrap_or_else(|| node.local());
            let matrix = match node.parent.map(usize::from) {
                Some(parent) if parent < world.len() => world[parent] * local,
                _ => local,
            };
            world.push(matrix);
        }
        world
    }

    pub fn rest_world(&self) -> Vec<Mat4> {
        self.world_matrices(&self.rest_locals())
    }

    /// Bounds of every visible mesh in the rest pose, in model units.
    pub fn bounds(&self) -> Option<(V3, V3)> {
        let world = self.rest_world();
        let mut out: Option<(V3, V3)> = None;
        for mesh in self.meshes.iter().filter(|mesh| !mesh.hidden) {
            // Skinned meshes stand where their bones put them.
            let skinned = skin_mesh(mesh, &world).map(|(positions, _)| positions);
            let matrix = if skinned.is_some() {
                Mat4::IDENTITY
            } else {
                mesh_matrix(mesh, &world)
            };
            for p in skinned.as_deref().unwrap_or(&mesh.positions) {
                let q = matrix.transform_point(*p);
                out = Some(match out {
                    Some((lo, hi)) => (lo.min(q), hi.max(q)),
                    None => (q, q),
                });
            }
        }
        out
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|mesh| mesh.positions.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(Mesh::triangle_count).sum()
    }

    pub fn find_node(&self, name: &str) -> Option<u16> {
        self.nodes
            .iter()
            .position(|node| node.name == name)
            .map(|index| index as u16)
    }

    pub fn children(&self, node: u16) -> impl Iterator<Item = u16> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(move |(_, n)| n.parent == Some(node))
            .map(|(index, _)| index as u16)
    }

    /// Fixes what an editor or an importer could leave inconsistent:
    /// out-of-range references, parents after children, broken numbers.
    pub fn sanitize(&mut self) {
        let count = self.nodes.len();
        for index in 0..count {
            let node = &mut self.nodes[index];
            if node
                .parent
                .is_some_and(|parent| usize::from(parent) >= index)
            {
                node.parent = None;
            }
            if !node.translation.is_finite() {
                node.translation = V3::ZERO;
            }
            if !node.rotation.is_finite() {
                node.rotation = Quat::IDENTITY;
            }
            node.rotation = node.rotation.normalized();
            if !node.scale.is_finite() {
                node.scale = V3::ONE;
            }
        }
        if self.materials.is_empty() {
            self.materials.push(Material::default());
        }
        let material_count = self.materials.len() as u16;
        let texture_count = self.textures.len();
        for material in &mut self.materials {
            if material
                .texture
                .is_some_and(|texture| usize::from(texture) >= texture_count)
            {
                material.texture = None;
            }
            material.roughness = material.roughness.clamp(0.0, 1.0);
            material.metallic = material.metallic.clamp(0.0, 1.0);
            material.toon_bands = material.toon_bands.clamp(2, 8);
        }
        for mesh in &mut self.meshes {
            if mesh.node.is_some_and(|node| usize::from(node) >= count) {
                mesh.node = None;
            }
            let vertex_count = mesh.positions.len() as u32;
            let corner_count = mesh.corner_verts.len() as u32;
            for p in &mut mesh.positions {
                if !p.is_finite() {
                    *p = V3::ZERO;
                }
            }
            mesh.faces.retain(|face| {
                face.len >= 3
                    && face
                        .start
                        .checked_add(face.len)
                        .is_some_and(|end| end <= corner_count)
            });
            let corner_verts = &mesh.corner_verts;
            mesh.faces.retain(|face| {
                corner_verts[face.start as usize..(face.start + face.len) as usize]
                    .iter()
                    .all(|v| *v < vertex_count)
            });
            for face in &mut mesh.faces {
                if face.material >= material_count {
                    face.material = 0;
                }
            }
            if mesh.corner_uvs.len() != mesh.corner_verts.len() {
                mesh.corner_uvs.clear();
            }
            if mesh.corner_normals.len() != mesh.corner_verts.len() {
                mesh.corner_normals.clear();
            }
            if mesh.colors.len() != mesh.positions.len() {
                mesh.colors.clear();
            }
            if let Some(skin) = &mut mesh.skin {
                let joints = skin.joints.len();
                let valid = !skin.joints.is_empty()
                    && skin.inverse_bind.len() == joints
                    && skin.joints.iter().all(|node| usize::from(*node) < count)
                    && skin.influences.len() == mesh.positions.len()
                    && skin.weights.len() == mesh.positions.len();
                if valid {
                    for (influence, weight) in skin.influences.iter_mut().zip(&mut skin.weights) {
                        for slot in 0..4 {
                            if usize::from(influence[slot]) >= joints || !weight[slot].is_finite() {
                                influence[slot] = 0;
                                weight[slot] = 0.0;
                            }
                        }
                    }
                } else {
                    mesh.skin = None;
                }
            }
        }
        for clip in &mut self.clips {
            clip.channels.retain(|channel| {
                usize::from(channel.node) < count
                    && channel.values.len() == channel.times.len() * channel.target.width()
            });
            if !clip.duration.is_finite() || clip.duration < 0.0 {
                clip.duration = 0.0;
            }
            let last = clip
                .channels
                .iter()
                .filter_map(|channel| channel.times.last().copied())
                .fold(0.0f32, f32::max);
            clip.duration = clip.duration.max(last);
        }
        if !self.scale.is_finite() || self.scale <= 0.0 {
            self.scale = 1.0;
        }
    }
}

/// Where a mesh's own coordinates go in model space, for a pose's world
/// matrices. Skinned meshes are placed by their bones instead.
pub fn mesh_matrix(mesh: &Mesh, world: &[Mat4]) -> Mat4 {
    mesh.node
        .and_then(|node| world.get(usize::from(node)).copied())
        .unwrap_or(Mat4::IDENTITY)
}

/// Faces meeting at more than this many degrees keep a hard edge between
/// them even when both are shaded smooth.
pub const AUTO_SMOOTH_DEGREES: f32 = 60.0;

/// A mesh ready to draw: triangles over corners that each carry a vertex,
/// a normal and a texture position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderMesh {
    /// The original vertex of each render vertex, for skinning.
    pub source: Vec<u32>,
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
    /// Triangles by material: (material, first index, index count).
    pub parts: Vec<(u16, u32, u32)>,
    pub has_colors: bool,
    /// Every edge of every polygon once, as render-vertex pairs.
    pub edges: Vec<[u32; 2]>,
}

impl RenderMesh {
    pub fn build(mesh: &Mesh) -> Self {
        let vertex_count = mesh.positions.len();
        let mut face_normals = Vec::with_capacity(mesh.faces.len());
        for face in &mesh.faces {
            let verts = mesh.face_verts(face);
            let mut area = V3::ZERO;
            for i in 1..verts.len().saturating_sub(1) {
                let a = mesh.positions[verts[0] as usize];
                let b = mesh.positions[verts[i] as usize];
                let c = mesh.positions[verts[i + 1] as usize];
                area += (b - a).cross(c - a);
            }
            let normal = if area.length_sq() > 1.0e-20 {
                area.normalized()
            } else {
                mesh.face_normal(face)
            };
            face_normals.push(normal);
        }
        // The smooth faces around each vertex, with the angle each makes
        // there: weighting by that angle keeps the way a face happens to be
        // cut up out of the shading.
        let mut around: Vec<Vec<(usize, f32)>> = vec![Vec::new(); vertex_count];
        for (face_index, face) in mesh.faces.iter().enumerate() {
            if !face.smooth {
                continue;
            }
            let verts = mesh.face_verts(face);
            let n = verts.len();
            for (k, v) in verts.iter().enumerate() {
                let at = mesh.positions[*v as usize];
                let to_prev = (mesh.positions[verts[(k + n - 1) % n] as usize] - at).normalized();
                let to_next = (mesh.positions[verts[(k + 1) % n] as usize] - at).normalized();
                let angle = to_prev.dot(to_next).clamp(-1.0, 1.0).acos();
                if let Some(list) = around.get_mut(*v as usize) {
                    list.push((face_index, angle));
                }
            }
        }
        // A smooth corner blends only the faces meeting its own at a gentle
        // angle, so a smooth box keeps its edges and a smooth ball is round.
        let min_cos = AUTO_SMOOTH_DEGREES.to_radians().cos();
        let smooth_normal = |face_index: usize, v: u32| {
            let own = face_normals[face_index];
            let mut sum = V3::ZERO;
            for (other, angle) in around.get(v as usize).map_or(&[][..], Vec::as_slice) {
                let normal = face_normals[*other];
                if normal.dot(own) >= min_cos {
                    sum += normal * *angle;
                }
            }
            if sum.length_sq() > 1.0e-20 {
                sum.normalized()
            } else {
                own
            }
        };
        let mut out = RenderMesh {
            has_colors: !mesh.colors.is_empty(),
            ..Default::default()
        };
        let mut by_material: Vec<(u16, Vec<u32>)> = Vec::new();
        let mut shared: std::collections::HashMap<(u32, [u32; 2], [u32; 3]), u32> =
            std::collections::HashMap::new();
        let mut seen_edges = std::collections::HashSet::new();
        for (face_index, face) in mesh.faces.iter().enumerate() {
            let verts = mesh.face_verts(face);
            let mut corner_ids = Vec::with_capacity(verts.len());
            for (offset, v) in verts.iter().enumerate() {
                let corner = face.start as usize + offset;
                let uv = mesh.corner_uvs.get(corner).copied().unwrap_or([0.0, 0.0]);
                let normal = match mesh.corner_normals.get(corner) {
                    Some(normal)
                        if face.smooth || mesh.corner_normals.len() == mesh.corner_verts.len() =>
                    {
                        *normal
                    }
                    _ if face.smooth => smooth_normal(face_index, *v),
                    _ => face_normals[face_index],
                };
                let key = (
                    *v,
                    [uv[0].to_bits(), uv[1].to_bits()],
                    [normal.x.to_bits(), normal.y.to_bits(), normal.z.to_bits()],
                );
                let id = *shared.entry(key).or_insert_with(|| {
                    let id = out.positions.len() as u32;
                    out.source.push(*v);
                    out.positions.push(mesh.positions[*v as usize]);
                    out.normals.push(normal);
                    out.uvs.push(uv);
                    let color = mesh
                        .colors
                        .get(*v as usize)
                        .map_or([255; 4], |c| [c.r, c.g, c.b, c.a]);
                    out.colors.push(color);
                    id
                });
                corner_ids.push(id);
            }
            for i in 0..verts.len() {
                let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
                if seen_edges.insert((a.min(b), a.max(b))) {
                    out.edges
                        .push([corner_ids[i], corner_ids[(i + 1) % verts.len()]]);
                }
            }
            let points: Vec<V3> = verts.iter().map(|v| mesh.positions[*v as usize]).collect();
            let triangles = triangulate(&points, face_normals[face_index]);
            let list = match by_material.iter_mut().find(|(m, _)| *m == face.material) {
                Some((_, list)) => list,
                None => {
                    by_material.push((face.material, Vec::new()));
                    &mut by_material.last_mut().unwrap().1
                }
            };
            for [a, b, c] in triangles {
                list.extend_from_slice(&[corner_ids[a], corner_ids[b], corner_ids[c]]);
            }
        }
        by_material.sort_by_key(|(material, _)| *material);
        for (material, list) in by_material {
            let first = out.indices.len() as u32;
            out.indices.extend_from_slice(&list);
            out.parts.push((material, first, list.len() as u32));
        }
        out
    }
}

/// Triangles covering a planar polygon, as corner index triples. Convex
/// polygons fan; others are ear-clipped in their own plane.
pub fn triangulate(points: &[V3], normal: V3) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    // Project onto the plane's two widest axes.
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();
    let flat: Vec<[f32; 2]> = points
        .iter()
        .map(|p| {
            if az >= ax && az >= ay {
                if normal.z >= 0.0 {
                    [p.x, p.y]
                } else {
                    [p.y, p.x]
                }
            } else if ax >= ay {
                if normal.x >= 0.0 {
                    [p.y, p.z]
                } else {
                    [p.z, p.y]
                }
            } else if normal.y >= 0.0 {
                [p.z, p.x]
            } else {
                [p.x, p.z]
            }
        })
        .collect();
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let convex = (0..n).all(|i| cross(flat[i], flat[(i + 1) % n], flat[(i + 2) % n]) >= -1.0e-9);
    if convex {
        return (1..n - 1).map(|i| [0, i, i + 1]).collect();
    }
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while remaining.len() > 3 && guard < n * n {
        guard += 1;
        let m = remaining.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (
                remaining[(i + m - 1) % m],
                remaining[i],
                remaining[(i + 1) % m],
            );
            let (a, b, c) = (flat[ia], flat[ib], flat[ic]);
            if cross(a, b, c) <= 1.0e-12 {
                continue;
            }
            let inside = remaining.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = flat[j];
                cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
            });
            if inside {
                continue;
            }
            out.push([ia, ib, ic]);
            remaining.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if remaining.len() >= 3 {
        for i in 1..remaining.len() - 1 {
            out.push([remaining[0], remaining[i], remaining[i + 1]]);
        }
    }
    out
}

/// Skinned vertex positions of `mesh` in model space for a pose, with the
/// blended 3×3 part (as columns) that turns each vertex's normals.
pub fn skin_mesh(mesh: &Mesh, world: &[Mat4]) -> Option<(Vec<V3>, Vec<[V3; 3]>)> {
    let skin = mesh.skin.as_ref()?;
    let joints: Vec<Mat4> = skin
        .joints
        .iter()
        .zip(&skin.inverse_bind)
        .map(|(node, inverse)| {
            world
                .get(usize::from(*node))
                .map(|m| *m * *inverse)
                .unwrap_or(Mat4::IDENTITY)
        })
        .collect();
    let mut positions = Vec::with_capacity(mesh.positions.len());
    let mut linear = Vec::with_capacity(mesh.positions.len());
    for (index, p) in mesh.positions.iter().enumerate() {
        let influence = skin.influences[index];
        let weight = skin.weights[index];
        let mut out = V3::ZERO;
        let mut blended = [0.0f32; 9];
        let mut total = 0.0;
        for slot in 0..4 {
            let w = weight[slot];
            if w <= 0.0 {
                continue;
            }
            let Some(joint) = joints.get(usize::from(influence[slot])) else {
                continue;
            };
            total += w;
            out += joint.transform_point(*p) * w;
            for c in 0..3 {
                for r in 0..3 {
                    blended[c * 3 + r] += joint.m[c * 4 + r] * w;
                }
            }
        }
        if total <= 0.0 {
            out = *p;
            blended = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        }
        positions.push(out);
        linear.push([
            V3::new(blended[0], blended[1], blended[2]),
            V3::new(blended[3], blended[4], blended[5]),
            V3::new(blended[6], blended[7], blended[8]),
        ]);
    }
    Some((positions, linear))
}

/// Posed render positions and normals of a skinned mesh in model space.
pub fn skinned_render_vertices(
    mesh: &Mesh,
    render: &RenderMesh,
    world: &[Mat4],
) -> Option<(Vec<V3>, Vec<V3>)> {
    let (positions, columns) = skin_mesh(mesh, world)?;
    let mut out_positions = Vec::with_capacity(render.positions.len());
    let mut out_normals = Vec::with_capacity(render.normals.len());
    for (index, source) in render.source.iter().enumerate() {
        let v = *source as usize;
        out_positions.push(positions[v]);
        let n = render.normals[index];
        let [c0, c1, c2] = columns[v];
        out_normals.push((c0 * n.x + c1 * n.y + c2 * n.z).normalized());
    }
    Some((out_positions, out_normals))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn quad_mesh() -> Mesh {
        let mut mesh = Mesh {
            name: "quad".into(),
            positions: vec![
                V3::new(-1.0, -1.0, 0.0),
                V3::new(1.0, -1.0, 0.0),
                V3::new(1.0, 1.0, 0.0),
                V3::new(-1.0, 1.0, 0.0),
            ],
            ..Default::default()
        };
        mesh.push_face(&[0, 1, 2, 3], None, 0, false);
        mesh
    }

    #[test]
    fn a_quad_becomes_two_triangles_facing_front() {
        let render = RenderMesh::build(&quad_mesh());
        assert_eq!(render.indices.len(), 6);
        assert!(render.normals.iter().all(|n| (n.z - 1.0).abs() < 1.0e-5));
        assert_eq!(render.edges.len(), 4);
    }

    /// A prism with `sides` around, its sides and caps all shaded smooth.
    fn smooth_prism(sides: u32) -> Mesh {
        let mut mesh = Mesh::default();
        for i in 0..sides {
            let angle = i as f32 / sides as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            mesh.positions.push(V3::new(cos, -1.0, sin));
            mesh.positions.push(V3::new(cos, 1.0, sin));
        }
        for i in 0..sides {
            let (a, b) = (i * 2, ((i + 1) % sides) * 2);
            mesh.push_face(&[a, b, b + 1, a + 1], None, 0, true);
        }
        let bottom: Vec<u32> = (0..sides).map(|i| i * 2).collect();
        let top: Vec<u32> = (0..sides).rev().map(|i| i * 2 + 1).collect();
        mesh.push_face(&bottom, None, 0, true);
        mesh.push_face(&top, None, 0, true);
        mesh
    }

    #[test]
    fn a_smooth_box_keeps_its_edges() {
        let mesh = smooth_prism(4);
        let faces: Vec<V3> = mesh
            .faces
            .iter()
            .map(|face| mesh.face_normal(face))
            .collect();
        let render = RenderMesh::build(&mesh);
        // Every corner keeps the normal of its own face, one of the six.
        for normal in &render.normals {
            assert!(
                faces.iter().any(|face| face.dot(*normal) > 0.9999),
                "{normal:?}"
            );
        }
    }

    #[test]
    fn a_smooth_round_side_is_shaded_round() {
        let render = RenderMesh::build(&smooth_prism(16));
        // The sides blend into normals pointing straight out from the axis,
        // while the caps stay flat.
        let mut side = 0;
        for (position, normal) in render.positions.iter().zip(&render.normals) {
            if normal.y.abs() > 0.99 {
                continue;
            }
            side += 1;
            let out = V3::new(position.x, 0.0, position.z).normalized();
            assert!(normal.dot(out).abs() > 0.999, "{normal:?} at {position:?}");
        }
        assert_eq!(side, 32);
    }

    #[test]
    fn a_concave_polygon_is_cut_into_ears() {
        // An L shape.
        let points = [
            V3::new(0.0, 0.0, 0.0),
            V3::new(2.0, 0.0, 0.0),
            V3::new(2.0, 1.0, 0.0),
            V3::new(1.0, 1.0, 0.0),
            V3::new(1.0, 2.0, 0.0),
            V3::new(0.0, 2.0, 0.0),
        ];
        let triangles = triangulate(&points, V3::Z);
        assert_eq!(triangles.len(), 4);
        let area: f32 = triangles
            .iter()
            .map(|[a, b, c]| (points[*b] - points[*a]).cross(points[*c] - points[*a]).z * 0.5)
            .sum();
        assert!((area - 3.0).abs() < 1.0e-4, "{area}");
    }

    #[test]
    fn a_bone_bends_the_vertices_it_holds() {
        let mut model = ModelData::default();
        let mut root = Node::new("root");
        root.bone = true;
        model.nodes.push(root);
        let mut mesh = quad_mesh();
        let mut skin = Skin {
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
            ..Default::default()
        };
        for v in 0..4 {
            skin.set_vertex(v, vec![(0, 1.0)]);
        }
        mesh.skin = Some(skin);
        model.meshes.push(mesh);
        let mut locals = model.rest_locals();
        locals[0].rotation = Quat::from_axis_angle(V3::Z, std::f32::consts::FRAC_PI_2);
        let world = model.world_matrices(&locals);
        let render = RenderMesh::build(&model.meshes[0]);
        let (positions, normals) =
            skinned_render_vertices(&model.meshes[0], &render, &world).unwrap();
        let moved = positions[render.source.iter().position(|s| *s == 1).unwrap()];
        assert!(
            (moved - V3::new(1.0, 1.0, 0.0)).length() < 1.0e-4,
            "{moved:?}"
        );
        assert!(normals.iter().all(|n| (n.z - 1.0).abs() < 1.0e-4));
    }

    #[test]
    fn clips_ease_between_keys() {
        let mut clip = Clip {
            name: "spin".into(),
            duration: 1.0,
            channels: Vec::new(),
        };
        let channel = clip.channel_mut(0, ChannelTarget::Translation);
        channel.set_key(0.0, &[0.0, 0.0, 0.0]);
        channel.set_key(1.0, &[10.0, 0.0, 0.0]);
        assert!((clip.channels[0].sample(0.25)[0] - 2.5).abs() < 1.0e-5);
        assert!((clip.channels[0].sample(5.0)[0] - 10.0).abs() < 1.0e-5);
    }
}
