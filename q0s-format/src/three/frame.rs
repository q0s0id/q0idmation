//! Gathering a timeline's 3D for one frame, the same way in every
//! renderer: which placements are 3D, the camera and lights, the posed
//! models, and drawings turned into textured planes.
//!
//! A placement is drawn in 3D when it shows a model, when it has a place in
//! 3D (`fx.three`), or when its layer's flat art is lit by the 3D lights.
//! Runs of such placements are drawn together, so they hide each other by
//! depth; anything flat in between is drawn over what came before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::draw::{model_root, pose_model, push_model, DrawItem, Image, PlaneDraw, View3d};
use super::math::{Mat4, V3};
use super::model::ModelData;
use super::scene::{CameraPose, LightRig, Scene3dSetup, Space3d};
use crate::transform::Affine;
use crate::v2::{Asset, PlacementFx, ProjectV2, Vec2};

/// The camera and lights of one timeline frame.
#[derive(Debug, Clone)]
pub struct SceneLook {
    pub camera: CameraPose,
    pub orthographic: bool,
    pub rig: LightRig,
    /// Flat art is lit as planes, up to this layer (`None`: all).
    pub light_flat: Option<Option<u16>>,
}

impl SceneLook {
    pub fn view(&self, project: &ProjectV2, to_output: Affine, output: [u32; 2]) -> View3d {
        View3d::new(
            self.camera,
            self.orthographic,
            [
                f32::from(project.meta.stage_width),
                f32::from(project.meta.stage_height),
            ],
            to_output,
            output,
        )
    }

    /// Walks a timeline's layers in drawing order, saying which have their
    /// flat art lit.
    pub fn flat_cut(&self) -> FlatCut {
        FlatCut {
            on: self.light_flat.is_some(),
            cut: self.light_flat.flatten(),
            passed: false,
        }
    }
}

/// Which layers' flat art the 3D lights reach, layer by layer.
#[derive(Debug, Clone, Copy)]
pub struct FlatCut {
    on: bool,
    cut: Option<u16>,
    passed: bool,
}

impl FlatCut {
    pub const OFF: Self = Self {
        on: false,
        cut: None,
        passed: true,
    };

    /// Is `layer_id`, the next layer drawn, lit?
    pub fn layer(&mut self, layer_id: u16) -> bool {
        if !self.on || self.passed {
            return false;
        }
        if self.cut == Some(layer_id) {
            self.passed = true;
        }
        true
    }
}

/// The look of `q0rg_id` on `frame`. `anchor` says where a followed object
/// sits in stage space.
pub fn scene_look(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    anchor: impl FnMut(u32) -> Option<Mat4>,
) -> SceneLook {
    let (width, height) = (project.meta.stage_width, project.meta.stage_height);
    let setup = project.runtime.scene3d.setup(q0rg_id);
    let fallback;
    let setup: &Scene3dSetup = match setup {
        Some(setup) => setup,
        None => {
            fallback = Scene3dSetup::new(q0rg_id, width, height);
            &fallback
        }
    };
    let camera = setup.camera.pose_at(frame, width, height);
    SceneLook {
        camera,
        orthographic: setup.camera.enabled && setup.camera.orthographic,
        rig: setup.light_rig(frame, &camera, anchor),
        light_flat: (setup.light_flat && setup.has_lights()).then_some(setup.light_flat_layer),
    }
}

/// Does the project have anything 3D at all? Renderers skip all 3D work
/// when it has not.
pub fn project_uses_3d(project: &ProjectV2) -> bool {
    project
        .assets
        .iter()
        .any(|asset| matches!(asset, Asset::Model(_)))
        || project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .any(|placement| placement.fx.three.is_some())
        || project
            .runtime
            .scene3d
            .setups
            .iter()
            .any(|setup| (setup.light_flat && setup.has_lights()) || !setup.liquids.is_empty())
}

/// The liquids of `q0rg_id` showing on `frame`, each with the layer it is
/// drawn with.
pub fn liquid_items(project: &ProjectV2, q0rg_id: u16, frame: u16) -> Vec<(u16, DrawItem)> {
    project
        .runtime
        .scene3d
        .setup(q0rg_id)
        .into_iter()
        .flat_map(|setup| &setup.liquids)
        .filter_map(|liquid| Some((liquid.layer_id, super::liquid::liquid_item(liquid, frame)?)))
        .collect()
}

/// Is a placement of `asset` (`None` for a symbol) with these effects drawn
/// by the 3D renderer?
pub fn is_3d(asset: Option<&Asset>, fx: &PlacementFx) -> bool {
    matches!(asset, Some(Asset::Model(_))) || fx.three.is_some()
}

/// Output pixels per unit of an object's own plane around `local`.
pub fn screen_scale(view: &View3d, matrix: &Mat4, local: V3) -> f32 {
    let world = matrix.transform_point(local);
    let v = view.view.transform_point(world);
    let to = &view.to_output;
    let output = (to.a11 * to.a22 - to.a12 * to.a21).abs().sqrt();
    let object = (matrix.determinant3().abs()).cbrt();
    view.stage_scale_at(v.z.max(1.0)) * output * object
}

/// A placed model's meshes, posed for this frame.
#[allow(clippy::too_many_arguments)]
pub fn model_items(
    project: &ProjectV2,
    model: &Arc<ModelData>,
    matrix: &Mat4,
    space: &Space3d,
    elapsed_frames: f32,
    pose: Option<(u16, u32, u16)>,
    opacity: f32,
    out: &mut Vec<DrawItem>,
) {
    let track = pose.and_then(|(q0rg_id, instance_id, frame)| {
        project
            .runtime
            .scene3d
            .pose(q0rg_id, instance_id)
            .map(|track| (track, frame))
    });
    let world = pose_model(model, space, elapsed_frames, project.meta.fps, track);
    push_model(out, model, matrix, &world, opacity, space.lit);
}

/// Bounds of a placed model on the output, in pixels: min x, min y, max x,
/// max y. Used for picking and selection boxes.
pub fn model_output_bounds(
    model: &ModelData,
    matrix: &Mat4,
    world: &[Mat4],
    view: &View3d,
) -> Option<[f32; 4]> {
    let root = model_root(matrix, model);
    let mut out: Option<[f32; 4]> = None;
    for mesh in model.meshes.iter().filter(|mesh| !mesh.hidden) {
        let m = if mesh.skin.is_some() {
            root
        } else {
            root * super::model::mesh_matrix(mesh, world)
        };
        let (positions, skinned);
        let points: &[V3] = if mesh.skin.is_some() {
            skinned = super::model::skin_mesh(mesh, world).map(|(p, _)| p);
            skinned.as_deref().unwrap_or(&mesh.positions)
        } else {
            positions = &mesh.positions;
            positions
        };
        // Every vertex of small meshes, a sample of big ones.
        let step = (points.len() / 4096).max(1);
        for p in points.iter().step_by(step) {
            let Some([x, y, _]) = view.project(m.transform_point(*p)) else {
                continue;
            };
            out = Some(match out {
                Some([a, b, c, d]) => [a.min(x), b.min(y), c.max(x), d.max(y)],
                None => [x, y, x, y],
            });
        }
    }
    out
}

/// A cached picture of a drawing.
#[derive(Clone)]
struct CachedImage {
    image: Arc<Image>,
    rect: [f32; 4],
    used: u64,
}

struct ImageCache {
    entries: HashMap<(u64, u32), CachedImage>,
    clock: u64,
    bytes: usize,
}

const IMAGE_CACHE_BYTES: usize = 256 << 20;

fn image_cache() -> &'static Mutex<ImageCache> {
    static CACHE: std::sync::OnceLock<Mutex<ImageCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(ImageCache {
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
        })
    })
}

fn cached(
    key: (u64, u32),
    build: impl FnOnce() -> Option<(Image, [f32; 4])>,
) -> Option<(Arc<Image>, [f32; 4])> {
    if let Ok(mut cache) = image_cache().lock() {
        cache.clock += 1;
        let clock = cache.clock;
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.used = clock;
            return Some((entry.image.clone(), entry.rect));
        }
    }
    let (image, rect) = build()?;
    let image = Arc::new(image);
    if let Ok(mut cache) = image_cache().lock() {
        cache.bytes += image.rgba.len();
        let clock = cache.clock;
        cache.entries.insert(
            key,
            CachedImage {
                image: image.clone(),
                rect,
                used: clock,
            },
        );
        while cache.bytes > IMAGE_CACHE_BYTES && cache.entries.len() > 1 {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(entry) = cache.entries.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(entry.image.rgba.len());
            }
        }
    }
    Some((image, rect))
}

/// FNV-1a over bytes.
fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

fn hash_vector(project: &ProjectV2, vector: &crate::v2::VectorAsset) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash_bytes(&mut hash, &vector.asset_id.to_le_bytes());
    for path in &vector.paths {
        hash_bytes(&mut hash, &[u8::from(path.closed)]);
        for anchor in &path.anchors {
            for v in [Some(anchor.point), anchor.in_handle, anchor.out_handle] {
                match v {
                    Some(p) => {
                        hash_bytes(&mut hash, &p.x.to_bits().to_le_bytes());
                        hash_bytes(&mut hash, &p.y.to_bits().to_le_bytes());
                    }
                    None => hash_bytes(&mut hash, &[0xff]),
                }
            }
        }
    }
    if let Some(fill) = vector.fill {
        hash_bytes(&mut hash, &[1, fill.r, fill.g, fill.b, fill.a]);
    }
    if let Some(stroke) = vector.stroke {
        hash_bytes(
            &mut hash,
            &[
                2,
                stroke.color.r,
                stroke.color.g,
                stroke.color.b,
                stroke.color.a,
            ],
        );
        hash_bytes(&mut hash, &stroke.width.to_bits().to_le_bytes());
        hash_bytes(
            &mut hash,
            format!("{:?}{:?}", stroke.cap, stroke.join).as_bytes(),
        );
    }
    if let Some(appearance) = project.asset_appearances.get(&vector.asset_id) {
        hash_bytes(&mut hash, format!("{appearance:?}").as_bytes());
    }
    hash
}

/// Pixels per unit a picture is drawn at for a wanted scale: a power of
/// two at or above it, so small zoom changes reuse the picture.
fn resolution_for(scale: f32) -> f32 {
    let scale = if scale.is_finite() { scale } else { 1.0 };
    2.0f32
        .powf(scale.clamp(1.0 / 16.0, 16.0).log2().ceil())
        .clamp(1.0 / 16.0, 16.0)
}

fn premultiply(mut rgba: Vec<u8>) -> Vec<u8> {
    for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let a = u32::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    rgba
}

const MAX_IMAGE_SIDE: f32 = 4096.0;

/// A picture of a drawing in its own coordinates at `scale` output pixels
/// per unit, and the rectangle of its plane it covers.
pub fn asset_image(
    project: &ProjectV2,
    asset: &Asset,
    scale: f32,
    elapsed_frames: u16,
) -> Option<(Arc<Image>, [f32; 4])> {
    match asset {
        Asset::Bitmap(bitmap) => {
            let key = (Arc::as_ptr(&bitmap.rgba) as u64, 0);
            cached(key, || {
                Some((
                    Image {
                        width: u32::from(bitmap.width),
                        height: u32::from(bitmap.height),
                        rgba: Arc::new(premultiply(bitmap.rgba.to_vec())),
                    },
                    [0.0, 0.0, f32::from(bitmap.width), f32::from(bitmap.height)],
                ))
            })
        }
        Asset::Q0v(video) => {
            let bitmap = crate::raster::video_frame(video, elapsed_frames, project.meta.fps)?;
            Some((
                Arc::new(Image {
                    width: u32::from(bitmap.width),
                    height: u32::from(bitmap.height),
                    rgba: Arc::new(premultiply(bitmap.rgba.to_vec())),
                }),
                [0.0, 0.0, f32::from(bitmap.width), f32::from(bitmap.height)],
            ))
        }
        Asset::Vector(vector) => {
            let resolution = resolution_for(scale);
            let key = (hash_vector(project, vector), resolution.to_bits());
            cached(key, || {
                let (min, max) = crate::raster::vector_local_bounds(project, vector)?;
                let pad = 2.0 / resolution;
                let (x0, y0) = (min.x - pad, min.y - pad);
                let (x1, y1) = (max.x + pad, max.y + pad);
                let mut resolution = resolution;
                let longest = (x1 - x0).max(y1 - y0) * resolution;
                if longest > MAX_IMAGE_SIDE {
                    resolution *= MAX_IMAGE_SIDE / longest;
                }
                let w = (((x1 - x0) * resolution).ceil() as u32).max(1);
                let h = (((y1 - y0) * resolution).ceil() as u32).max(1);
                let transform = Affine::compose(
                    Affine::from_uniform_scale(resolution),
                    Affine {
                        tx: -x0,
                        ty: -y0,
                        ..Affine::IDENTITY
                    },
                );
                let rgba = crate::raster::rasterize_asset_alone(project, asset, transform, w, h, 0);
                Some((
                    Image {
                        width: w,
                        height: h,
                        rgba: Arc::new(premultiply(rgba)),
                    },
                    [
                        x0,
                        y0,
                        x0 + w as f32 / resolution,
                        y0 + h as f32 / resolution,
                    ],
                ))
            })
        }
        Asset::Rig(_) | Asset::Model(_) => None,
    }
}

/// A flat drawing as a plane in 3D.
#[allow(clippy::too_many_arguments)]
pub fn asset_plane(
    project: &ProjectV2,
    asset: &Asset,
    matrix: Mat4,
    view: &View3d,
    elapsed_frames: u16,
    opacity: f32,
    lit: bool,
    layered: bool,
) -> Option<DrawItem> {
    let scale = screen_scale(view, &matrix, V3::ZERO);
    let (image, rect) = asset_image(project, asset, scale, elapsed_frames)?;
    Some(DrawItem::Plane(PlaneDraw {
        image,
        rect,
        matrix,
        opacity,
        lit,
        layered,
    }))
}

/// A symbol's frame flattened onto one plane: what a symbol placed in 3D
/// shows. `revision` names the project's content so the picture can be
/// kept; without it the symbol is drawn afresh.
#[allow(clippy::too_many_arguments)]
pub fn symbol_plane(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    matrix: Mat4,
    view: &View3d,
    opacity: f32,
    lit: bool,
    revision: Option<u64>,
    scene_state: &crate::runtime_scene::RuntimeSceneState,
) -> Option<DrawItem> {
    let (min, max) = crate::raster::q0rg_local_bounds(project, q0rg_id, frame, scene_state)?;
    let centre = V3::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, 0.0);
    let resolution = resolution_for(screen_scale(view, &matrix, centre));
    let build = || {
        let pad = 2.0 / resolution;
        let (x0, y0) = (min.x - pad, min.y - pad);
        let (x1, y1) = (max.x + pad, max.y + pad);
        let mut resolution = resolution;
        let longest = (x1 - x0).max(y1 - y0) * resolution;
        if longest > MAX_IMAGE_SIDE {
            resolution *= MAX_IMAGE_SIDE / longest;
        }
        let w = (((x1 - x0) * resolution).ceil() as u32).max(1);
        let h = (((y1 - y0) * resolution).ceil() as u32).max(1);
        let root = Affine::compose(
            Affine::from_uniform_scale(resolution),
            Affine {
                tx: -x0,
                ty: -y0,
                ..Affine::IDENTITY
            },
        );
        let rgba = crate::raster::three_pass::as_card(|| {
            crate::raster::rasterize_q0rg_frame_with_mask_preview(
                project,
                q0rg_id,
                frame,
                w,
                h,
                1,
                [0, 0, 0, 0],
                root,
                &scene_state.rig_overrides,
                scene_state,
                None,
            )
        });
        Some((
            Image {
                width: w,
                height: h,
                rgba: Arc::new(premultiply(rgba)),
            },
            [
                x0,
                y0,
                x0 + w as f32 / resolution,
                y0 + h as f32 / resolution,
            ],
        ))
    };
    let (image, rect) = match revision {
        Some(revision) => {
            let mut key = 0xcbf2_9ce4_8422_2325u64;
            hash_bytes(&mut key, &revision.to_le_bytes());
            hash_bytes(&mut key, &q0rg_id.to_le_bytes());
            hash_bytes(&mut key, &frame.to_le_bytes());
            hash_bytes(&mut key, b"symbol");
            cached((key, resolution.to_bits()), build)?
        }
        None => {
            let (image, rect) = build()?;
            (Arc::new(image), rect)
        }
    };
    Some(DrawItem::Plane(PlaneDraw {
        image,
        rect,
        matrix,
        opacity,
        lit,
        layered: false,
    }))
}

/// Draws a run of 3D items into a straight-alpha RGBA buffer, over what is
/// there.
pub fn composite_over(buffer: &mut [u8], premultiplied: &[u8]) {
    for (dst, src) in buffer
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(premultiplied.as_chunks::<4>().0.iter())
    {
        let sa = u32::from(src[3]);
        if sa == 0 {
            continue;
        }
        let da = u32::from(dst[3]);
        let inv = 255 - sa;
        let out_a = sa * 255 + da * inv;
        if out_a == 0 {
            continue;
        }
        for c in 0..3 {
            // src is premultiplied; dst is straight.
            let numerator = u32::from(src[c]) * 255 * 255 + u32::from(dst[c]) * da * inv;
            dst[c] = ((numerator + out_a / 2) / out_a).min(255) as u8;
        }
        dst[3] = ((out_a + 127) / 255) as u8;
    }
}

/// Where a placement's own coordinates sit in 3D, from its matrix in a 2D
/// parent and its place in 3D. `pivot` is its transform anchor.
pub fn placement_space_matrix(
    parent: &Mat4,
    local: Affine,
    fx: &PlacementFx,
    pivot: Option<Vec2>,
) -> Mat4 {
    super::scene::placement_matrix(
        parent,
        local,
        fx.three,
        pivot.unwrap_or(Vec2::new(0.0, 0.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_art_is_lit_up_to_its_layer() {
        let look = SceneLook {
            camera: CameraPose::stage(10, 10),
            orthographic: false,
            rig: LightRig::default_for(&CameraPose::stage(10, 10)),
            light_flat: Some(Some(2)),
        };
        let mut cut = look.flat_cut();
        assert!(cut.layer(1));
        assert!(cut.layer(2));
        assert!(!cut.layer(3));
        let mut off = FlatCut::OFF;
        assert!(!off.layer(1));
    }

    #[test]
    fn premultiplied_pixels_land_over_straight_ones() {
        let mut buffer = vec![255, 255, 255, 255];
        composite_over(&mut buffer, &[128, 0, 0, 128]);
        assert!(
            buffer[0] > 250 && buffer[1] < 130 && buffer[3] == 255,
            "{buffer:?}"
        );
        let mut clear = vec![0, 0, 0, 0];
        composite_over(&mut clear, &[100, 50, 0, 200]);
        assert_eq!(clear[3], 200);
        assert!((i32::from(clear[0]) - 128).abs() <= 1);
    }
}
