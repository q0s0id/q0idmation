//! How models, placements in 3D and timeline 3D setups are stored.
//!
//! A model is written as its own blob and deflated, so a project with big
//! meshes and textures stays small on disk.

use std::sync::Arc;

use super::math::{Mat4, Quat, V3};
use super::model::{
    Channel, ChannelTarget, Clip, Face, Material, Mesh, ModelData, Node, Outline, Shading, Skin,
    Texture, Trs,
};
use super::scene::{
    Body3d, BodyShape3d, Camera3d, CameraKey3d, CameraPose, Fog3d, Light3d, Light3dKey,
    Light3dKind, Liquid3d, Physics3d, PoseKey, PoseTrack, Scene3dData, Scene3dSetup, Space3d,
};
use crate::error::Error;
use crate::io::{write_string_u16, Cursor};
use crate::v2::Rgba;

const MODEL_LAYOUT: u8 = 1;
/// 2 adds 3D physics and liquids to each setup.
const SCENE_LAYOUT: u8 = 2;
/// The most a model blob may inflate to.
const MAX_MODEL_BYTES: usize = 1 << 30;
const NONE_U16: u16 = u16::MAX;

struct Out<'a>(&'a mut Vec<u8>);

impl Out<'_> {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn v3(&mut self, v: V3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
    fn quat(&mut self, q: Quat) {
        self.f32(q.x);
        self.f32(q.y);
        self.f32(q.z);
        self.f32(q.w);
    }
    fn rgba(&mut self, c: Rgba) {
        self.0.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    fn string(&mut self, s: &str) -> Result<(), Error> {
        let mut text = s;
        // Names longer than a u16 length are cut at a character boundary.
        if text.len() > usize::from(u16::MAX) {
            let mut end = usize::from(u16::MAX);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text = &text[..end];
        }
        write_string_u16(self.0, text)
    }
    fn count_u16(&mut self, n: usize, what: &'static str) -> Result<(), Error> {
        self.u16(u16::try_from(n).map_err(|_| Error::Overflow(what))?);
        Ok(())
    }
    fn count_u32(&mut self, n: usize, what: &'static str) -> Result<(), Error> {
        self.u32(u32::try_from(n).map_err(|_| Error::Overflow(what))?);
        Ok(())
    }
    fn opt_u16(&mut self, v: Option<u16>) {
        self.u16(v.unwrap_or(NONE_U16));
    }
}

struct In<'a, 'b>(&'a mut Cursor<'b>);

impl In<'_, '_> {
    fn u8(&mut self) -> Result<u8, Error> {
        self.0.read_u8()
    }
    fn bool(&mut self) -> Result<bool, Error> {
        match self.0.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Validation("invalid 3D flag")),
        }
    }
    fn u16(&mut self) -> Result<u16, Error> {
        self.0.read_u16()
    }
    fn u32(&mut self) -> Result<u32, Error> {
        self.0.read_u32()
    }
    fn f32(&mut self) -> Result<f32, Error> {
        self.0.read_f32()
    }
    fn v3(&mut self) -> Result<V3, Error> {
        Ok(V3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn quat(&mut self) -> Result<Quat, Error> {
        Ok(Quat::new(
            self.f32()?,
            self.f32()?,
            self.f32()?,
            self.f32()?,
        ))
    }
    fn rgba(&mut self) -> Result<Rgba, Error> {
        Ok(Rgba {
            r: self.u8()?,
            g: self.u8()?,
            b: self.u8()?,
            a: self.u8()?,
        })
    }
    fn string(&mut self) -> Result<String, Error> {
        self.0.read_string_u16()
    }
    fn i16(&mut self) -> Result<i16, Error> {
        self.0.read_i16()
    }
    fn opt_u16(&mut self) -> Result<Option<u16>, Error> {
        let v = self.u16()?;
        Ok((v != NONE_U16).then_some(v))
    }
    /// A count of items at least `each` bytes long, checked against what
    /// is left so a damaged file cannot ask for a huge allocation.
    fn count_u32(&mut self, each: usize) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        if n.saturating_mul(each.max(1)) > self.0.remaining() {
            return Err(Error::Validation("3D data count runs past the end"));
        }
        Ok(n)
    }
    fn count_u16(&mut self, each: usize) -> Result<usize, Error> {
        let n = usize::from(self.u16()?);
        if n.saturating_mul(each.max(1)) > self.0.remaining() {
            return Err(Error::Validation("3D data count runs past the end"));
        }
        Ok(n)
    }
}

fn deflate_into(out: &mut Vec<u8>, raw: &[u8]) -> Result<(), Error> {
    if raw.len() > MAX_MODEL_BYTES {
        return Err(Error::Overflow("3D data exceeds 1 GiB"));
    }
    let packed = miniz_oxide::deflate::compress_to_vec(raw, 6);
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    out.extend_from_slice(&packed);
    Ok(())
}

fn inflate_from(c: &mut Cursor<'_>) -> Result<Vec<u8>, Error> {
    let len = c.read_u32()? as usize;
    let packed_len = c.read_u32()? as usize;
    let packed = c.read_exact(packed_len)?;
    if len > MAX_MODEL_BYTES || len > packed.len().saturating_mul(1032).saturating_add(64) {
        return Err(Error::Validation("3D data is too large"));
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(packed, len)
        .map_err(|_| Error::Validation("3D data is damaged"))?;
    if raw.len() != len {
        return Err(Error::Validation("3D data is damaged"));
    }
    Ok(raw)
}

pub fn write_model(out: &mut Vec<u8>, model: &ModelData) -> Result<(), Error> {
    let mut raw = Vec::new();
    write_model_raw(&mut Out(&mut raw), model)?;
    deflate_into(out, &raw)
}

pub(crate) fn read_model(c: &mut Cursor<'_>) -> Result<ModelData, Error> {
    let raw = inflate_from(c)?;
    let mut inner = Cursor::new(&raw);
    let model = read_model_raw(&mut In(&mut inner))?;
    inner.finish()?;
    Ok(model)
}

/// The model in `bytes`, as [`write_model`] wrote it.
pub fn model_from_bytes(bytes: &[u8]) -> Result<ModelData, Error> {
    let mut c = Cursor::new(bytes);
    let model = read_model(&mut c)?;
    c.finish()?;
    Ok(model)
}

pub fn model_to_bytes(model: &ModelData) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    write_model(&mut out, model)?;
    Ok(out)
}

fn write_model_raw(o: &mut Out<'_>, model: &ModelData) -> Result<(), Error> {
    o.u8(MODEL_LAYOUT);
    o.f32(model.scale);
    match model.outline {
        Some(outline) => {
            o.u8(1);
            o.f32(outline.width);
            o.rgba(outline.color);
        }
        None => o.u8(0),
    }
    o.count_u32(model.nodes.len(), "too many 3D nodes")?;
    for node in &model.nodes {
        o.string(&node.name)?;
        o.opt_u16(node.parent);
        o.v3(node.translation);
        o.quat(node.rotation);
        o.v3(node.scale);
        o.bool(node.bone);
        o.f32(node.length);
    }
    o.count_u16(model.materials.len(), "too many 3D materials")?;
    for m in &model.materials {
        o.string(&m.name)?;
        o.rgba(m.color);
        o.opt_u16(m.texture);
        o.rgba(m.emissive);
        o.f32(m.roughness);
        o.f32(m.metallic);
        o.u8(match m.shading {
            Shading::Lit => 0,
            Shading::Toon => 1,
            Shading::Unlit => 2,
            Shading::Glass => 3,
        });
        o.u8(m.toon_bands);
        o.bool(m.double_sided);
        match m.alpha_cutoff {
            Some(cutoff) => {
                o.u8(1);
                o.f32(cutoff);
            }
            None => o.u8(0),
        }
    }
    o.count_u16(model.textures.len(), "too many 3D textures")?;
    for t in &model.textures {
        o.string(&t.name)?;
        o.u32(t.width);
        o.u32(t.height);
        o.bool(t.pixelated);
        if t.rgba.len() != (t.width as usize) * (t.height as usize) * 4 {
            return Err(Error::Validation(
                "3D texture size does not match its pixels",
            ));
        }
        o.0.extend_from_slice(&t.rgba);
    }
    o.count_u16(model.meshes.len(), "too many 3D meshes")?;
    for mesh in &model.meshes {
        write_mesh(o, mesh)?;
    }
    o.count_u16(model.clips.len(), "too many 3D clips")?;
    for clip in &model.clips {
        o.string(&clip.name)?;
        o.f32(clip.duration);
        o.count_u32(clip.channels.len(), "too many 3D channels")?;
        for channel in &clip.channels {
            o.u16(channel.node);
            o.u8(match channel.target {
                ChannelTarget::Translation => 0,
                ChannelTarget::Rotation => 1,
                ChannelTarget::Scale => 2,
            });
            o.bool(channel.step);
            o.count_u32(channel.times.len(), "too many 3D keys")?;
            if channel.values.len() != channel.times.len() * channel.target.width() {
                return Err(Error::Validation("3D channel values do not match its keys"));
            }
            for t in &channel.times {
                o.f32(*t);
            }
            for v in &channel.values {
                o.f32(*v);
            }
        }
    }
    Ok(())
}

fn write_mesh(o: &mut Out<'_>, mesh: &Mesh) -> Result<(), Error> {
    o.string(&mesh.name)?;
    o.opt_u16(mesh.node);
    o.bool(mesh.hidden);
    o.count_u32(mesh.positions.len(), "too many 3D vertices")?;
    for p in &mesh.positions {
        o.v3(*p);
    }
    o.count_u32(mesh.faces.len(), "too many 3D faces")?;
    for face in &mesh.faces {
        o.u32(face.start);
        o.u32(face.len);
        o.u16(face.material);
        o.bool(face.smooth);
    }
    o.count_u32(mesh.corner_verts.len(), "too many 3D corners")?;
    for v in &mesh.corner_verts {
        o.u32(*v);
    }
    o.bool(!mesh.corner_uvs.is_empty());
    for uv in &mesh.corner_uvs {
        o.f32(uv[0]);
        o.f32(uv[1]);
    }
    o.bool(!mesh.corner_normals.is_empty());
    for n in &mesh.corner_normals {
        o.v3(*n);
    }
    o.bool(!mesh.colors.is_empty());
    for c in &mesh.colors {
        o.rgba(*c);
    }
    match &mesh.skin {
        None => o.u8(0),
        Some(skin) => {
            o.u8(1);
            o.count_u16(skin.joints.len(), "too many 3D joints")?;
            for joint in &skin.joints {
                o.u16(*joint);
            }
            for m in &skin.inverse_bind {
                for v in m.m {
                    o.f32(v);
                }
            }
            for influence in &skin.influences {
                for j in influence {
                    o.u16(*j);
                }
            }
            for weight in &skin.weights {
                for w in weight {
                    o.f32(*w);
                }
            }
        }
    }
    Ok(())
}

fn read_model_raw(i: &mut In<'_, '_>) -> Result<ModelData, Error> {
    if i.u8()? != MODEL_LAYOUT {
        return Err(Error::Validation("unknown 3D model layout"));
    }
    let scale = i.f32()?;
    let outline = match i.u8()? {
        0 => None,
        1 => Some(Outline {
            width: i.f32()?,
            color: i.rgba()?,
        }),
        _ => return Err(Error::Validation("invalid 3D outline flag")),
    };
    let count = i.count_u32(2 + 2 + 40 + 1 + 4)?;
    let mut nodes = Vec::with_capacity(count);
    for _ in 0..count {
        nodes.push(Node {
            name: i.string()?,
            parent: i.opt_u16()?,
            translation: i.v3()?,
            rotation: i.quat()?,
            scale: i.v3()?,
            bone: i.bool()?,
            length: i.f32()?,
        });
    }
    let count = i.count_u16(2 + 4 + 2 + 4 + 8 + 4)?;
    let mut materials = Vec::with_capacity(count);
    for _ in 0..count {
        let name = i.string()?;
        let color = i.rgba()?;
        let texture = i.opt_u16()?;
        let emissive = i.rgba()?;
        let roughness = i.f32()?;
        let metallic = i.f32()?;
        let shading = match i.u8()? {
            0 => Shading::Lit,
            1 => Shading::Toon,
            2 => Shading::Unlit,
            3 => Shading::Glass,
            _ => return Err(Error::Validation("invalid 3D shading")),
        };
        let toon_bands = i.u8()?;
        let double_sided = i.bool()?;
        let alpha_cutoff = match i.u8()? {
            0 => None,
            1 => Some(i.f32()?),
            _ => return Err(Error::Validation("invalid 3D cutoff flag")),
        };
        materials.push(Material {
            name,
            color,
            texture,
            emissive,
            roughness,
            metallic,
            shading,
            toon_bands,
            double_sided,
            alpha_cutoff,
        });
    }
    let count = i.count_u16(2 + 9)?;
    let mut textures = Vec::with_capacity(count);
    for _ in 0..count {
        let name = i.string()?;
        let width = i.u32()?;
        let height = i.u32()?;
        let pixelated = i.bool()?;
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::Validation("3D texture is too large"))?;
        let rgba = i.0.read_exact(len)?.to_vec();
        let mut texture = Texture::new(name, width, height, rgba);
        texture.pixelated = pixelated;
        textures.push(texture);
    }
    let count = i.count_u16(2 + 2 + 1 + 12)?;
    let mut meshes = Vec::with_capacity(count);
    for _ in 0..count {
        meshes.push(read_mesh(i)?);
    }
    let count = i.count_u16(2 + 4 + 4)?;
    let mut clips = Vec::with_capacity(count);
    for _ in 0..count {
        let name = i.string()?;
        let duration = i.f32()?;
        let channel_count = i.count_u32(2 + 1 + 1 + 4)?;
        let mut channels = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            let node = i.u16()?;
            let target = match i.u8()? {
                0 => ChannelTarget::Translation,
                1 => ChannelTarget::Rotation,
                2 => ChannelTarget::Scale,
                _ => return Err(Error::Validation("invalid 3D channel target")),
            };
            let step = i.bool()?;
            let keys = i.count_u32(4 * (1 + target.width()))?;
            let mut times = Vec::with_capacity(keys);
            for _ in 0..keys {
                times.push(i.f32()?);
            }
            let mut values = Vec::with_capacity(keys * target.width());
            for _ in 0..keys * target.width() {
                values.push(i.f32()?);
            }
            channels.push(Channel {
                node,
                target,
                times,
                values,
                step,
            });
        }
        clips.push(Clip {
            name,
            duration,
            channels,
        });
    }
    let mut model = ModelData {
        nodes,
        meshes,
        materials,
        textures,
        clips,
        scale,
        outline,
    };
    model.sanitize();
    Ok(model)
}

fn read_mesh(i: &mut In<'_, '_>) -> Result<Mesh, Error> {
    let name = i.string()?;
    let node = i.opt_u16()?;
    let hidden = i.bool()?;
    let count = i.count_u32(12)?;
    let mut positions = Vec::with_capacity(count);
    for _ in 0..count {
        positions.push(i.v3()?);
    }
    let count = i.count_u32(11)?;
    let mut faces = Vec::with_capacity(count);
    for _ in 0..count {
        faces.push(Face {
            start: i.u32()?,
            len: i.u32()?,
            material: i.u16()?,
            smooth: i.bool()?,
        });
    }
    let corners = i.count_u32(4)?;
    let mut corner_verts = Vec::with_capacity(corners);
    for _ in 0..corners {
        corner_verts.push(i.u32()?);
    }
    let corner_uvs = if i.bool()? {
        if corners * 8 > i.0.remaining() {
            return Err(Error::Validation("3D data count runs past the end"));
        }
        let mut uvs = Vec::with_capacity(corners);
        for _ in 0..corners {
            uvs.push([i.f32()?, i.f32()?]);
        }
        uvs
    } else {
        Vec::new()
    };
    let corner_normals = if i.bool()? {
        if corners * 12 > i.0.remaining() {
            return Err(Error::Validation("3D data count runs past the end"));
        }
        let mut normals = Vec::with_capacity(corners);
        for _ in 0..corners {
            normals.push(i.v3()?);
        }
        normals
    } else {
        Vec::new()
    };
    let colors = if i.bool()? {
        if positions.len() * 4 > i.0.remaining() {
            return Err(Error::Validation("3D data count runs past the end"));
        }
        let mut colors = Vec::with_capacity(positions.len());
        for _ in 0..positions.len() {
            colors.push(i.rgba()?);
        }
        colors
    } else {
        Vec::new()
    };
    let skin = match i.u8()? {
        0 => None,
        1 => {
            let joints_count = i.count_u16(2 + 64)?;
            let mut joints = Vec::with_capacity(joints_count);
            for _ in 0..joints_count {
                joints.push(i.u16()?);
            }
            let mut inverse_bind = Vec::with_capacity(joints_count);
            for _ in 0..joints_count {
                let mut m = [0.0f32; 16];
                for v in &mut m {
                    *v = i.f32()?;
                }
                inverse_bind.push(Mat4 { m });
            }
            if positions.len() * 24 > i.0.remaining() {
                return Err(Error::Validation("3D data count runs past the end"));
            }
            let mut influences = Vec::with_capacity(positions.len());
            for _ in 0..positions.len() {
                influences.push([i.u16()?, i.u16()?, i.u16()?, i.u16()?]);
            }
            let mut weights = Vec::with_capacity(positions.len());
            for _ in 0..positions.len() {
                weights.push([i.f32()?, i.f32()?, i.f32()?, i.f32()?]);
            }
            Some(Skin {
                joints,
                inverse_bind,
                influences,
                weights,
            })
        }
        _ => return Err(Error::Validation("invalid 3D skin flag")),
    };
    Ok(Mesh {
        name,
        node,
        positions,
        faces,
        corner_verts,
        corner_uvs,
        corner_normals,
        colors,
        skin,
        hidden,
    })
}

pub(crate) fn write_space(out: &mut Vec<u8>, space: Option<Space3d>) {
    let mut o = Out(out);
    match space {
        None => o.u8(0),
        Some(space) => {
            o.u8(1);
            o.f32(space.z);
            o.f32(space.tilt_x);
            o.f32(space.tilt_y);
            o.u16(space.clip);
            o.f32(space.clip_time);
            o.f32(space.clip_speed);
            o.u8(u8::from(space.clip_loop) | (u8::from(space.lit) << 1));
        }
    }
}

pub(crate) fn read_space(c: &mut Cursor<'_>) -> Result<Option<Space3d>, Error> {
    let mut i = In(c);
    match i.u8()? {
        0 => Ok(None),
        1 => {
            let mut space = Space3d {
                z: i.f32()?,
                tilt_x: i.f32()?,
                tilt_y: i.f32()?,
                clip: i.u16()?,
                clip_time: i.f32()?,
                clip_speed: i.f32()?,
                ..Space3d::default()
            };
            let flags = i.u8()?;
            if flags & !3 != 0 {
                return Err(Error::Validation("invalid 3D placement flags"));
            }
            space.clip_loop = flags & 1 != 0;
            space.lit = flags & 2 != 0;
            space.sanitize();
            Ok(Some(space))
        }
        _ => Err(Error::Validation("invalid 3D placement flag")),
    }
}

fn write_pose(o: &mut Out<'_>, pose: &CameraPose) {
    o.v3(pose.position);
    o.v3(pose.target);
    o.f32(pose.roll);
    o.f32(pose.zoom);
}

fn read_pose(i: &mut In<'_, '_>) -> Result<CameraPose, Error> {
    Ok(CameraPose {
        position: i.v3()?,
        target: i.v3()?,
        roll: i.f32()?,
        zoom: i.f32()?,
    })
}

fn write_trs(o: &mut Out<'_>, trs: &Trs) {
    o.v3(trs.translation);
    o.quat(trs.rotation);
    o.v3(trs.scale);
}

fn read_trs(i: &mut In<'_, '_>) -> Result<Trs, Error> {
    Ok(Trs {
        translation: i.v3()?,
        rotation: i.quat()?.normalized(),
        scale: i.v3()?,
    })
}

pub(crate) fn write_scene(out: &mut Vec<u8>, data: &Scene3dData) -> Result<(), Error> {
    let mut o = Out(out);
    o.u8(SCENE_LAYOUT);
    o.count_u16(data.setups.len(), "too many 3D setups")?;
    for setup in &data.setups {
        o.u16(setup.q0rg_id);
        o.bool(setup.camera.enabled);
        o.bool(setup.camera.orthographic);
        write_pose(&mut o, &setup.camera.pose);
        o.count_u16(setup.camera.keys.len(), "too many 3D camera keys")?;
        for key in &setup.camera.keys {
            o.u16(key.frame);
            write_pose(&mut o, &key.pose);
        }
        o.count_u16(setup.lights.len(), "too many 3D lights")?;
        for light in &setup.lights {
            o.u16(light.light_id);
            o.string(&light.name)?;
            o.u8(match light.kind {
                Light3dKind::Point => 0,
                Light3dKind::Spot => 1,
                Light3dKind::Sun => 2,
            });
            o.bool(light.enabled);
            o.v3(light.position);
            o.v3(light.target);
            match light.follow {
                Some(instance) => {
                    o.u8(1);
                    o.u32(instance);
                }
                None => o.u8(0),
            }
            o.rgba(light.color);
            o.f32(light.intensity);
            o.f32(light.range);
            o.f32(light.cone);
            o.f32(light.softness);
            o.bool(light.shadows);
            o.count_u16(light.keys.len(), "too many 3D light keys")?;
            for key in &light.keys {
                o.u16(key.frame);
                o.v3(key.position);
                o.v3(key.target);
                o.rgba(key.color);
                o.f32(key.intensity);
                o.f32(key.range);
            }
        }
        o.rgba(setup.ambient);
        o.f32(setup.ambient_intensity);
        o.bool(setup.light_flat);
        match setup.light_flat_layer {
            Some(layer) => {
                o.u8(1);
                o.u16(layer);
            }
            None => o.u8(0),
        }
        o.bool(setup.shadows);
        write_physics(&mut o, &setup.physics)?;
        o.count_u16(setup.liquids.len(), "too many 3D liquids")?;
        for liquid in &setup.liquids {
            write_liquid(&mut o, liquid)?;
        }
        o.bool(setup.fog.on);
        o.rgba(setup.fog.color);
        o.f32(setup.fog.near);
        o.f32(setup.fog.far);
    }
    o.count_u16(data.poses.len(), "too many 3D pose tracks")?;
    for track in &data.poses {
        o.u16(track.q0rg_id);
        o.u32(track.instance_id);
        o.count_u16(track.keys.len(), "too many 3D pose keys")?;
        for key in &track.keys {
            o.u16(key.frame);
            o.count_u16(key.bones.len(), "too many posed bones")?;
            for (node, trs) in &key.bones {
                o.u16(*node);
                write_trs(&mut o, trs);
            }
        }
    }
    Ok(())
}

fn write_physics(o: &mut Out<'_>, physics: &Physics3d) -> Result<(), Error> {
    o.f32(physics.gravity);
    o.bool(physics.floor);
    o.f32(physics.floor_offset);
    o.count_u16(physics.bodies.len(), "too many 3D bodies")?;
    for body in &physics.bodies {
        o.u32(body.instance_id);
        o.bool(body.falls);
        o.u8(match body.shape {
            BodyShape3d::Box => 0,
            BodyShape3d::Ball => 1,
        });
        o.f32(body.bounce);
        o.f32(body.friction);
        o.f32(body.weight);
        o.v3(body.push);
        o.v3(body.spin);
    }
    Ok(())
}

fn read_physics(i: &mut In<'_, '_>) -> Result<Physics3d, Error> {
    let gravity = i.f32()?;
    let floor = i.bool()?;
    let floor_offset = i.f32()?;
    let count = i.count_u16(4 + 1 + 1 + 12 + 24)?;
    let mut bodies = Vec::with_capacity(count);
    for _ in 0..count {
        bodies.push(Body3d {
            instance_id: i.u32()?,
            falls: i.bool()?,
            shape: match i.u8()? {
                0 => BodyShape3d::Box,
                1 => BodyShape3d::Ball,
                _ => return Err(Error::Validation("invalid 3D body shape")),
            },
            bounce: i.f32()?,
            friction: i.f32()?,
            weight: i.f32()?,
            push: i.v3()?,
            spin: i.v3()?,
        });
    }
    Ok(Physics3d {
        gravity,
        floor,
        floor_offset,
        bodies,
    })
}

fn write_liquid(o: &mut Out<'_>, liquid: &Liquid3d) -> Result<(), Error> {
    o.u16(liquid.liquid_id);
    o.string(&liquid.name)?;
    o.u16(liquid.layer_id);
    o.rgba(liquid.color);
    o.bool(liquid.clear);
    o.v3(liquid.from);
    o.v3(liquid.to);
    o.f32(liquid.spacing);
    o.v3(liquid.splash);
    o.f32(liquid.thickness);
    o.f32(liquid.cling);
    o.bool(liquid.tank);
    o.u16(liquid.start_frame);
    o.count_u16(liquid.frames.len(), "too many baked liquid frames")?;
    for frame in &liquid.frames {
        let count =
            u32::try_from(frame.len()).map_err(|_| Error::Validation("too many liquid drops"))?;
        o.u32(count);
        for drop in frame {
            for axis in drop {
                o.0.extend_from_slice(&axis.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn read_liquid(i: &mut In<'_, '_>) -> Result<Liquid3d, Error> {
    let liquid_id = i.u16()?;
    let name = i.string()?;
    let layer_id = i.u16()?;
    let color = i.rgba()?;
    let clear = i.bool()?;
    let from = i.v3()?;
    let to = i.v3()?;
    let spacing = i.f32()?;
    let splash = i.v3()?;
    let thickness = i.f32()?;
    let cling = i.f32()?;
    let tank = i.bool()?;
    let start_frame = i.u16()?;
    let count = i.count_u16(4)?;
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let drops = i.count_u32(6)?;
        let mut frame = Vec::with_capacity(drops);
        for _ in 0..drops {
            frame.push([i.i16()?, i.i16()?, i.i16()?]);
        }
        frames.push(frame);
    }
    Ok(Liquid3d {
        liquid_id,
        name,
        layer_id,
        color,
        clear,
        from,
        to,
        spacing,
        splash,
        thickness,
        cling,
        tank,
        start_frame,
        frames,
    })
}

pub(crate) fn read_scene(c: &mut Cursor<'_>) -> Result<Scene3dData, Error> {
    let mut i = In(c);
    let layout = i.u8()?;
    if !(1..=SCENE_LAYOUT).contains(&layout) {
        return Err(Error::Validation("unknown 3D setup layout"));
    }
    let count = i.count_u16(2 + 2 + 32 + 2)?;
    let mut setups = Vec::with_capacity(count);
    for _ in 0..count {
        let q0rg_id = i.u16()?;
        let enabled = i.bool()?;
        let orthographic = i.bool()?;
        let pose = read_pose(&mut i)?;
        let key_count = i.count_u16(2 + 32)?;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            keys.push(CameraKey3d {
                frame: i.u16()?,
                pose: read_pose(&mut i)?,
            });
        }
        let light_count = i.count_u16(2 + 2 + 2 + 24)?;
        let mut lights = Vec::with_capacity(light_count);
        for _ in 0..light_count {
            let light_id = i.u16()?;
            let name = i.string()?;
            let kind = match i.u8()? {
                0 => Light3dKind::Point,
                1 => Light3dKind::Spot,
                2 => Light3dKind::Sun,
                _ => return Err(Error::Validation("invalid 3D light kind")),
            };
            let enabled = i.bool()?;
            let position = i.v3()?;
            let target = i.v3()?;
            let follow = match i.u8()? {
                0 => None,
                1 => Some(i.u32()?),
                _ => return Err(Error::Validation("invalid 3D light follow flag")),
            };
            let color = i.rgba()?;
            let intensity = i.f32()?;
            let range = i.f32()?;
            let cone = i.f32()?;
            let softness = i.f32()?;
            let shadows = i.bool()?;
            let key_count = i.count_u16(2 + 24 + 4 + 8)?;
            let mut keys = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                keys.push(Light3dKey {
                    frame: i.u16()?,
                    position: i.v3()?,
                    target: i.v3()?,
                    color: i.rgba()?,
                    intensity: i.f32()?,
                    range: i.f32()?,
                });
            }
            lights.push(Light3d {
                light_id,
                name,
                kind,
                enabled,
                position,
                target,
                follow,
                color,
                intensity,
                range,
                cone,
                softness,
                shadows,
                keys,
            });
        }
        let ambient = i.rgba()?;
        let ambient_intensity = i.f32()?;
        let light_flat = i.bool()?;
        let light_flat_layer = match i.u8()? {
            0 => None,
            1 => Some(i.u16()?),
            _ => return Err(Error::Validation("invalid 3D layer flag")),
        };
        let shadows = i.bool()?;
        let (physics, liquids, fog) = if layout >= 2 {
            let physics = read_physics(&mut i)?;
            let count = i.count_u16(2 + 2 + 2 + 4 + 1 + 24 + 4 + 12 + 8 + 1 + 2 + 2)?;
            let mut liquids = Vec::with_capacity(count);
            for _ in 0..count {
                liquids.push(read_liquid(&mut i)?);
            }
            let fog = Fog3d {
                on: i.bool()?,
                color: i.rgba()?,
                near: i.f32()?,
                far: i.f32()?,
            };
            (physics, liquids, fog)
        } else {
            (Physics3d::default(), Vec::new(), Fog3d::default())
        };
        setups.push(Scene3dSetup {
            q0rg_id,
            camera: Camera3d {
                enabled,
                orthographic,
                pose,
                keys,
            },
            lights,
            ambient,
            ambient_intensity,
            light_flat,
            light_flat_layer,
            shadows,
            physics,
            liquids,
            fog,
        });
    }
    let count = i.count_u16(2 + 4 + 2)?;
    let mut poses = Vec::with_capacity(count);
    for _ in 0..count {
        let q0rg_id = i.u16()?;
        let instance_id = i.u32()?;
        let key_count = i.count_u16(4)?;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            let frame = i.u16()?;
            let bone_count = i.count_u16(2 + 40)?;
            let mut bones = Vec::with_capacity(bone_count);
            for _ in 0..bone_count {
                bones.push((i.u16()?, read_trs(&mut i)?));
            }
            keys.push(PoseKey { frame, bones });
        }
        poses.push(PoseTrack {
            q0rg_id,
            instance_id,
            keys,
        });
    }
    let mut data = Scene3dData { setups, poses };
    data.sanitize();
    Ok(data)
}

/// Models in JSON: their stored bytes as base64.
#[cfg(feature = "serde")]
pub mod serde_model {
    use super::*;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        model: &Arc<ModelData>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let bytes = model_to_bytes(model).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<ModelData>, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)?;
        model_from_bytes(&bytes)
            .map(Arc::new)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(not(feature = "serde"))]
#[allow(dead_code)]
fn _keep_arc(_: Arc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample_model() -> ModelData {
        let mut model = ModelData {
            outline: Some(Outline {
                width: 2.0,
                color: Rgba {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                },
            }),
            ..ModelData::default()
        };
        let mut root = Node::new("hips");
        root.bone = true;
        root.length = 3.0;
        model.nodes.push(root);
        let mut child = Node::new("spine");
        child.parent = Some(0);
        child.translation = V3::new(0.0, 3.0, 0.0);
        child.rotation = Quat::from_euler_deg(0.0, 0.0, 10.0);
        model.nodes.push(child);
        model.textures.push(Texture::new(
            "tex",
            2,
            1,
            vec![255, 0, 0, 255, 0, 255, 0, 128],
        ));
        model.materials[0].texture = Some(0);
        let mut mesh = crate::three::model::Mesh {
            name: "body".into(),
            positions: vec![V3::ZERO, V3::X, V3::Y],
            ..Default::default()
        };
        mesh.push_face(
            &[0, 1, 2],
            Some(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
            0,
            true,
        );
        let mut skin = Skin {
            joints: vec![0, 1],
            inverse_bind: vec![Mat4::IDENTITY, Mat4::translation(V3::new(0.0, -3.0, 0.0))],
            ..Default::default()
        };
        for v in 0..3 {
            skin.set_vertex(v, vec![(0, 0.75), (1, 0.25)]);
        }
        mesh.skin = Some(skin);
        model.meshes.push(mesh);
        let mut clip = Clip {
            name: "bend".into(),
            duration: 1.0,
            channels: Vec::new(),
        };
        clip.channel_mut(1, ChannelTarget::Rotation)
            .set_key(0.5, &[0.0, 0.0, 0.3, 0.95]);
        model.clips.push(clip);
        model.sanitize();
        model
    }

    #[test]
    fn a_model_comes_back_as_it_went() {
        let model = sample_model();
        let bytes = model_to_bytes(&model).unwrap();
        assert_eq!(model_from_bytes(&bytes).unwrap(), model);
    }

    #[test]
    fn a_damaged_model_is_refused() {
        let bytes = model_to_bytes(&sample_model()).unwrap();
        for cut in [3, bytes.len() / 2, bytes.len() - 1] {
            assert!(model_from_bytes(&bytes[..cut]).is_err());
        }
    }

    #[test]
    fn bodies_and_liquids_come_back_as_they_went() {
        use super::super::scene::{Body3d, Liquid3d};
        let mut setup = Scene3dSetup::new(1, 640, 480);
        let mut body = Body3d::new(7);
        body.falls = false;
        body.shape = BodyShape3d::Ball;
        body.push = V3::new(10.0, -20.0, 30.0);
        setup.physics.bodies.push(body);
        setup.physics.floor_offset = 12.5;
        let mut liquid = Liquid3d::new(3, 2, V3::new(1.0, 2.0, 3.0), V3::new(40.0, 50.0, 60.0));
        liquid.frames = vec![vec![[1, -2, 3], [400, 300, -200]], vec![]];
        liquid.start_frame = 5;
        setup.liquids.push(liquid);
        let data = Scene3dData {
            setups: vec![setup],
            poses: Vec::new(),
        };
        let mut out = Vec::new();
        write_scene(&mut out, &data).unwrap();
        let mut cursor = Cursor::new(&out);
        let back = read_scene(&mut cursor).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn scenes_come_back_as_they_went() {
        let mut data = Scene3dData::default();
        let setup = data.setup_mut(1, 640, 480);
        setup.camera.enabled = true;
        setup.camera.set_key(12);
        let mut light = Light3d::new(1, Light3dKind::Spot, V3::new(1.0, 2.0, 3.0), V3::ZERO);
        light.follow = Some(7);
        light.set_key(4);
        setup.lights.push(light);
        setup.light_flat = true;
        setup.light_flat_layer = Some(3);
        data.pose_mut(1, 9).key_at_mut(5).bones.push((
            1,
            Trs {
                translation: V3::X,
                rotation: Quat::from_euler_deg(10.0, 0.0, 0.0),
                scale: V3::ONE,
            },
        ));
        let mut out = Vec::new();
        write_scene(&mut out, &data).unwrap();
        let mut c = Cursor::new(&out);
        let back = read_scene(&mut c).unwrap();
        c.finish().unwrap();
        assert_eq!(back, data);
    }
}
