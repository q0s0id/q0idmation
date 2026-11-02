//! FBX import: binary (6.1 to 7.7) and ASCII files, with meshes, materials,
//! embedded or nearby textures, the node tree, skins and animation stacks.

use std::collections::HashMap;
use std::path::Path;

use q0s_format::three::model::{
    Channel, ChannelTarget, Clip, Material, Mesh, ModelData, Node, Skin, Texture,
};
use q0s_format::three::{Mat4, Quat, V3};
use q0s_format::v2::Rgba;

use crate::Imported;

/// One FBX record: a name, its values and the records inside it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FbxNode {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<FbxNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Raw(Vec<u8>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
}

impl Prop {
    pub fn int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Float(v) => Some(*v as i64),
            Self::Bool(v) => Some(i64::from(*v)),
            _ => None,
        }
    }

    pub fn float(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Float(v) => Some(*v),
            Self::Bool(v) => Some(f64::from(u8::from(*v))),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn floats(&self) -> Vec<f64> {
        match self {
            Self::Floats(v) => v.clone(),
            Self::Ints(v) => v.iter().map(|x| *x as f64).collect(),
            Self::Int(v) => vec![*v as f64],
            Self::Float(v) => vec![*v],
            _ => Vec::new(),
        }
    }

    pub fn ints(&self) -> Vec<i64> {
        match self {
            Self::Ints(v) => v.clone(),
            Self::Floats(v) => v.iter().map(|x| *x as i64).collect(),
            Self::Int(v) => vec![*v],
            _ => Vec::new(),
        }
    }
}

impl FbxNode {
    pub fn child(&self, name: &str) -> Option<&FbxNode> {
        self.children.iter().find(|child| child.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a FbxNode> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// All numbers of a child array record, wherever its values sit.
    fn array_f64(&self, name: &str) -> Vec<f64> {
        let Some(child) = self.child(name) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for prop in &child.props {
            out.extend(prop.floats());
        }
        // ASCII 7.x keeps the values in an `a:` record.
        if let Some(a) = child.child("a") {
            for prop in &a.props {
                out.extend(prop.floats());
            }
        }
        out
    }

    fn array_i64(&self, name: &str) -> Vec<i64> {
        let Some(child) = self.child(name) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for prop in &child.props {
            out.extend(prop.ints());
        }
        if let Some(a) = child.child("a") {
            for prop in &a.props {
                out.extend(prop.ints());
            }
        }
        out
    }

    fn first_str(&self, name: &str) -> Option<&str> {
        self.child(name)?.props.first()?.str()
    }
}

const BINARY_MAGIC: &[u8] = b"Kaydara FBX Binary  \0";

/// Parses an FBX file into its record tree.
pub fn parse(bytes: &[u8]) -> Result<Vec<FbxNode>, String> {
    if bytes.starts_with(BINARY_MAGIC) {
        parse_binary(bytes)
    } else {
        let text = String::from_utf8_lossy(bytes);
        parse_ascii(&text)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or("FBX ends early")?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn parse_binary(bytes: &[u8]) -> Result<Vec<FbxNode>, String> {
    let mut r = Reader {
        data: bytes,
        pos: 23,
    };
    let version = r.u32()?;
    let wide = version >= 7500;
    let mut nodes = Vec::new();
    while r.pos < bytes.len() {
        match read_binary_node(&mut r, wide, 0)? {
            Some(node) => nodes.push(node),
            None => break,
        }
    }
    Ok(nodes)
}

fn read_binary_node(
    r: &mut Reader<'_>,
    wide: bool,
    depth: usize,
) -> Result<Option<FbxNode>, String> {
    if depth > 64 {
        return Err("FBX nests too deeply".into());
    }
    let (end, count, _list_len) = if wide {
        (r.u64()? as usize, r.u64()? as usize, r.u64()? as usize)
    } else {
        (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize)
    };
    let name_len = r.u8()? as usize;
    if end == 0 {
        return Ok(None);
    }
    if end > r.data.len() || end < r.pos {
        return Err("FBX record runs past the end".into());
    }
    let name = String::from_utf8_lossy(r.take(name_len)?).into_owned();
    let mut props = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        props.push(read_binary_prop(r)?);
    }
    let mut children = Vec::new();
    while r.pos < end {
        match read_binary_node(r, wide, depth + 1)? {
            Some(child) => children.push(child),
            None => break,
        }
    }
    r.pos = end;
    Ok(Some(FbxNode {
        name,
        props,
        children,
    }))
}

fn read_binary_prop(r: &mut Reader<'_>) -> Result<Prop, String> {
    let kind = r.u8()?;
    Ok(match kind {
        b'Y' => Prop::Int(i64::from(i16::from_le_bytes(
            r.take(2)?.try_into().unwrap(),
        ))),
        b'C' => Prop::Bool(r.u8()? != 0),
        b'I' => Prop::Int(i64::from(i32::from_le_bytes(
            r.take(4)?.try_into().unwrap(),
        ))),
        b'F' => Prop::Float(f64::from(f32::from_le_bytes(
            r.take(4)?.try_into().unwrap(),
        ))),
        b'D' => Prop::Float(f64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        b'L' => Prop::Int(i64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        b'S' => {
            let len = r.u32()? as usize;
            let raw = r.take(len)?;
            // "Name\0\x01Class" names are written "Class::Name".
            let text = match raw.windows(2).position(|w| w == [0, 1]) {
                Some(split) => format!(
                    "{}::{}",
                    String::from_utf8_lossy(&raw[split + 2..]),
                    String::from_utf8_lossy(&raw[..split])
                ),
                None => String::from_utf8_lossy(raw).into_owned(),
            };
            Prop::Str(text)
        }
        b'R' => {
            let len = r.u32()? as usize;
            Prop::Raw(r.take(len)?.to_vec())
        }
        b'f' | b'd' | b'l' | b'i' | b'b' => {
            let len = r.u32()? as usize;
            let encoding = r.u32()?;
            let compressed = r.u32()? as usize;
            let width = match kind {
                b'f' | b'i' => 4,
                b'd' | b'l' => 8,
                _ => 1,
            };
            let raw = r.take(compressed)?;
            let data = if encoding == 1 {
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                    raw,
                    len.saturating_mul(width).max(1),
                )
                .map_err(|_| "FBX array is damaged".to_string())?
            } else {
                raw.to_vec()
            };
            if data.len() < len * width {
                return Err("FBX array is short".into());
            }
            match kind {
                b'f' => Prop::Floats(
                    data.as_chunks::<4>()
                        .0
                        .iter()
                        .take(len)
                        .map(|c| f64::from(f32::from_le_bytes(*c)))
                        .collect(),
                ),
                b'd' => Prop::Floats(
                    data.as_chunks::<8>()
                        .0
                        .iter()
                        .take(len)
                        .map(|c| f64::from_le_bytes(*c))
                        .collect(),
                ),
                b'l' => Prop::Ints(
                    data.as_chunks::<8>()
                        .0
                        .iter()
                        .take(len)
                        .map(|c| i64::from_le_bytes(*c))
                        .collect(),
                ),
                b'i' => Prop::Ints(
                    data.as_chunks::<4>()
                        .0
                        .iter()
                        .take(len)
                        .map(|c| i64::from(i32::from_le_bytes(*c)))
                        .collect(),
                ),
                _ => Prop::Ints(data.iter().take(len).map(|b| i64::from(*b)).collect()),
            }
        }
        other => return Err(format!("unknown FBX value type {:?}", other as char)),
    })
}

struct Ascii<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Ascii<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    /// Skips spaces, and newlines and comments too when `lines` is set.
    fn skip(&mut self, lines: bool) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' if lines => self.pos += 1,
                b';' if lines => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn nodes(&mut self, depth: usize) -> Result<Vec<FbxNode>, String> {
        if depth > 64 {
            return Err("FBX nests too deeply".into());
        }
        let mut out = Vec::new();
        loop {
            self.skip(true);
            match self.peek() {
                None => return Ok(out),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(out);
                }
                _ => {}
            }
            let start = self.pos;
            while self
                .peek()
                .is_some_and(|c| c != b':' && c != b'\n' && c != b'{' && c != b'}')
            {
                self.pos += 1;
            }
            if self.peek() != Some(b':') {
                // Not a record: skip the line.
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.pos += 1;
                }
                continue;
            }
            let name = String::from_utf8_lossy(&self.s[start..self.pos])
                .trim()
                .to_string();
            self.pos += 1;
            let props = self.values()?;
            self.skip(false);
            let children = if self.peek() == Some(b'{') {
                self.pos += 1;
                self.nodes(depth + 1)?
            } else {
                Vec::new()
            };
            out.push(FbxNode {
                name,
                props,
                children,
            });
            let _ = start;
        }
    }

    fn values(&mut self) -> Result<Vec<Prop>, String> {
        let mut props = Vec::new();
        let mut after_comma = false;
        loop {
            self.skip(false);
            match self.peek() {
                None | Some(b'}') => break,
                Some(b'\n') => {
                    if after_comma {
                        self.pos += 1;
                        continue;
                    }
                    break;
                }
                Some(b';') => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'{') => break,
                Some(b',') => {
                    self.pos += 1;
                    after_comma = true;
                    continue;
                }
                Some(b'"') => {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|c| c != b'"') {
                        self.pos += 1;
                    }
                    props.push(Prop::Str(
                        String::from_utf8_lossy(&self.s[start..self.pos]).into_owned(),
                    ));
                    self.pos += 1;
                }
                Some(b'*') => {
                    // `*N { a: values }`
                    self.pos += 1;
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    self.skip(true);
                    if self.peek() == Some(b'{') {
                        self.pos += 1;
                        let mut floats = Vec::new();
                        let mut all_ints = true;
                        loop {
                            self.skip(true);
                            match self.peek() {
                                None => break,
                                Some(b'}') => {
                                    self.pos += 1;
                                    break;
                                }
                                Some(b',') => self.pos += 1,
                                _ => {
                                    let token = self.token();
                                    if token.ends_with(':') || token.is_empty() {
                                        if token.is_empty() {
                                            self.pos += 1;
                                        }
                                        continue;
                                    }
                                    if token.contains(['.', 'e', 'E']) {
                                        all_ints = false;
                                    }
                                    if let Ok(v) = token.parse::<f64>() {
                                        floats.push(v);
                                    }
                                }
                            }
                        }
                        props.push(if all_ints {
                            Prop::Ints(floats.iter().map(|v| *v as i64).collect())
                        } else {
                            Prop::Floats(floats)
                        });
                    }
                }
                _ => {
                    let token = self.token();
                    if token.is_empty() {
                        self.pos += 1;
                        continue;
                    }
                    props.push(if let Ok(v) = token.parse::<i64>() {
                        Prop::Int(v)
                    } else if let Ok(v) = token.parse::<f64>() {
                        Prop::Float(v)
                    } else {
                        Prop::Str(token)
                    });
                }
            }
            after_comma = false;
        }
        Ok(props)
    }

    fn token(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| !matches!(c, b',' | b'\n' | b'\r' | b' ' | b'\t' | b'{' | b'}' | b'"'))
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.pos]).into_owned()
    }
}

fn parse_ascii(text: &str) -> Result<Vec<FbxNode>, String> {
    let mut parser = Ascii {
        s: text.as_bytes(),
        pos: 0,
    };
    let nodes = parser.nodes(0)?;
    if !nodes.iter().any(|node| node.name == "Objects") {
        return Err("not an FBX file".into());
    }
    Ok(nodes)
}

/// Named properties of an object (`Properties70` or `Properties60`).
struct Props<'a> {
    map: HashMap<&'a str, &'a [Prop]>,
}

impl<'a> Props<'a> {
    fn of(node: &'a FbxNode) -> Self {
        let mut map = HashMap::new();
        for block in ["Properties70", "Properties60"] {
            if let Some(props) = node.child(block) {
                for p in &props.children {
                    let skip = if p.name == "P" { 4 } else { 3 };
                    if let Some(name) = p.props.first().and_then(Prop::str) {
                        map.insert(name, p.props.get(skip..).unwrap_or(&[]));
                    }
                }
            }
        }
        Self { map }
    }

    fn v3(&self, name: &str, default: V3) -> V3 {
        match self.map.get(name) {
            Some(values) if values.len() >= 3 => V3::new(
                values[0].float().unwrap_or(0.0) as f32,
                values[1].float().unwrap_or(0.0) as f32,
                values[2].float().unwrap_or(0.0) as f32,
            ),
            _ => default,
        }
    }

    fn f(&self, name: &str, default: f64) -> f64 {
        self.map
            .get(name)
            .and_then(|values| values.first())
            .and_then(Prop::float)
            .unwrap_or(default)
    }

    fn i(&self, name: &str, default: i64) -> i64 {
        self.map
            .get(name)
            .and_then(|values| values.first())
            .and_then(Prop::int)
            .unwrap_or(default)
    }

    fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
}

/// An object's identity: its number, or in old files its name.
type Key = i64;

fn key_of(prop: &Prop) -> Option<Key> {
    match prop {
        Prop::Int(v) => Some(*v),
        Prop::Str(s) => {
            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            for b in s.bytes() {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x100_0000_01b3);
            }
            Some((hash >> 1) as i64 | (1 << 62))
        }
        _ => None,
    }
}

struct Object<'a> {
    class: &'a str,
    name: String,
    kind: String,
    node: &'a FbxNode,
}

/// Rotation by Euler angles in degrees in FBX's rotation orders.
fn euler(order: i64, v: V3) -> Quat {
    let x = Quat::from_axis_angle(V3::X, v.x.to_radians());
    let y = Quat::from_axis_angle(V3::Y, v.y.to_radians());
    let z = Quat::from_axis_angle(V3::Z, v.z.to_radians());
    match order {
        1 => y * z * x, // XZY
        2 => x * z * y, // YZX
        3 => z * x * y, // YXZ
        4 => y * x * z, // ZXY
        5 => x * y * z, // ZYX
        _ => z * y * x, // XYZ
    }
}

/// A Model's static transform settings.
struct Xform {
    order: i64,
    roff: V3,
    rp: V3,
    pre: Quat,
    post_inv: Quat,
    soff: V3,
    sp: V3,
}

impl Xform {
    fn of(props: &Props<'_>) -> Self {
        let enabled = props.i("RotationActive", 1) != 0 || props.has("PreRotation");
        let pre = if enabled {
            euler(0, props.v3("PreRotation", V3::ZERO))
        } else {
            Quat::IDENTITY
        };
        let post = if enabled {
            euler(0, props.v3("PostRotation", V3::ZERO))
        } else {
            Quat::IDENTITY
        };
        Self {
            order: props.i("RotationOrder", 0),
            roff: props.v3("RotationOffset", V3::ZERO),
            rp: props.v3("RotationPivot", V3::ZERO),
            pre,
            post_inv: post.conjugate(),
            soff: props.v3("ScalingOffset", V3::ZERO),
            sp: props.v3("ScalingPivot", V3::ZERO),
        }
    }

    /// L = T Roff Rp Rpre R Rpost⁻¹ Rp⁻¹ Soff Sp S Sp⁻¹
    fn matrix(&self, t: V3, r: V3, s: V3) -> Mat4 {
        let rotation = self.pre * euler(self.order, r) * self.post_inv;
        Mat4::translation(t + self.roff + self.rp)
            * Mat4::from_quat(rotation)
            * Mat4::translation(-self.rp + self.soff + self.sp)
            * Mat4::scale(s)
            * Mat4::translation(-self.sp)
    }
}

const TICKS_PER_SECOND: f64 = 46_186_158_000.0;

fn rgba(v: V3, alpha: f32) -> Rgba {
    let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba {
        r: c(v.x),
        g: c(v.y),
        b: c(v.z),
        a: c(alpha),
    }
}

fn decode_image(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
}

/// Reads an FBX file into a model.
pub fn import(bytes: &[u8], base_dir: Option<&Path>) -> Result<Imported, String> {
    let nodes = parse(bytes)?;
    let mut warnings = Vec::new();
    let objects_node = nodes
        .iter()
        .find(|node| node.name == "Objects")
        .ok_or("the FBX file has no objects")?;
    let mut objects: HashMap<Key, Object<'_>> = HashMap::new();
    for node in &objects_node.children {
        let (key, name, kind) = match node.props.first() {
            Some(Prop::Int(_)) => (
                key_of(&node.props[0]),
                node.props.get(1).and_then(Prop::str).unwrap_or(""),
                node.props.get(2).and_then(Prop::str).unwrap_or(""),
            ),
            Some(Prop::Str(_)) => (
                key_of(&node.props[0]),
                node.props[0].str().unwrap_or(""),
                node.props.get(1).and_then(Prop::str).unwrap_or(""),
            ),
            _ => continue,
        };
        let Some(key) = key else { continue };
        let name = name.split_once("::").map_or(name, |(_, n)| n).to_string();
        objects.insert(
            key,
            Object {
                class: node.name.as_str(),
                name,
                kind: kind.to_string(),
                node,
            },
        );
    }
    // child -> [(parent, property)]
    let mut parents: HashMap<Key, Vec<(Key, Option<String>)>> = HashMap::new();
    let mut children: HashMap<Key, Vec<(Key, Option<String>)>> = HashMap::new();
    if let Some(connections) = nodes.iter().find(|node| node.name == "Connections") {
        for c in &connections.children {
            if c.name != "C" && c.name != "Connect" {
                continue;
            }
            let (Some(child), Some(parent)) = (
                c.props.get(1).and_then(key_of),
                c.props.get(2).and_then(key_of),
            ) else {
                continue;
            };
            let property = c.props.get(3).and_then(Prop::str).map(str::to_string);
            parents
                .entry(child)
                .or_default()
                .push((parent, property.clone()));
            children.entry(parent).or_default().push((child, property));
        }
    }
    let children_of = |key: Key, class: &str| -> Vec<(Key, Option<String>)> {
        children
            .get(&key)
            .map(|list| {
                list.iter()
                    .filter(|(child, _)| objects.get(child).is_some_and(|o| o.class == class))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };

    // Global axes and units.
    let settings = nodes
        .iter()
        .find(|node| node.name == "GlobalSettings")
        .map(Props::of);
    let axis = |name: &str, default: i64| settings.as_ref().map_or(default, |p| p.i(name, default));
    let unit = |v: i64| -> V3 {
        match v {
            0 => V3::X,
            1 => V3::Y,
            _ => V3::Z,
        }
    };
    let up = unit(axis("UpAxis", 1)) * axis("UpAxisSign", 1).signum() as f32;
    let front = unit(axis("FrontAxis", 2)) * axis("FrontAxisSign", 1).signum() as f32;
    let coord = unit(axis("CoordAxis", 0)) * axis("CoordAxisSign", 1).signum() as f32;
    // Rows: file axes onto ours (x right, y up, z toward the viewer).
    let mut conversion = Mat4::from_rows([
        [coord.x, coord.y, coord.z, 0.0],
        [up.x, up.y, up.z, 0.0],
        [front.x, front.y, front.z, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    if conversion.determinant3().abs() < 0.5 {
        conversion = Mat4::IDENTITY;
    }

    // The node tree: every Model except cameras and lights.
    let mut model_keys: Vec<Key> = objects
        .iter()
        .filter(|(_, o)| {
            o.class == "Model" && !matches!(o.kind.as_str(), "Camera" | "Light" | "CameraSwitcher")
        })
        .map(|(key, _)| *key)
        .collect();
    model_keys.sort_unstable();
    let model_parent = |key: Key| -> Option<Key> {
        parents.get(&key)?.iter().find_map(|(parent, _)| {
            (*parent != 0 && model_keys.binary_search(parent).is_ok()).then_some(*parent)
        })
    };
    // Parents first.
    let mut ordered: Vec<Key> = Vec::new();
    let mut placed: std::collections::HashSet<Key> = std::collections::HashSet::new();
    fn visit(
        key: Key,
        parent_of: &dyn Fn(Key) -> Option<Key>,
        placed: &mut std::collections::HashSet<Key>,
        ordered: &mut Vec<Key>,
        depth: usize,
    ) {
        if placed.contains(&key) || depth > 256 {
            return;
        }
        if let Some(parent) = parent_of(key) {
            visit(parent, parent_of, placed, ordered, depth + 1);
        }
        if placed.insert(key) {
            ordered.push(key);
        }
    }
    for key in &model_keys {
        visit(*key, &model_parent, &mut placed, &mut ordered, 0);
    }
    let mut model = ModelData {
        materials: Vec::new(),
        ..ModelData::default()
    };
    let mut root = Node::new("FBX");
    let (_, rotation, scale) = conversion.decompose();
    root.rotation = rotation;
    root.scale = scale;
    model.nodes.push(root);
    let mut node_index: HashMap<Key, u16> = HashMap::new();
    let mut xforms: HashMap<Key, Xform> = HashMap::new();
    let mut bind_globals: HashMap<Key, Mat4> = HashMap::new();
    for key in &ordered {
        let object = &objects[key];
        let props = Props::of(object.node);
        let xform = Xform::of(&props);
        let local = xform.matrix(
            props.v3("Lcl Translation", V3::ZERO),
            props.v3("Lcl Rotation", V3::ZERO),
            props.v3("Lcl Scaling", V3::ONE),
        );
        let (t, r, s) = local.decompose();
        let mut node = Node::new(object.name.clone());
        node.translation = t;
        node.rotation = r;
        node.scale = s;
        node.bone = matches!(
            object.kind.as_str(),
            "LimbNode" | "Limb" | "Root" | "Skeleton"
        );
        let parent = model_parent(*key).and_then(|p| node_index.get(&p).copied());
        node.parent = Some(parent.unwrap_or(0));
        let parent_global = model_parent(*key)
            .and_then(|p| bind_globals.get(&p).copied())
            .unwrap_or(Mat4::IDENTITY);
        bind_globals.insert(*key, parent_global * local);
        node_index.insert(*key, model.nodes.len() as u16);
        model.nodes.push(node);
        xforms.insert(*key, xform);
    }

    // Textures, by object.
    let mut texture_cache: HashMap<Key, Option<u16>> = HashMap::new();
    let mut load_texture =
        |texture_key: Key, model: &mut ModelData, warnings: &mut Vec<String>| -> Option<u16> {
            if let Some(found) = texture_cache.get(&texture_key) {
                return *found;
            }
            let object = objects.get(&texture_key)?;
            let mut bytes: Option<Vec<u8>> = None;
            for (video, _) in children_of(texture_key, "Video") {
                if let Some(content) = objects[&video].node.child("Content") {
                    for prop in &content.props {
                        if let Prop::Raw(raw) = prop {
                            if !raw.is_empty() {
                                bytes = Some(raw.clone());
                            }
                        }
                    }
                }
                if bytes.is_none() {
                    if let Some(file) = objects[&video]
                        .node
                        .first_str("RelativeFilename")
                        .or_else(|| objects[&video].node.first_str("Filename"))
                    {
                        bytes = read_near(base_dir, file);
                    }
                }
            }
            let file = object
                .node
                .first_str("RelativeFilename")
                .or_else(|| object.node.first_str("FileName"))
                .map(str::to_string);
            if bytes.is_none() {
                if let Some(file) = &file {
                    bytes = read_near(base_dir, file);
                }
            }
            let result = match bytes.and_then(|b| decode_image(&b)) {
                Some((w, h, rgba)) => {
                    let name = file
                        .as_deref()
                        .and_then(|f| f.rsplit(['/', '\\']).next())
                        .unwrap_or(&object.name)
                        .to_string();
                    model.textures.push(Texture::new(name, w, h, rgba));
                    Some((model.textures.len() - 1) as u16)
                }
                None => {
                    warnings.push(format!(
                        "texture {} was not found",
                        file.unwrap_or_else(|| object.name.clone())
                    ));
                    None
                }
            };
            texture_cache.insert(texture_key, result);
            result
        };

    // Materials, by object.
    let mut material_index: HashMap<Key, u16> = HashMap::new();
    let mut material_for =
        |material_key: Key, model: &mut ModelData, warnings: &mut Vec<String>| -> u16 {
            if let Some(index) = material_index.get(&material_key) {
                return *index;
            }
            let object = &objects[&material_key];
            let props = Props::of(object.node);
            let diffuse = if props.has("DiffuseColor") {
                props.v3("DiffuseColor", V3::splat(0.8))
            } else {
                props.v3("Diffuse", V3::splat(0.8))
            };
            let factor = props.f("DiffuseFactor", 1.0) as f32;
            let diffuse = if factor > 0.0 {
                diffuse * factor.min(1.0)
            } else {
                diffuse
            };
            let opacity = if props.has("Opacity") {
                props.f("Opacity", 1.0) as f32
            } else {
                1.0 - props.f("TransparencyFactor", 0.0) as f32
            };
            let opacity = if opacity <= 0.0 {
                1.0
            } else {
                opacity.clamp(0.0, 1.0)
            };
            let emissive =
                props.v3("EmissiveColor", V3::ZERO) * props.f("EmissiveFactor", 1.0) as f32;
            let shininess = props.f("ShininessExponent", props.f("Shininess", 20.0)) as f32;
            let mut material = Material::with_color(object.name.clone(), rgba(diffuse, opacity));
            material.emissive = rgba(emissive, 1.0);
            material.roughness = (1.0 - (shininess.max(0.0) / 100.0).sqrt()).clamp(0.05, 1.0);
            for (texture, property) in children_of(material_key, "Texture") {
                let wanted = property.as_deref().is_none_or(|p| {
                    p.contains("Diffuse") || p.contains("BaseColor") || p.contains("Base")
                });
                if wanted && material.texture.is_none() {
                    material.texture = load_texture(texture, model, warnings);
                    if material.texture.is_some() {
                        // A textured surface takes its colour from the picture.
                        material.color = rgba(V3::ONE, opacity);
                    }
                }
            }
            model.materials.push(material);
            let index = (model.materials.len() - 1) as u16;
            material_index.insert(material_key, index);
            index
        };

    // Meshes.
    for key in &ordered {
        let object = &objects[key];
        for (geometry_key, _) in children_of(*key, "Geometry") {
            let geometry = &objects[&geometry_key];
            if geometry.kind != "Mesh"
                && !geometry.node.children.iter().any(|c| c.name == "Vertices")
            {
                continue;
            }
            let props = Props::of(object.node);
            let geometric = Mat4::translation(props.v3("GeometricTranslation", V3::ZERO))
                * Mat4::from_quat(euler(0, props.v3("GeometricRotation", V3::ZERO)))
                * Mat4::scale(props.v3("GeometricScaling", V3::ONE));
            let materials: Vec<u16> = children_of(*key, "Material")
                .into_iter()
                .map(|(m, _)| material_for(m, &mut model, &mut warnings))
                .collect();
            let mut mesh = read_geometry(geometry.node, &geometric, &materials, &mut warnings);
            mesh.name = object.name.clone();
            mesh.node = node_index.get(key).copied();
            if mesh.faces.is_empty() {
                continue;
            }
            // Skin.
            let mut skin = Skin::default();
            let mut weights: Vec<Vec<(u16, f32)>> = vec![Vec::new(); mesh.positions.len()];
            for (deformer, _) in children_of(geometry_key, "Deformer") {
                if objects[&deformer].kind != "Skin" {
                    continue;
                }
                for (cluster, _) in children_of(deformer, "Deformer") {
                    let cluster_node = objects[&cluster].node;
                    let Some(bone_key) = children_of(cluster, "Model").first().map(|(k, _)| *k)
                    else {
                        continue;
                    };
                    let Some(&bone_node) = node_index.get(&bone_key) else {
                        continue;
                    };
                    let indexes = cluster_node.array_i64("Indexes");
                    let values = cluster_node.array_f64("Weights");
                    let matrix = |name: &str| -> Option<Mat4> {
                        let m = cluster_node.array_f64(name);
                        (m.len() == 16).then(|| Mat4 {
                            m: std::array::from_fn(|i| m[i] as f32),
                        })
                    };
                    // `Transform` is stored already as the joint's inverse
                    // bind: the bone's bind matrix undone, times the mesh's.
                    let inverse = matrix("Transform").unwrap_or_else(|| {
                        let link = matrix("TransformLink")
                            .or_else(|| bind_globals.get(&bone_key).copied())
                            .unwrap_or(Mat4::IDENTITY);
                        let mesh_bind = bind_globals.get(key).copied().unwrap_or(Mat4::IDENTITY);
                        link.inverse().unwrap_or(Mat4::IDENTITY) * mesh_bind
                    });
                    let joint = skin.joints.len() as u16;
                    skin.joints.push(bone_node);
                    skin.inverse_bind.push(inverse);
                    model.nodes[usize::from(bone_node)].bone = true;
                    for (v, w) in indexes.iter().zip(&values) {
                        if let Some(list) = weights.get_mut(*v as usize) {
                            list.push((joint, *w as f32));
                        }
                    }
                }
            }
            if !skin.joints.is_empty() {
                if weights.iter().any(Vec::is_empty) {
                    // Unweighted vertices ride with the mesh's own node.
                    if let Some(own) = mesh.node {
                        let joint = skin.joints.len() as u16;
                        skin.joints.push(own);
                        let bind = bind_globals.get(key).copied().unwrap_or(Mat4::IDENTITY);
                        skin.inverse_bind
                            .push(bind.inverse().unwrap_or(Mat4::IDENTITY) * bind);
                        for list in weights.iter_mut().filter(|list| list.is_empty()) {
                            list.push((joint, 1.0));
                        }
                    }
                }
                for (v, list) in weights.into_iter().enumerate() {
                    skin.set_vertex(v, list);
                }
                mesh.skin = Some(skin);
            }
            model.meshes.push(mesh);
        }
    }
    if model.materials.is_empty() {
        model.materials.push(Material::default());
    }

    // Bone lengths: out to the first child bone.
    let rest = model.rest_world();
    for index in 0..model.nodes.len() {
        if !model.nodes[index].bone {
            continue;
        }
        let here = rest[index].translation_part();
        let child = model
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.parent == Some(index as u16) && n.bone)
            .map(|(i, _)| rest[i].translation_part());
        model.nodes[index].length = child.map_or(0.0, |c| (c - here).length());
    }
    let lengths: Vec<f32> = model
        .nodes
        .iter()
        .filter(|n| n.bone && n.length > 0.0)
        .map(|n| n.length)
        .collect();
    let typical = if lengths.is_empty() {
        1.0
    } else {
        lengths.iter().sum::<f32>() / lengths.len() as f32
    };
    for node in model.nodes.iter_mut().filter(|n| n.bone && n.length <= 0.0) {
        node.length = typical * 0.5;
    }

    // Animation stacks.
    let mut stacks: Vec<(&Key, &Object<'_>)> = objects
        .iter()
        .filter(|(_, o)| o.class == "AnimationStack")
        .collect();
    stacks.sort_by_key(|(key, _)| **key);
    for (stack_key, stack) in stacks {
        let mut clip = Clip {
            name: if stack.name.is_empty() {
                "Take".into()
            } else {
                stack.name.clone()
            },
            ..Clip::default()
        };
        let stack_props = Props::of(stack.node);
        let start = stack_props.i("LocalStart", stack_props.i("ReferenceStart", 0)) as f64
            / TICKS_PER_SECOND;
        let stop =
            stack_props.i("LocalStop", stack_props.i("ReferenceStop", 0)) as f64 / TICKS_PER_SECOND;
        let mut curves: HashMap<Key, HashMap<String, AxisCurves>> = HashMap::new();
        for (layer, _) in children_of(*stack_key, "AnimationLayer") {
            for (curve_node, _) in children_of(layer, "AnimationCurveNode") {
                let Some((model_key, property)) = parents.get(&curve_node).and_then(|list| {
                    list.iter().find_map(|(parent, property)| {
                        (objects.get(parent).is_some_and(|o| o.class == "Model"))
                            .then(|| (*parent, property.clone().unwrap_or_default()))
                    })
                }) else {
                    continue;
                };
                let slots = curves
                    .entry(model_key)
                    .or_default()
                    .entry(property)
                    .or_default();
                for (curve, axis) in children_of(curve_node, "AnimationCurve") {
                    let slot = match axis.as_deref() {
                        Some("d|X") | Some("X") => 0,
                        Some("d|Y") | Some("Y") => 1,
                        Some("d|Z") | Some("Z") => 2,
                        _ => continue,
                    };
                    let node = objects[&curve].node;
                    let times: Vec<f64> = node
                        .array_i64("KeyTime")
                        .into_iter()
                        .map(|t| t as f64 / TICKS_PER_SECOND - start.max(0.0))
                        .collect();
                    let values = node.array_f64("KeyValueFloat");
                    if !times.is_empty() && times.len() == values.len() {
                        slots[slot] = Some((times, values));
                    }
                }
            }
        }
        for (model_key, properties) in &curves {
            let (Some(&node), Some(xform)) = (node_index.get(model_key), xforms.get(model_key))
            else {
                continue;
            };
            let props = Props::of(objects[model_key].node);
            let statics = [
                ("Lcl Translation", props.v3("Lcl Translation", V3::ZERO)),
                ("Lcl Rotation", props.v3("Lcl Rotation", V3::ZERO)),
                ("Lcl Scaling", props.v3("Lcl Scaling", V3::ONE)),
            ];
            let mut times: Vec<f64> = properties
                .values()
                .flat_map(|slots| slots.iter().flatten().flat_map(|(t, _)| t.iter().copied()))
                .collect();
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1.0e-6);
            if times.is_empty() {
                continue;
            }
            let sample = |property: &str, fallback: V3, time: f64| -> V3 {
                let Some(slots) = properties.get(property) else {
                    return fallback;
                };
                let mut out = [fallback.x, fallback.y, fallback.z];
                for (axis, slot) in slots.iter().enumerate() {
                    if let Some((t, v)) = slot {
                        out[axis] = sample_curve(t, v, time) as f32;
                    }
                }
                V3::new(out[0], out[1], out[2])
            };
            let mut t_keys = Vec::new();
            let mut r_keys = Vec::new();
            let mut s_keys = Vec::new();
            let mut previous: Option<Quat> = None;
            for time in &times {
                let m = xform.matrix(
                    sample("Lcl Translation", statics[0].1, *time),
                    sample("Lcl Rotation", statics[1].1, *time),
                    sample("Lcl Scaling", statics[2].1, *time),
                );
                let (t, mut r, s) = m.decompose();
                // Keep quaternions on one side so slerp takes the short way.
                if let Some(p) = previous {
                    if p.dot(r) < 0.0 {
                        r = Quat::new(-r.x, -r.y, -r.z, -r.w);
                    }
                }
                previous = Some(r);
                t_keys.extend_from_slice(&[t.x, t.y, t.z]);
                r_keys.extend_from_slice(&[r.x, r.y, r.z, r.w]);
                s_keys.extend_from_slice(&[s.x, s.y, s.z]);
            }
            let seconds: Vec<f32> = times.iter().map(|t| *t as f32).collect();
            for (target, values) in [
                (ChannelTarget::Translation, t_keys),
                (ChannelTarget::Rotation, r_keys),
                (ChannelTarget::Scale, s_keys),
            ] {
                clip.channels.push(Channel {
                    node,
                    target,
                    times: seconds.clone(),
                    values,
                    step: false,
                });
            }
        }
        if clip.channels.is_empty() {
            continue;
        }
        let last = clip
            .channels
            .iter()
            .filter_map(|c| c.times.last().copied())
            .fold(0.0f32, f32::max);
        clip.duration = if stop > start {
            ((stop - start) as f32).max(last)
        } else {
            last
        };
        model.clips.push(clip);
    }

    model.sanitize();
    if model.meshes.is_empty() {
        return Err("the FBX file has no meshes".into());
    }
    Ok(Imported {
        model,
        name: "FBX model".to_string(),
        warnings,
    })
}

fn read_near(base_dir: Option<&Path>, file: &str) -> Option<Vec<u8>> {
    let normalized = file.replace('\\', "/");
    let path = Path::new(&normalized);
    let mut candidates = Vec::new();
    if path.is_absolute() {
        candidates.push(path.to_path_buf());
    }
    if let Some(base) = base_dir {
        candidates.push(base.join(path));
        if let Some(name) = path.file_name() {
            candidates.push(base.join(name));
            candidates.push(base.join("textures").join(name));
            candidates.push(base.join("Textures").join(name));
        }
    }
    candidates
        .into_iter()
        .find_map(|candidate| std::fs::read(candidate).ok())
}

/// Linear interpolation through FBX curve keys, held at the ends.
fn sample_curve(times: &[f64], values: &[f64], time: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    let next = times.partition_point(|t| *t <= time);
    if next == 0 {
        return values[0];
    }
    if next >= times.len() {
        return values[times.len() - 1];
    }
    let (t0, t1) = (times[next - 1], times[next]);
    let t = if t1 > t0 {
        (time - t0) / (t1 - t0)
    } else {
        0.0
    };
    values[next - 1] + (values[next] - values[next - 1]) * t
}

/// What a layer element holds per polygon corner, per vertex or per
/// polygon, resolved to a value index.
struct Layer {
    mapping: String,
    indices: Option<Vec<i64>>,
}

impl Layer {
    fn of(node: &FbxNode, index_name: &str) -> Self {
        let mapping = node
            .first_str("MappingInformationType")
            .unwrap_or("ByPolygonVertex")
            .to_string();
        let reference = node
            .first_str("ReferenceInformationType")
            .unwrap_or("Direct");
        let indices = (reference == "IndexToDirect" || reference == "Index")
            .then(|| node.array_i64(index_name))
            .filter(|i| !i.is_empty());
        Self { mapping, indices }
    }

    fn index(&self, corner: usize, vertex: usize, polygon: usize) -> usize {
        let raw = match self.mapping.as_str() {
            "ByVertice" | "ByVertex" | "ByControlPoint" => vertex,
            "ByPolygon" => polygon,
            "AllSame" => 0,
            _ => corner,
        };
        match &self.indices {
            Some(indices) => indices.get(raw).map_or(0, |i| (*i).max(0) as usize),
            None => raw,
        }
    }
}

/// One animated property's curves on x, y and z: (key times, values).
type AxisCurves = [Option<(Vec<f64>, Vec<f64>)>; 3];

fn read_geometry(
    geometry: &FbxNode,
    geometric: &Mat4,
    materials: &[u16],
    warnings: &mut Vec<String>,
) -> Mesh {
    let vertices = geometry.array_f64("Vertices");
    let indices = geometry.array_i64("PolygonVertexIndex");
    let mut mesh = Mesh {
        positions: vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| geometric.transform_point(V3::new(c[0] as f32, c[1] as f32, c[2] as f32)))
            .collect(),
        ..Mesh::default()
    };
    let normal_layer = geometry.child("LayerElementNormal");
    let normals = normal_layer.map(|n| (Layer::of(n, "NormalsIndex"), n.array_f64("Normals")));
    let uv_layer = geometry.child("LayerElementUV");
    let uvs = uv_layer.map(|n| (Layer::of(n, "UVIndex"), n.array_f64("UV")));
    let material_layer = geometry.child("LayerElementMaterial");
    let material_ids = material_layer.map(|n| {
        (
            n.first_str("MappingInformationType")
                .unwrap_or("AllSame")
                .to_string(),
            n.array_i64("Materials"),
        )
    });
    let color_layer = geometry.child("LayerElementColor");
    let colors = color_layer.map(|n| (Layer::of(n, "ColorIndex"), n.array_f64("Colors")));
    let geometric_normal = geometric
        .inverse()
        .map(|m| m.transpose())
        .unwrap_or(*geometric);
    let vertex_count = mesh.positions.len();
    let mut color_sums = vec![[0.0f32; 5]; if colors.is_some() { vertex_count } else { 0 }];
    let mut polygon = Vec::new();
    let mut polygon_index = 0usize;
    let mut skipped = 0;
    for (corner, raw) in indices.into_iter().enumerate() {
        let (vertex, end) = if raw < 0 {
            ((-raw - 1) as usize, true)
        } else {
            (raw as usize, false)
        };
        polygon.push((vertex, corner));
        if !end {
            continue;
        }
        let corners = std::mem::take(&mut polygon);
        if corners.len() < 3 || corners.iter().any(|(v, _)| *v >= vertex_count) {
            skipped += 1;
            polygon_index += 1;
            continue;
        }
        let verts: Vec<u32> = corners.iter().map(|(v, _)| *v as u32).collect();
        let face_uvs: Option<Vec<[f32; 2]>> = uvs.as_ref().map(|(layer, data)| {
            corners
                .iter()
                .map(|(v, c)| {
                    let i = layer.index(*c, *v, polygon_index);
                    let (u, w) = (
                        data.get(i * 2).copied().unwrap_or(0.0),
                        data.get(i * 2 + 1).copied().unwrap_or(0.0),
                    );
                    [u as f32, 1.0 - w as f32]
                })
                .collect()
        });
        let material = match &material_ids {
            Some((mapping, ids)) if !ids.is_empty() => {
                let slot = if mapping == "AllSame" {
                    ids[0]
                } else {
                    ids.get(polygon_index).copied().unwrap_or(0)
                };
                materials
                    .get(slot.max(0) as usize)
                    .copied()
                    .unwrap_or(materials.first().copied().unwrap_or(0))
            }
            _ => materials.first().copied().unwrap_or(0),
        };
        if let Some((layer, data)) = &normals {
            for (v, c) in &corners {
                let i = layer.index(*c, *v, polygon_index);
                let n = V3::new(
                    data.get(i * 3).copied().unwrap_or(0.0) as f32,
                    data.get(i * 3 + 1).copied().unwrap_or(0.0) as f32,
                    data.get(i * 3 + 2).copied().unwrap_or(1.0) as f32,
                );
                mesh.corner_normals
                    .push(geometric_normal.transform_vector(n).normalized());
            }
        }
        if let Some((layer, data)) = &colors {
            for (v, c) in &corners {
                let i = layer.index(*c, *v, polygon_index);
                let sum = &mut color_sums[*v];
                for (k, slot) in sum.iter_mut().take(4).enumerate() {
                    *slot += data.get(i * 4 + k).copied().unwrap_or(1.0) as f32;
                }
                sum[4] += 1.0;
            }
        }
        // `push_face` would also extend corner normals; add the face by hand.
        let start = mesh.corner_verts.len() as u32;
        mesh.corner_verts.extend_from_slice(&verts);
        if let Some(face_uvs) = face_uvs {
            mesh.corner_uvs.resize(start as usize, [0.0, 0.0]);
            mesh.corner_uvs.extend(face_uvs);
        }
        mesh.faces.push(q0s_format::three::Face {
            start,
            len: verts.len() as u32,
            material,
            smooth: true,
        });
        polygon_index += 1;
    }
    if !mesh.corner_uvs.is_empty() {
        mesh.corner_uvs.resize(mesh.corner_verts.len(), [0.0, 0.0]);
    }
    if mesh.corner_normals.len() != mesh.corner_verts.len() {
        mesh.corner_normals.clear();
    }
    if !color_sums.is_empty() {
        mesh.colors = color_sums
            .iter()
            .map(|s| {
                let n = s[4].max(1.0);
                let c = |x: f32| ((x / n).clamp(0.0, 1.0) * 255.0).round() as u8;
                Rgba {
                    r: c(s[0]),
                    g: c(s[1]),
                    b: c(s[2]),
                    a: if s[4] > 0.0 { c(s[3]) } else { 255 },
                }
            })
            .collect();
    }
    if skipped > 0 {
        warnings.push(format!("skipped {skipped} broken polygon(s)"));
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII: &str = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
}
GlobalSettings:  {
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",1
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",2
		P: "FrontAxisSign", "int", "Integer", "",1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
	}
}
Objects:  {
	Geometry: 10, "Geometry::Quad", "Mesh" {
		Vertices: *12 {
			a: -1,-1,0,1,-1,0,1,1,0,-1,1,0
		}
		PolygonVertexIndex: *4 {
			a: 0,1,2,-4
		}
		LayerElementUV: 0 {
			MappingInformationType: "ByPolygonVertex"
			ReferenceInformationType: "IndexToDirect"
			UV: *8 {
				a: 0,0,1,0,1,1,0,1
			}
			UVIndex: *4 {
				a: 0,1,2,3
			}
		}
	}
	Model: 20, "Model::Quad", "Mesh" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",5,0,0
		}
	}
	Model: 30, "Model::Hips", "LimbNode" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0,2,0
		}
	}
	Model: 31, "Model::Spine", "LimbNode" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0,3,0
		}
	}
	Material: 40, "Material::Red", "" {
		Properties70:  {
			P: "DiffuseColor", "Color", "", "A",1,0,0
		}
	}
	AnimationStack: 50, "AnimStack::Wave", "" {
	}
	AnimationLayer: 51, "AnimLayer::Base", "" {
	}
	AnimationCurveNode: 52, "AnimCurveNode::R", "" {
	}
	AnimationCurve: 53, "AnimCurve::", "" {
		KeyTime: *2 {
			a: 0,46186158000
		}
		KeyValueFloat: *2 {
			a: 0,90
		}
	}
}
Connections:  {
	C: "OO",20,0
	C: "OO",10,20
	C: "OO",40,20
	C: "OO",30,0
	C: "OO",31,30
	C: "OO",51,50
	C: "OO",52,51
	C: "OP",52,31, "Lcl Rotation"
	C: "OP",53,52, "d|Z"
}
"#;

    #[test]
    fn an_ascii_fbx_comes_in_whole() {
        let imported = import(ASCII.as_bytes(), None).unwrap();
        let model = &imported.model;
        assert_eq!(model.meshes.len(), 1);
        assert_eq!(model.meshes[0].faces.len(), 1);
        assert_eq!(model.meshes[0].corner_uvs.len(), 4);
        assert_eq!(model.materials[0].color.r, 255);
        assert!(model.nodes.iter().any(|n| n.name == "Spine" && n.bone));
        assert_eq!(model.clips.len(), 1);
        assert!((model.clips[0].duration - 1.0).abs() < 1.0e-3);
        let spine = model.find_node("Spine").unwrap();
        let locals = model.sample_clip(Some(0), 1.0);
        let turned = locals[usize::from(spine)].rotation.rotate(V3::X);
        assert!((turned - V3::Y).length() < 1.0e-3, "{turned:?}");
        let world = model.rest_world();
        let quad = model.meshes[0].node.unwrap();
        assert!((world[usize::from(quad)].translation_part().x - 5.0).abs() < 1.0e-4);
    }

    /// Writes a minimal binary FBX 7.4 of `nodes`.
    fn write_binary(nodes: &[FbxNode]) -> Vec<u8> {
        fn prop(out: &mut Vec<u8>, p: &Prop) {
            match p {
                Prop::Int(v) => {
                    out.push(b'L');
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Prop::Float(v) => {
                    out.push(b'D');
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Prop::Bool(v) => {
                    out.push(b'C');
                    out.push(u8::from(*v));
                }
                Prop::Str(s) => {
                    out.push(b'S');
                    let bytes = match s.split_once("::") {
                        Some((class, name)) => {
                            let mut b = name.as_bytes().to_vec();
                            b.extend_from_slice(&[0, 1]);
                            b.extend_from_slice(class.as_bytes());
                            b
                        }
                        None => s.as_bytes().to_vec(),
                    };
                    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    out.extend_from_slice(&bytes);
                }
                Prop::Raw(r) => {
                    out.push(b'R');
                    out.extend_from_slice(&(r.len() as u32).to_le_bytes());
                    out.extend_from_slice(r);
                }
                Prop::Ints(v) if v.iter().any(|x| i32::try_from(*x).is_err()) => {
                    out.push(b'l');
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    out.extend_from_slice(&0u32.to_le_bytes());
                    out.extend_from_slice(&((v.len() * 8) as u32).to_le_bytes());
                    for x in v {
                        out.extend_from_slice(&x.to_le_bytes());
                    }
                }
                Prop::Ints(v) => {
                    out.push(b'i');
                    let raw: Vec<u8> = v.iter().flat_map(|x| (*x as i32).to_le_bytes()).collect();
                    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
                    out.extend_from_slice(&packed);
                }
                Prop::Floats(v) => {
                    out.push(b'd');
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    out.extend_from_slice(&0u32.to_le_bytes());
                    out.extend_from_slice(&((v.len() * 8) as u32).to_le_bytes());
                    for x in v {
                        out.extend_from_slice(&x.to_le_bytes());
                    }
                }
            }
        }
        fn node(out: &mut Vec<u8>, n: &FbxNode) {
            let start = out.len();
            out.extend_from_slice(&[0; 12]);
            out.push(n.name.len() as u8);
            out.extend_from_slice(n.name.as_bytes());
            let props_start = out.len();
            for p in &n.props {
                prop(out, p);
            }
            let props_len = out.len() - props_start;
            for c in &n.children {
                node(out, c);
            }
            if !n.children.is_empty() {
                out.extend_from_slice(&[0; 13]);
            }
            let end = out.len() as u32;
            out[start..start + 4].copy_from_slice(&end.to_le_bytes());
            out[start + 4..start + 8].copy_from_slice(&(n.props.len() as u32).to_le_bytes());
            out[start + 8..start + 12].copy_from_slice(&(props_len as u32).to_le_bytes());
        }
        let mut out = BINARY_MAGIC.to_vec();
        out.extend_from_slice(&[0x1a, 0]);
        out.extend_from_slice(&7400u32.to_le_bytes());
        for n in nodes {
            node(&mut out, n);
        }
        out.extend_from_slice(&[0; 13]);
        out
    }

    #[test]
    fn a_binary_fbx_reads_like_its_ascii_twin() {
        let tree = parse(ASCII.as_bytes()).unwrap();
        // ASCII arrays sit in `a:` records; binary ones are the values.
        fn flatten(n: &FbxNode) -> FbxNode {
            let mut out = n.clone();
            if let Some(a) = n.child("a") {
                out.props = a.props.clone();
                out.children.clear();
            } else {
                out.children = n.children.iter().map(flatten).collect();
            }
            out
        }
        let tree: Vec<FbxNode> = tree.iter().map(flatten).collect();
        let bytes = write_binary(&tree);
        let back = parse(&bytes).unwrap();
        assert_eq!(back.len(), tree.len());
        let imported = import(&bytes, None).unwrap();
        assert_eq!(imported.model.meshes[0].faces.len(), 1);
        assert_eq!(imported.model.clips.len(), 1);
    }
}
