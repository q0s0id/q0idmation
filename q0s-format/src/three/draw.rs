//! A frame of 3D, ready for a renderer: the camera, the lights and every
//! mesh and flat drawing to draw, all in stage space. The software
//! renderer and the GPU renderer both draw from this, and both light with
//! [`shade`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use super::math::{Mat4, V3};
use super::model::{
    mesh_matrix, skinned_render_vertices, Material, ModelData, RenderMesh, Shading,
};
use super::scene::{model_to_stage, CameraPose, LightRig, PoseTrack, Space3d};
use crate::transform::Affine;

/// How stage space is seen: the camera, and where its picture lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View3d {
    pub camera: CameraPose,
    pub view: Mat4,
    pub orthographic: bool,
    /// The stage's size; the camera's picture is centred on its middle.
    pub stage: [f32; 2],
    /// Stage coordinates to output pixels.
    pub to_output: Affine,
    /// Output size in pixels.
    pub output: [u32; 2],
}

impl View3d {
    pub fn new(
        camera: CameraPose,
        orthographic: bool,
        stage: [f32; 2],
        to_output: Affine,
        output: [u32; 2],
    ) -> Self {
        Self {
            view: camera.view_matrix(),
            camera,
            orthographic,
            stage,
            to_output,
            output,
        }
    }

    fn ortho_scale(&self) -> f32 {
        let distance = (self.camera.target - self.camera.position)
            .length()
            .max(1.0e-3);
        self.camera.zoom / distance
    }

    /// Stage pixels per view-space unit at view depth `z`.
    pub fn stage_scale_at(&self, z: f32) -> f32 {
        if self.orthographic {
            self.ortho_scale()
        } else {
            self.camera.zoom / z.max(1.0e-3)
        }
    }

    /// Camera-space point to stage coordinates.
    pub fn view_to_stage(&self, v: V3) -> [f32; 2] {
        let (cx, cy) = (self.stage[0] * 0.5, self.stage[1] * 0.5);
        let s = self.stage_scale_at(v.z);
        [cx + v.x * s, cy + v.y * s]
    }

    /// Camera-space point to output pixels.
    pub fn view_to_output(&self, v: V3) -> [f32; 2] {
        let [x, y] = self.view_to_stage(v);
        let a = &self.to_output;
        [a.a11 * x + a.a12 * y + a.tx, a.a21 * x + a.a22 * y + a.ty]
    }

    /// Stage-space point to output pixels and depth, if it is in front of
    /// the camera.
    pub fn project(&self, world: V3) -> Option<[f32; 3]> {
        let v = self.view.transform_point(world);
        if !self.orthographic && v.z <= 1.0e-3 {
            return None;
        }
        let [x, y] = self.view_to_output(v);
        Some([x, y, v.z])
    }

    /// Camera space to GL clip space for this output, with depth from
    /// `near` to `far`.
    pub fn clip_matrix(&self, near: f32, far: f32) -> Mat4 {
        let (w, h) = (self.output[0].max(1) as f32, self.output[1].max(1) as f32);
        let (cx, cy) = (self.stage[0] * 0.5, self.stage[1] * 0.5);
        let a = &self.to_output;
        let (ox, oy) = (
            a.a11 * cx + a.a12 * cy + a.tx,
            a.a21 * cx + a.a22 * cy + a.ty,
        );
        let far = far.max(near + 1.0e-3);
        if self.orthographic {
            let k = self.ortho_scale();
            Mat4::from_rows([
                [
                    2.0 / w * a.a11 * k,
                    2.0 / w * a.a12 * k,
                    0.0,
                    2.0 / w * ox - 1.0,
                ],
                [
                    -2.0 / h * a.a21 * k,
                    -2.0 / h * a.a22 * k,
                    0.0,
                    1.0 - 2.0 / h * oy,
                ],
                [0.0, 0.0, 2.0 / (far - near), -(far + near) / (far - near)],
                [0.0, 0.0, 0.0, 1.0],
            ])
        } else {
            let zoom = self.camera.zoom;
            Mat4::from_rows([
                [
                    2.0 / w * a.a11 * zoom,
                    2.0 / w * a.a12 * zoom,
                    2.0 / w * ox - 1.0,
                    0.0,
                ],
                [
                    -2.0 / h * a.a21 * zoom,
                    -2.0 / h * a.a22 * zoom,
                    1.0 - 2.0 / h * oy,
                    0.0,
                ],
                [
                    0.0,
                    0.0,
                    (far + near) / (far - near),
                    -2.0 * far * near / (far - near),
                ],
                [0.0, 0.0, 1.0, 0.0],
            ])
        }
    }

    /// The ray through output pixel `(x, y)`, as an origin and unit
    /// direction in stage space.
    pub fn ray(&self, x: f32, y: f32) -> Option<(V3, V3)> {
        let back = self.to_output.inverse()?;
        let stage = back.apply(crate::v2::Vec2::new(x, y));
        let (cx, cy) = (self.stage[0] * 0.5, self.stage[1] * 0.5);
        let inverse = self.view.inverse()?;
        if self.orthographic {
            let k = self.ortho_scale();
            let origin = V3::new((stage.x - cx) / k, (stage.y - cy) / k, 0.0);
            Some((
                inverse.transform_point(origin),
                inverse.transform_vector(V3::Z).normalized(),
            ))
        } else {
            let direction = V3::new(
                (stage.x - cx) / self.camera.zoom,
                (stage.y - cy) / self.camera.zoom,
                1.0,
            );
            Some((
                self.camera.position,
                inverse.transform_vector(direction).normalized(),
            ))
        }
    }

    /// Unit vector from `point` toward the eye.
    pub fn toward_eye(&self, point: V3) -> V3 {
        if self.orthographic {
            -(self.camera.target - self.camera.position).normalized()
        } else {
            (self.camera.position - point).normalized()
        }
    }
}

/// A premultiplied RGBA picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// Posed vertices of a skinned mesh, in model space.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Posed {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
}

#[derive(Debug, Clone)]
pub struct MeshDraw {
    pub model: Arc<ModelData>,
    pub mesh: usize,
    pub render: Arc<RenderMesh>,
    /// Skinned vertices, moved by `matrix` like the rest shape would be.
    pub posed: Option<Arc<Posed>>,
    /// The mesh's coordinates to stage space.
    pub matrix: Mat4,
    pub opacity: f32,
    pub lit: bool,
    /// Draw the cartoon outline too.
    pub outline: bool,
}

#[derive(Debug, Clone)]
pub struct PlaneDraw {
    pub image: Arc<Image>,
    /// The part of the object's own plane the picture covers: x0, y0, x1,
    /// y1.
    pub rect: [f32; 4],
    pub matrix: Mat4,
    pub opacity: f32,
    pub lit: bool,
    /// A flat drawing lit where it lies: drawn in layer order over what
    /// came before, never hiding what comes after by depth.
    pub layered: bool,
}

#[derive(Debug, Clone)]
pub enum DrawItem {
    Mesh(MeshDraw),
    Plane(PlaneDraw),
}

impl DrawItem {
    pub fn opacity(&self) -> f32 {
        match self {
            Self::Mesh(mesh) => mesh.opacity,
            Self::Plane(plane) => plane.opacity,
        }
    }

    pub fn layered(&self) -> bool {
        matches!(self, Self::Plane(plane) if plane.layered)
    }

    /// Does any of it let what is behind show through?
    pub fn blends(&self) -> bool {
        self.opacity() < 0.999
            || match self {
                Self::Plane(_) => true,
                Self::Mesh(mesh) => mesh.render.parts.iter().any(|(m, _, _)| {
                    mesh.model
                        .materials
                        .get(usize::from(*m))
                        .is_some_and(|material| material.blends(&mesh.model.textures))
                }),
            }
    }

    /// Its middle in stage space, for sorting.
    pub fn centre(&self) -> V3 {
        match self {
            Self::Mesh(mesh) => {
                let corners = mesh_bounds(mesh);
                mesh.matrix.transform_point((corners[0] + corners[7]) * 0.5)
            }
            Self::Plane(plane) => {
                let [x0, y0, x1, y1] = plane.rect;
                plane
                    .matrix
                    .transform_point(V3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0))
            }
        }
    }
}

/// When each item draws: runs of depth-sorted 3D items, opaque parts first
/// and see-through ones back to front, and layered drawings in their place.
/// Returns a pass number per item for its opaque and its blended parts.
pub fn draw_passes(items: &[DrawItem], view: &Mat4) -> Vec<(u32, u32)> {
    let mut passes = vec![(0, 0); items.len()];
    let mut next = 0u32;
    let mut index = 0;
    while index < items.len() {
        if items[index].layered() {
            passes[index] = (next, next);
            next += 1;
            index += 1;
            continue;
        }
        let start = index;
        while index < items.len() && !items[index].layered() {
            index += 1;
        }
        let opaque = next;
        next += 1;
        let mut blended: Vec<(f32, usize)> = (start..index)
            .filter(|i| items[*i].blends())
            .map(|i| (view.transform_point(items[i].centre()).z, i))
            .collect();
        blended.sort_by(|a, b| b.0.total_cmp(&a.0));
        for pass in passes.iter_mut().take(index).skip(start) {
            *pass = (opaque, opaque);
        }
        for (_, i) in blended {
            passes[i] = (opaque, next);
            next += 1;
        }
    }
    passes
}

/// Lines, points and see-through triangles drawn over a frame, for the
/// model editor: wireframes, vertices, bones, picked faces. Colours are
/// straight RGBA.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub lines: Vec<([V3; 2], [u8; 4])>,
    pub points: Vec<(V3, [u8; 4])>,
    pub triangles: Vec<([V3; 3], [u8; 4])>,
    /// Point size in pixels.
    pub point_size: f32,
    /// Drawn over everything instead of hidden behind surfaces.
    pub on_top: bool,
    /// Lines and points that always show, like bones and the grid.
    pub top_lines: Vec<([V3; 2], [u8; 4])>,
    pub top_points: Vec<(V3, [u8; 4])>,
}

/// Everything one 3D picture needs.
#[derive(Debug, Clone)]
pub struct Frame3d {
    pub view: View3d,
    pub rig: LightRig,
    pub items: Vec<DrawItem>,
    /// Editor overlays; the software renderer leaves them out.
    pub overlay: Option<std::sync::Arc<Overlay>>,
    /// Fill the background with this straight RGBA colour first.
    pub clear: Option<[u8; 4]>,
}

impl Frame3d {
    /// The nearest and furthest view depths anything reaches, padded.
    pub fn depth_range(&self) -> (f32, f32) {
        let mut near = f32::INFINITY;
        let mut far = f32::NEG_INFINITY;
        let mut take = |world: V3| {
            let z = self.view.view.transform_point(world).z;
            near = near.min(z);
            far = far.max(z);
        };
        for item in &self.items {
            match item {
                DrawItem::Mesh(mesh) => {
                    let corners = mesh_bounds(mesh);
                    for corner in corners {
                        take(mesh.matrix.transform_point(corner));
                    }
                }
                DrawItem::Plane(plane) => {
                    let [x0, y0, x1, y1] = plane.rect;
                    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                        take(plane.matrix.transform_point(V3::new(x, y, 0.0)));
                    }
                }
            }
        }
        // Overlays count too, or a floor reaching past the model would be
        // cut off by the far plane.
        if let Some(overlay) = &self.overlay {
            for (line, _) in overlay.lines.iter().chain(&overlay.top_lines) {
                line.iter().for_each(|p| take(*p));
            }
            for (point, _) in overlay.points.iter().chain(&overlay.top_points) {
                take(*point);
            }
            for (triangle, _) in &overlay.triangles {
                triangle.iter().for_each(|p| take(*p));
            }
        }
        if !near.is_finite() || (!self.view.orthographic && far <= 0.0) {
            return (1.0, 10_000.0);
        }
        // Padding in proportion, so tiny models (a few hundredths of a unit)
        // are not clipped and big scenes keep their depth precision.
        let span = (far - near).max(far.abs() * 1.0e-3).max(1.0e-6);
        let pad = span * 0.05 + far.abs() * 1.0e-3;
        if self.view.orthographic {
            (near - pad, far + pad)
        } else {
            ((near - pad).max(far * 1.0e-4), far + pad)
        }
    }
}

fn mesh_bounds(mesh: &MeshDraw) -> [V3; 8] {
    let positions = mesh
        .posed
        .as_ref()
        .map(|posed| posed.positions.as_slice())
        .unwrap_or(&mesh.render.positions);
    let mut lo = V3::splat(f32::INFINITY);
    let mut hi = V3::splat(f32::NEG_INFINITY);
    for p in positions {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    if !lo.is_finite() {
        lo = V3::ZERO;
        hi = V3::ZERO;
    }
    [
        V3::new(lo.x, lo.y, lo.z),
        V3::new(hi.x, lo.y, lo.z),
        V3::new(lo.x, hi.y, lo.z),
        V3::new(hi.x, hi.y, lo.z),
        V3::new(lo.x, lo.y, hi.z),
        V3::new(hi.x, lo.y, hi.z),
        V3::new(lo.x, hi.y, hi.z),
        V3::new(hi.x, hi.y, hi.z),
    ]
}

type MeshKey = (usize, usize);
/// Built meshes by model and mesh index, kept while their model lives.
type MeshCache = Mutex<HashMap<MeshKey, (Weak<ModelData>, Arc<RenderMesh>)>>;

fn mesh_cache() -> &'static MeshCache {
    static CACHE: std::sync::OnceLock<MeshCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The drawable form of one mesh of `model`, built once per model data.
/// Model data is never edited in place: an edited model is a new `Arc`.
pub fn render_mesh(model: &Arc<ModelData>, mesh: usize) -> Arc<RenderMesh> {
    let key = (Arc::as_ptr(model) as usize, mesh);
    let Ok(mut cache) = mesh_cache().lock() else {
        return Arc::new(RenderMesh::build(&model.meshes[mesh]));
    };
    if let Some((weak, built)) = cache.get(&key) {
        if weak
            .upgrade()
            .is_some_and(|alive| Arc::ptr_eq(&alive, model))
        {
            return built.clone();
        }
    }
    if cache.len() > 512 {
        cache.retain(|_, (weak, _)| weak.strong_count() > 0);
    }
    let built = Arc::new(RenderMesh::build(&model.meshes[mesh]));
    cache.insert(key, (Arc::downgrade(model), built.clone()));
    built
}

/// Where a model's own (y-up) coordinates land in stage space, given the
/// placement's matrix.
pub fn model_root(placement: &Mat4, model: &ModelData) -> Mat4 {
    *placement * model_to_stage(model.scale)
}

/// The pose of a placed model: its clip at the right time, with any bone
/// keys from the timeline on top. Returns model-space node matrices.
pub fn pose_model(
    model: &ModelData,
    space: &Space3d,
    elapsed_frames: f32,
    fps: u16,
    pose: Option<(&PoseTrack, u16)>,
) -> Vec<Mat4> {
    let clip = (space.clip != super::scene::NO_CLIP)
        .then_some(usize::from(space.clip))
        .filter(|clip| *clip < model.clips.len());
    let time = clip
        .map(|clip| space.clip_seconds(elapsed_frames, fps, model.clips[clip].duration))
        .unwrap_or(0.0);
    let mut locals = model.sample_clip(clip, time);
    if let Some((track, frame)) = pose {
        track.apply(frame, &mut locals);
    }
    model.world_matrices(&locals)
}

/// Adds a placed model's meshes to `out`.
pub fn push_model(
    out: &mut Vec<DrawItem>,
    model: &Arc<ModelData>,
    placement: &Mat4,
    world: &[Mat4],
    opacity: f32,
    lit: bool,
) {
    let root = model_root(placement, model);
    for (index, mesh) in model.meshes.iter().enumerate() {
        if mesh.hidden || mesh.faces.is_empty() {
            continue;
        }
        let render = render_mesh(model, index);
        let posed = skinned_render_vertices(mesh, &render, world)
            .map(|(positions, normals)| Arc::new(Posed { positions, normals }));
        let matrix = if posed.is_some() {
            root
        } else {
            root * mesh_matrix(mesh, world)
        };
        out.push(DrawItem::Mesh(MeshDraw {
            model: model.clone(),
            mesh: index,
            render,
            posed,
            matrix,
            opacity,
            lit,
            outline: model.outline.is_some(),
        }));
    }
}

/// What a surface is made of, as the shading needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub roughness: f32,
    pub metallic: f32,
    pub shading: Shading,
    pub bands: u8,
    pub emissive: [f32; 3],
}

impl Surface {
    pub fn of(material: &Material) -> Self {
        Self {
            roughness: material.roughness,
            metallic: material.metallic,
            shading: material.shading,
            bands: material.toon_bands.max(2),
            emissive: super::scene::unit_rgb(material.emissive),
        }
    }

    /// A flat drawing: no shine.
    pub const MATTE: Self = Self {
        roughness: 1.0,
        metallic: 0.0,
        shading: Shading::Lit,
        bands: 3,
        emissive: [0.0; 3],
    };
}

/// Quantizes light into `bands` flat steps.
pub fn toon_step(light: f32, bands: u8) -> f32 {
    let bands = f32::from(bands.max(2));
    let l = light.clamp(0.0, 1.0);
    ((l * bands).floor() / (bands - 1.0)).min(1.0)
}

/// The most lights that cast shadows in one frame.
pub const MAX_SHADOWS: usize = 2;
/// Shadow maps are this many texels across.
pub const SHADOW_SIZE: usize = 2048;

/// A light's view for its shadow map: stage space into the light's clip
/// space (x, y and depth from -1 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowView {
    /// Index into the rig's lights.
    pub light: usize,
    pub matrix: Mat4,
    /// World units a shadow-map texel covers near the scene, for biasing.
    pub texel: f32,
}

/// Where the frame's shadow casters and receivers are: min and max corners.
fn frame_bounds(items: &[DrawItem]) -> Option<(V3, V3)> {
    let mut out: Option<(V3, V3)> = None;
    let mut take = |p: V3| {
        out = Some(match out {
            Some((lo, hi)) => (lo.min(p), hi.max(p)),
            None => (p, p),
        });
    };
    for item in items {
        match item {
            DrawItem::Mesh(mesh) => {
                for corner in mesh_bounds(mesh) {
                    take(mesh.matrix.transform_point(corner));
                }
            }
            DrawItem::Plane(plane) => {
                let [x0, y0, x1, y1] = plane.rect;
                for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                    take(plane.matrix.transform_point(V3::new(x, y, 0.0)));
                }
            }
        }
    }
    out
}

/// The shadow views of a frame's shadow-casting suns and spots.
pub fn shadow_views(frame: &Frame3d) -> Vec<ShadowView> {
    if !frame.rig.shadows {
        return Vec::new();
    }
    let Some((lo, hi)) = frame_bounds(&frame.items) else {
        return Vec::new();
    };
    let centre = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1.0);
    let mut out = Vec::new();
    for (index, light) in frame.rig.lights.iter().enumerate() {
        if !light.shadows || out.len() >= MAX_SHADOWS {
            continue;
        }
        let forward = light.direction.normalized();
        if forward == V3::ZERO {
            continue;
        }
        let right = forward.any_perpendicular();
        let up = forward.cross(right).normalized();
        match light.kind {
            super::scene::Light3dKind::Sun => {
                let eye = centre - forward * (radius * 2.0);
                let depth = radius * 4.0;
                let view = Mat4::from_rows([
                    [
                        right.x / radius,
                        right.y / radius,
                        right.z / radius,
                        -right.dot(eye) / radius,
                    ],
                    [
                        up.x / radius,
                        up.y / radius,
                        up.z / radius,
                        -up.dot(eye) / radius,
                    ],
                    [
                        2.0 * forward.x / depth,
                        2.0 * forward.y / depth,
                        2.0 * forward.z / depth,
                        -2.0 * forward.dot(eye) / depth - 1.0,
                    ],
                    [0.0, 0.0, 0.0, 1.0],
                ]);
                out.push(ShadowView {
                    light: index,
                    matrix: view,
                    texel: 2.0 * radius / SHADOW_SIZE as f32,
                });
            }
            super::scene::Light3dKind::Spot => {
                let eye = light.position;
                let reach = (centre - eye).length() + radius;
                let far = if light.range > 0.0 {
                    light.range.min(reach)
                } else {
                    reach
                }
                .max(2.0);
                let near = (far * 0.002).max(0.5);
                let half = light.cos_outer.clamp(-0.99, 0.999).acos() + 0.05;
                let t = half.min(1.5).tan();
                let a = (far + near) / (far - near);
                let b = -2.0 * far * near / (far - near);
                let view = Mat4::from_rows([
                    [right.x, right.y, right.z, -right.dot(eye)],
                    [up.x, up.y, up.z, -up.dot(eye)],
                    [forward.x, forward.y, forward.z, -forward.dot(eye)],
                    [0.0, 0.0, 0.0, 1.0],
                ]);
                let projection = Mat4::from_rows([
                    [1.0 / t, 0.0, 0.0, 0.0],
                    [0.0, 1.0 / t, 0.0, 0.0],
                    [0.0, 0.0, a, b],
                    [0.0, 0.0, 1.0, 0.0],
                ]);
                out.push(ShadowView {
                    light: index,
                    matrix: projection * view,
                    texel: 2.0 * t * (centre - eye).length() / SHADOW_SIZE as f32,
                });
            }
            super::scene::Light3dKind::Point => {}
        }
    }
    out
}

/// Light arriving at a point, as the multiplier for the colour under it
/// and the shine added on top. `n` faces the eye already.
pub fn shade(
    rig: &LightRig,
    surface: &Surface,
    point: V3,
    n: V3,
    to_eye: V3,
) -> ([f32; 3], [f32; 3]) {
    shade_with(rig, surface, point, n, to_eye, |_| 1.0)
}

/// [`shade`], with `visible(light)` saying how much of each light is not
/// in shadow at the point.
pub fn shade_with(
    rig: &LightRig,
    surface: &Surface,
    point: V3,
    n: V3,
    to_eye: V3,
    visible: impl Fn(usize) -> f32,
) -> ([f32; 3], [f32; 3]) {
    if surface.shading == Shading::Unlit {
        return ([1.0; 3], [0.0; 3]);
    }
    let mut diffuse = rig.ambient;
    let mut shine = [0.0f32; 3];
    let rough = surface.roughness.clamp(0.02, 1.0);
    let exponent = 2.0f32.powf(1.0 + 10.0 * (1.0 - rough));
    let strength = (0.04 + 0.96 * surface.metallic) * (1.0 - rough) * (1.0 - rough) * 2.0;
    let toon = surface.shading == Shading::Toon;
    for (index, light) in rig.lights.iter().enumerate() {
        let (l, reach) = light.toward(point);
        if reach <= 0.0 {
            continue;
        }
        let reach = if light.shadows && n.dot(l) > 0.0 {
            reach * visible(index)
        } else {
            reach
        };
        if reach <= 0.0 {
            continue;
        }
        let ndl = n.dot(l);
        let d = if toon {
            toon_step(ndl, surface.bands)
        } else {
            ndl.max(0.0)
        };
        let mut s = 0.0;
        if strength > 0.0 && ndl > 0.0 {
            let h = (l + to_eye).normalized();
            let nh = n.dot(h).max(0.0);
            s = nh.powf(exponent) * strength;
            if toon {
                s = if s > 0.5 { 0.6 } else { 0.0 };
            }
        }
        for c in 0..3 {
            diffuse[c] += light.color[c] * reach * d;
            shine[c] += light.color[c] * reach * s;
        }
    }
    let keep = 1.0 - surface.metallic * 0.9;
    for (channel, emissive) in diffuse.iter_mut().zip(surface.emissive) {
        *channel = *channel * keep + emissive;
    }
    (diffuse, shine)
}

/// The soft studio a glass surface mirrors: bright above, dim below. `r`
/// is the reflected direction in stage space, where y points down.
pub fn glass_surroundings(r: V3) -> [f32; 3] {
    let up = (-r.y * 0.5 + 0.5).clamp(0.0, 1.0);
    let up = up * up;
    let low = [0.10, 0.11, 0.13];
    let high = [0.92, 0.95, 1.0];
    [0, 1, 2].map(|c| low[c] + (high[c] - low[c]) * up)
}

/// A glass surface over premultiplied `rgba`: a little of the glass's own
/// colour, its surroundings mirrored more toward its edges, and sharp
/// highlights from the lights. Returns premultiplied RGBA.
pub fn shade_glass(rig: &LightRig, rgba: [f32; 4], point: V3, n: V3, to_eye: V3) -> [f32; 4] {
    let body = rgba[3].clamp(0.0, 1.0);
    let tint = if body > 1.0e-4 {
        [rgba[0] / body, rgba[1] / body, rgba[2] / body]
    } else {
        [1.0; 3]
    };
    let cos = n.dot(to_eye).clamp(0.0, 1.0);
    let fresnel = 0.04 + 0.96 * (1.0 - cos).powi(5);
    let reflected = n * (2.0 * n.dot(to_eye)) - to_eye;
    let around = glass_surroundings(reflected);
    let mut light = rig.ambient;
    let mut shine = [0.0f32; 3];
    for light_source in &rig.lights {
        let (l, reach) = light_source.toward(point);
        if reach <= 0.0 {
            continue;
        }
        let ndl = n.dot(l);
        let h = (l + to_eye).normalized();
        let s = n.dot(h).max(0.0).powf(180.0) * 1.6;
        for c in 0..3 {
            light[c] += light_source.color[c] * reach * ndl.max(0.0) * 0.25;
            shine[c] += light_source.color[c] * reach * s;
        }
    }
    let glint = shine.iter().cloned().fold(0.0f32, f32::max);
    let alpha = (body + fresnel * (1.0 - body) + glint).clamp(0.0, 1.0);
    let mut out = [0.0f32; 4];
    for c in 0..3 {
        out[c] = (tint[c] * body * light[c] + around[c] * fresnel + shine[c]).min(alpha);
    }
    out[3] = alpha;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three::scene::{Light3dKind, LitLight3d};

    #[test]
    fn glass_is_clearer_face_on_than_edge_on() {
        let rig = LightRig {
            ambient: [0.3; 3],
            lights: Vec::new(),
            authored: true,
            shadows: false,
            fog: None,
        };
        let glass = [0.2, 0.22, 0.25, 0.25];
        let face_on = shade_glass(&rig, glass, V3::ZERO, -V3::Z, -V3::Z);
        let edge_on = shade_glass(
            &rig,
            glass,
            V3::ZERO,
            V3::X,
            (-V3::Z * 8.0 + V3::X).normalized(),
        );
        assert!(face_on[3] < 0.35, "{face_on:?}");
        assert!(edge_on[3] > face_on[3] + 0.2, "{edge_on:?}");
        assert!(face_on.iter().take(3).all(|c| *c <= face_on[3] + 1.0e-6));
    }

    #[test]
    fn a_light_in_front_brightens_a_surface_facing_it() {
        let rig = LightRig {
            ambient: [0.1; 3],
            lights: vec![LitLight3d {
                kind: Light3dKind::Point,
                position: V3::new(0.0, 0.0, -100.0),
                direction: V3::Z,
                color: [1.0; 3],
                range: 0.0,
                cos_inner: 1.0,
                cos_outer: 1.0,
                shadows: false,
            }],
            authored: true,
            shadows: false,
            fog: None,
        };
        let (facing, _) = shade(&rig, &Surface::MATTE, V3::ZERO, -V3::Z, -V3::Z);
        let (away, _) = shade(&rig, &Surface::MATTE, V3::ZERO, V3::X, -V3::Z);
        assert!(facing[0] > 1.0 && (away[0] - 0.1).abs() < 1.0e-4);
    }

    #[test]
    fn the_clip_matrix_lands_where_projection_does() {
        let camera = CameraPose::stage(640, 480);
        let view = View3d::new(
            camera,
            false,
            [640.0, 480.0],
            Affine::from_uniform_scale(0.5),
            [320, 240],
        );
        let world = V3::new(100.0, 80.0, 250.0);
        let [x, y, _] = view.project(world).unwrap();
        let clip = view.clip_matrix(1.0, 5000.0) * view.view;
        let c = clip.transform4(world);
        let px = (c[0] / c[3] + 1.0) * 0.5 * 320.0;
        let py = (1.0 - c[1] / c[3]) * 0.5 * 240.0;
        assert!(
            (px - x).abs() < 0.01 && (py - y).abs() < 0.01,
            "{px} {py} vs {x} {y}"
        );
        let (origin, direction) = view.ray(x, y).unwrap();
        let along = world - origin;
        assert!(along.cross(direction).length() / along.length() < 1.0e-4);
    }
}
