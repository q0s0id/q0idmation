//! Software rasteriser for `ProjectV2` РІР‚вЂќ turns a single frame of a q0rg
//! into a flat RGBA buffer. Shared between:
//!   * q0editor's `.q0s` export path (rasterise + ship if you want a v1
//!     bitmap-only `.q0s`, or skip rasterisation entirely for v2 vector
//!     `.q0s`),
//!   * q0player's playback loop (when fed a v2 vector `.q0s`, rasterise
//!     each frame on demand instead of blitting pre-baked bitmaps).
//!
//! Dependency-free (`std` only) so the player doesn't pick up GUI deps.
//!
//! Two fill rules:
//!   * `EvenOdd` for closed filled paths (Flash convention).
//!   * `NonZero` for stroke outlines (`brush_outline` self-intersects on
//!     sharp turns; even-odd would punch holes).
//!
//! `rasterize_q0rg_frame` does optional supersampling: pass `ss > 1` and
//! the function rasterises at `ss Р“вЂ” ss` and box-filters down. Caller
//! controls quality/cost trade-off РІР‚вЂќ players default to `1` (live), the
//! exporter could use `2`.

use crate::geom::{brush_outline, flatten_path};
use crate::runtime_scene::RuntimeSceneState;
use crate::transform::Affine;
use crate::v2::{
    Asset, BitmapAsset, BlendMode, InstanceKey, Placement, PlacementFx, ProjectV2, Rgba, Target,
    Transform2D, Vec2, VectorAppearance, VectorAsset, VectorMaterial, MAX_Q0RG_NESTING_DEPTH,
};

/// Render one frame of `q0rg_id` (resolved against `project`) into an
/// RGBA8 buffer of size `w Р“вЂ” h`. `ss` controls supersample factor:
///   * `ss = 1`: no AA, fast РІР‚вЂќ fine for live playback.
///   * `ss = 2`: ~4Р“вЂ” slower, smooth diagonals РІР‚вЂќ sane default for export.
///   * `ss >= 3`: diminishing returns, lots of memory.
///
/// `bg_rgba` is the canvas clear colour. Use `[255, 255, 255, 255]` to
/// match the editor's stage-paper white.
pub fn rasterize_q0rg_frame(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
) -> Vec<u8> {
    rasterize_q0rg_frame_with_rig_overrides(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        &crate::rig::RigRuntimeOverrides::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn rasterize_q0rg_frame_with_rig_overrides(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
    rig_overrides: &crate::rig::RigRuntimeOverrides,
) -> Vec<u8> {
    rasterize_q0rg_frame_with_runtime_overrides(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        rig_overrides,
        &RuntimeSceneState::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn rasterize_q0rg_frame_with_runtime_overrides(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
    rig_overrides: &crate::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
) -> Vec<u8> {
    rasterize_q0rg_frame_with_root_transform(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        Affine::IDENTITY,
        rig_overrides,
        scene_state,
    )
}

/// Render a project frame while mapping logical stage coordinates into an
/// arbitrary output size. This is the canonical offscreen export path: the
/// q0editor viewport is not involved, and q0player keeps using the identity
/// wrapper above for native-size playback.
#[derive(Debug, Clone, Copy)]
struct StageBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl StageBounds {
    fn from_points(points: impl IntoIterator<Item = Vec2>) -> Option<Self> {
        let mut bounds = Self {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for point in points {
            bounds.min_x = bounds.min_x.min(point.x);
            bounds.min_y = bounds.min_y.min(point.y);
            bounds.max_x = bounds.max_x.max(point.x);
            bounds.max_y = bounds.max_y.max(point.y);
        }
        bounds.min_x.is_finite().then_some(bounds)
    }

    fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    fn expand(self, amount: f32) -> Self {
        let amount = amount.max(0.0);
        Self {
            min_x: self.min_x - amount,
            min_y: self.min_y - amount,
            max_x: self.max_x + amount,
            max_y: self.max_y + amount,
        }
    }

    fn translate(self, x: f32, y: f32) -> Self {
        Self {
            min_x: self.min_x + x,
            min_y: self.min_y + y,
            max_x: self.max_x + x,
            max_y: self.max_y + y,
        }
    }

    fn transform(self, affine: Affine) -> Self {
        let corners = [
            Vec2::new(self.min_x, self.min_y),
            Vec2::new(self.max_x, self.min_y),
            Vec2::new(self.max_x, self.max_y),
            Vec2::new(self.min_x, self.max_y),
        ];
        Self::from_points(corners.into_iter().map(|point| affine.apply(point)))
            .expect("finite affine bounds")
    }
}

fn path_bounds(paths: &[crate::v2::Path]) -> Option<StageBounds> {
    StageBounds::from_points(
        paths
            .iter()
            .flat_map(|path| flatten_path(path, FLATTEN_SAMPLES)),
    )
}

fn vector_local_visual_bounds(project: &ProjectV2, vector: &VectorAsset) -> Option<StageBounds> {
    let mut bounds = path_bounds(&vector.paths)?;
    if let Some(stroke) = vector.stroke {
        bounds = bounds.expand(stroke.width * 0.5);
    }
    let Some(appearance) = project.asset_appearances.get(&vector.asset_id) else {
        return Some(bounds);
    };
    let VectorMaterial::SoftHalo { radius, opacity } = appearance.material else {
        return Some(bounds);
    };
    if vector.fill.is_none() || opacity <= 0.0 {
        return Some(bounds);
    }
    let material_paths = if appearance.material_source.is_empty() {
        &vector.paths
    } else {
        &appearance.material_source
    };
    let work_paths = if appearance.clip_mask.is_empty() {
        material_paths
    } else {
        &appearance.clip_mask
    };
    if let Some(halo) = path_bounds(work_paths) {
        // `rasterize_vector_appearance_local_impl` keeps four local units of
        // transparent/filter guard at the lowest supported PPU. Mirror that
        // support conservatively here so the local placement tile never clips it.
        let halo = halo
            .expand(radius.max(0.0) + 4.0)
            .transform(appearance.field_transform);
        bounds = bounds.union(halo);
    }
    Some(bounds)
}

fn asset_stage_bounds(
    project: &ProjectV2,
    asset: &Asset,
    transform: Affine,
) -> Option<StageBounds> {
    let local = match asset {
        Asset::Bitmap(bitmap) => StageBounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: f32::from(bitmap.width),
            max_y: f32::from(bitmap.height),
        },
        Asset::Vector(vector) => vector_local_visual_bounds(project, vector)?,
        Asset::Q0v(video) => {
            let media = q0video::q0v::Q0vFile::parse(video.bytes.clone()).ok()?;
            StageBounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: media.spec.width as f32,
                max_y: media.spec.height as f32,
            }
        }
        Asset::Rig(_) => return None,
    };
    Some(local.transform(transform))
}

fn bounds_with_placement_fx(
    bounds: StageBounds,
    fx: PlacementFx,
    transform_scale: f32,
) -> StageBounds {
    let scale = transform_scale.abs().max(1.0e-4);
    let mut result = bounds;
    if let Some(blur) = fx.blur {
        result = result.union(bounds.expand(blur.radius * scale));
    }
    if let Some(glow) = fx.glow {
        if glow.strength > 0.0 && glow.color.a > 0 {
            result = result.union(bounds.expand(glow.radius * scale));
        }
    }
    if let Some(shadow) = fx.shadow {
        if shadow.strength > 0.0 && shadow.color.a > 0 {
            let shadow_bounds = bounds
                .expand(shadow.blur_radius * scale)
                .translate(shadow.offset_x * scale, shadow.offset_y * scale);
            result = result.union(shadow_bounds);
        }
    }
    result
}

fn placement_target_stage_bounds(
    project: &ProjectV2,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    composed: Affine,
    depth: u8,
    scene_state: &RuntimeSceneState,
) -> Option<StageBounds> {
    if depth > MAX_Q0RG_NESTING_DEPTH {
        return None;
    }
    let rig = crate::rig::rig_for_q0rg(project, host_q0rg_id);
    let rig_pose = rig.map(|rig| crate::rig::evaluate_rig(rig, f32::from(local_frame), &[]));
    let target = match (rig, rig_pose.as_ref()) {
        (Some(rig), Some(pose)) => {
            crate::rig::resolved_variant_target(rig, pose, placement.instance_id, placement.target)
        }
        _ => placement.target,
    };
    match target {
        Target::Asset(asset_id) => {
            let asset = project.assets.iter().find(|asset| asset.id() == asset_id)?;
            let deformed = match (asset, rig, rig_pose.as_ref()) {
                (Asset::Vector(vector), Some(rig), Some(pose)) => {
                    crate::rig::deform_vector_for_instance(rig, pose, placement.instance_id, vector)
                        .map(Asset::Vector)
                }
                _ => None,
            };
            asset_stage_bounds(project, deformed.as_ref().unwrap_or(asset), composed)
        }

        Target::Q0rg(child_id) if child_id != host_q0rg_id => q0rg_stage_bounds(
            project,
            child_id,
            local_frame,
            composed,
            depth + 1,
            scene_state,
        ),
        Target::Q0rg(_) => None,
    }
}

fn q0rg_stage_bounds(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    depth: u8,
    scene_state: &RuntimeSceneState,
) -> Option<StageBounds> {
    if depth > MAX_Q0RG_NESTING_DEPTH {
        return None;
    }
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let mut bounds: Option<StageBounds> = None;
    let rig_pose = crate::rig::rig_for_q0rg(project, q0rg_id)
        .map(|rig| crate::rig::evaluate_rig(rig, f32::from(local_frame), &[]));
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let authored_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let local_affine = scene_state.effective_affine(key, authored_affine);
            let composed = Affine::compose(parent, local_affine);
            let Some(target_bounds) = placement_target_stage_bounds(
                project,
                q0rg_id,
                placement,
                local_frame,
                composed,
                depth,
                scene_state,
            ) else {
                continue;
            };
            let target_bounds =
                bounds_with_placement_fx(target_bounds, active.fx, composed.uniform_scale());
            bounds = Some(match bounds {
                Some(existing) => existing.union(target_bounds),
                None => target_bounds,
            });
        }
    }
    bounds
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeInstanceMetrics {
    pub x: f32,
    pub y: f32,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl RuntimeInstanceMetrics {
    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }
}

/// Resolve one stable display-object instance at a host q0rg frame using the
/// same visual bounds path as the shared renderer. Runtime position overrides
/// are ephemeral and do not mutate `ProjectV2`.
pub fn runtime_instance_metrics(
    project: &ProjectV2,
    host_q0rg_id: u16,
    frame: u16,
    instance_id: u32,
    scene_state: &RuntimeSceneState,
) -> Option<RuntimeInstanceMetrics> {
    if instance_id == 0 {
        return None;
    }
    let q0rg = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == host_q0rg_id)?;
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let rig_pose = crate::rig::rig_for_q0rg(project, host_q0rg_id)
        .map(|rig| crate::rig::evaluate_rig(rig, f32::from(local_frame), &[]));
    for layer in &q0rg.layers {
        if !project.layer_is_visible(host_q0rg_id, layer.layer_id) {
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            let placement = layer.placements.get(active.index)?;
            if placement.instance_id != instance_id {
                continue;
            }
            let authored_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let key = InstanceKey::new(host_q0rg_id, instance_id);
            let effective = scene_state.effective_affine(key, authored_affine);
            let target = placement_target_stage_bounds(
                project,
                host_q0rg_id,
                placement,
                local_frame,
                effective,
                0,
                scene_state,
            )?;
            let visible = bounds_with_placement_fx(target, active.fx, effective.uniform_scale());
            return Some(RuntimeInstanceMetrics {
                x: effective.tx,
                y: effective.ty,
                left: visible.min_x,
                right: visible.max_x,
                top: visible.min_y,
                bottom: visible.max_y,
            });
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CroppedRgba {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Rasterise exactly one active placement into a tightly bounded stage-scaled
/// offscreen surface, apply its group FX, then crop any remaining transparent
/// guard. The rest of the scene is never rasterised or allocated here.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_placement_fx_scaled_cropped(
    project: &ProjectV2,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    composed_stage_transform: Affine,
    fx: PlacementFx,
    w: u32,
    h: u32,
) -> Option<CroppedRgba> {
    if w == 0 || h == 0 {
        return None;
    }
    let stage_w = f32::from(project.meta.stage_width.max(1));
    let stage_h = f32::from(project.meta.stage_height.max(1));
    let target_bounds = placement_target_stage_bounds(
        project,
        host_q0rg_id,
        placement,
        local_frame,
        composed_stage_transform,
        0,
        &RuntimeSceneState::new(),
    )?;
    let fx_bounds =
        bounds_with_placement_fx(target_bounds, fx, composed_stage_transform.uniform_scale());
    let scale_x = w as f32 / stage_w;
    let scale_y = h as f32 / stage_h;
    // Two device pixels on every side cover scanline rounding and linear
    // sampling at the tile edge without growing the surface to stage size.
    let x0 = (fx_bounds.min_x * scale_x).floor() as i64 - 2;
    let y0 = (fx_bounds.min_y * scale_y).floor() as i64 - 2;
    let x1 = (fx_bounds.max_x * scale_x).ceil() as i64 + 2;
    let y1 = (fx_bounds.max_y * scale_y).ceil() as i64 + 2;
    let x0 = x0.clamp(0, i64::from(w)) as u32;
    let y0 = y0.clamp(0, i64::from(h)) as u32;
    let x1 = x1.clamp(0, i64::from(w)) as u32;
    let y1 = y1.clamp(0, i64::from(h)) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let local_width = x1 - x0;
    let local_height = y1 - y0;
    let root = Affine {
        a11: scale_x,
        a12: 0.0,
        a21: 0.0,
        a22: scale_y,
        tx: -(x0 as f32),
        ty: -(y0 as f32),
    };
    let composed_pixels = Affine::compose(root, composed_stage_transform);
    let len = (local_width as usize)
        .saturating_mul(local_height as usize)
        .saturating_mul(4);
    let mut source = vec![0u8; len];
    let rig = crate::rig::rig_for_q0rg(project, host_q0rg_id);
    let rig_pose = rig.map(|rig| crate::rig::evaluate_rig(rig, f32::from(local_frame), &[]));
    render_placement_target(
        project,
        host_q0rg_id,
        placement,
        local_frame,
        composed_pixels,
        &mut source,
        local_width,
        local_height,
        0,
        &crate::rig::RigRuntimeOverrides::new(),
        &RuntimeSceneState::new(),
        rig,
        rig_pose.as_ref(),
    );
    let mut filtered = vec![0u8; len];
    let local_fx = PlacementFx {
        blend_mode: BlendMode::Normal,
        ..fx
    };
    apply_placement_fx(
        &mut filtered,
        &source,
        local_width,
        local_height,
        local_fx,
        composed_pixels.uniform_scale(),
    );
    let crop = crop_rgba_alpha(&filtered, local_width, local_height)?;
    Some(CroppedRgba {
        x: x0 + crop.x,
        y: y0 + crop.y,
        width: crop.width,
        height: crop.height,
        rgba: crop.rgba,
    })
}

/// Rasterize only the outer placement effects (drop shadow and glow), leaving
/// the placement body out of the returned tile. q0editor uses this as a soft
/// underlay and then renders the actual q0rg/vector body through its normal
/// resolution-independent path.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_placement_outer_fx_scaled_cropped(
    project: &ProjectV2,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    composed_stage_transform: Affine,
    fx: PlacementFx,
    w: u32,
    h: u32,
) -> Option<CroppedRgba> {
    if w == 0 || h == 0 {
        return None;
    }
    let glow_active = fx
        .glow
        .is_some_and(|glow| glow.strength > 1.0e-6 && glow.color.a > 0);
    let shadow_active = fx
        .shadow
        .is_some_and(|shadow| shadow.strength > 1.0e-6 && shadow.color.a > 0);
    if !glow_active && !shadow_active {
        return None;
    }

    let stage_w = f32::from(project.meta.stage_width.max(1));
    let stage_h = f32::from(project.meta.stage_height.max(1));
    let target_bounds = placement_target_stage_bounds(
        project,
        host_q0rg_id,
        placement,
        local_frame,
        composed_stage_transform,
        0,
        &RuntimeSceneState::new(),
    )?;
    let outer_fx = PlacementFx {
        opacity: fx.opacity,
        blend_mode: BlendMode::Normal,
        blur: None,
        glow: fx.glow,
        shadow: fx.shadow,
        audio_gain: fx.audio_gain,
        audio_muted: fx.audio_muted,
    };
    let fx_bounds = bounds_with_placement_fx(
        target_bounds,
        outer_fx,
        composed_stage_transform.uniform_scale(),
    );
    let scale_x = w as f32 / stage_w;
    let scale_y = h as f32 / stage_h;
    let x0 = (fx_bounds.min_x * scale_x).floor() as i64 - 2;
    let y0 = (fx_bounds.min_y * scale_y).floor() as i64 - 2;
    let x1 = (fx_bounds.max_x * scale_x).ceil() as i64 + 2;
    let y1 = (fx_bounds.max_y * scale_y).ceil() as i64 + 2;
    let x0 = x0.clamp(0, i64::from(w)) as u32;
    let y0 = y0.clamp(0, i64::from(h)) as u32;
    let x1 = x1.clamp(0, i64::from(w)) as u32;
    let y1 = y1.clamp(0, i64::from(h)) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    let local_width = x1 - x0;
    let local_height = y1 - y0;
    let root = Affine {
        a11: scale_x,
        a12: 0.0,
        a21: 0.0,
        a22: scale_y,
        tx: -(x0 as f32),
        ty: -(y0 as f32),
    };
    let composed_pixels = Affine::compose(root, composed_stage_transform);
    let len = (local_width as usize)
        .saturating_mul(local_height as usize)
        .saturating_mul(4);
    let mut source = vec![0u8; len];
    let rig = crate::rig::rig_for_q0rg(project, host_q0rg_id);
    let rig_pose = rig.map(|rig| crate::rig::evaluate_rig(rig, f32::from(local_frame), &[]));
    render_placement_target(
        project,
        host_q0rg_id,
        placement,
        local_frame,
        composed_pixels,
        &mut source,
        local_width,
        local_height,
        0,
        &crate::rig::RigRuntimeOverrides::new(),
        &RuntimeSceneState::new(),
        rig,
        rig_pose.as_ref(),
    );
    let source_alpha = alpha_channel(&source);
    if source_alpha.iter().all(|alpha| *alpha == 0) {
        return None;
    }

    let local_fx = placement_fx_in_pixels(outer_fx, composed_pixels.uniform_scale());
    let mut group = vec![0u8; len];
    if let Some(shadow) = local_fx.shadow {
        let mask =
            gaussian_blur_alpha(&source_alpha, local_width, local_height, shadow.blur_radius);
        composite_mask_color(
            &mut group,
            &mask,
            local_width,
            local_height,
            shadow.color,
            shadow.strength,
            (
                shadow.offset_x.round() as i32,
                shadow.offset_y.round() as i32,
            ),
        );
    }
    if let Some(glow) = local_fx.glow {
        let mask = gaussian_blur_alpha(&source_alpha, local_width, local_height, glow.radius);
        composite_mask_color(
            &mut group,
            &mask,
            local_width,
            local_height,
            glow.color,
            glow.strength,
            (0, 0),
        );
    }
    if outer_fx.opacity < 0.999_999 {
        let opacity = outer_fx.opacity.clamp(0.0, 1.0);
        for pixel in group.chunks_exact_mut(4) {
            pixel[3] = (f32::from(pixel[3]) * opacity).round().clamp(0.0, 255.0) as u8;
        }
    }
    let crop = crop_rgba_alpha(&group, local_width, local_height)?;
    Some(CroppedRgba {
        x: x0 + crop.x,
        y: y0 + crop.y,
        width: crop.width,
        height: crop.height,
        rgba: crop.rgba,
    })
}
fn crop_rgba_alpha(source: &[u8], width: u32, height: u32) -> Option<CroppedRgba> {
    let (min_x, min_y, max_x, max_y) = rgba_alpha_bounds(source, width, height)?;
    Some(CroppedRgba {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
        rgba: copy_rgba_rect(source, width, min_x, min_y, max_x, max_y),
    })
}

pub fn rasterize_q0rg_frame_scaled(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
) -> Vec<u8> {
    rasterize_q0rg_frame_scaled_with_rig_overrides(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        &crate::rig::RigRuntimeOverrides::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn rasterize_q0rg_frame_scaled_with_rig_overrides(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
    rig_overrides: &crate::rig::RigRuntimeOverrides,
) -> Vec<u8> {
    let stage_w = f32::from(project.meta.stage_width.max(1));
    let stage_h = f32::from(project.meta.stage_height.max(1));
    let root = Affine::from_scale(w as f32 / stage_w, h as f32 / stage_h);
    rasterize_q0rg_frame_with_root_transform(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        root,
        rig_overrides,
        &RuntimeSceneState::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn rasterize_q0rg_frame_with_root_transform(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
    root: Affine,
    rig_overrides: &crate::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
) -> Vec<u8> {
    let ss = ss.max(1);
    let sw = w.saturating_mul(ss);
    let sh = h.saturating_mul(ss);
    let mut hi = vec![0u8; (sw as usize).saturating_mul(sh as usize).saturating_mul(4)];
    for px in hi.chunks_exact_mut(4) {
        px.copy_from_slice(&bg_rgba);
    }
    render_to_buffer(
        project,
        q0rg_id,
        frame,
        Affine::compose(Affine::from_uniform_scale(ss as f32), root),
        &mut hi,
        sw,
        sh,
        0,
        rig_overrides,
        scene_state,
    );
    if ss == 1 {
        return hi;
    }
    downscale_box(&hi, sw, w, h, ss)
}

fn downscale_box(src: &[u8], sw: u32, dw: u32, dh: u32, ss: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize).saturating_mul(dh as usize).saturating_mul(4)];
    let s = ss as usize;
    let n = (s * s) as u64;
    for y in 0..dh as usize {
        for x in 0..dw as usize {
            let mut premul_r = 0u64;
            let mut premul_g = 0u64;
            let mut premul_b = 0u64;
            let mut alpha = 0u64;
            for dy in 0..s {
                for dx in 0..s {
                    let sx = x * s + dx;
                    let sy = y * s + dy;
                    let idx = (sy * sw as usize + sx) * 4;
                    let a = u64::from(src[idx + 3]);
                    premul_r += u64::from(src[idx]) * a;
                    premul_g += u64::from(src[idx + 1]) * a;
                    premul_b += u64::from(src[idx + 2]) * a;
                    alpha += a;
                }
            }
            let oi = (y * dw as usize + x) * 4;
            match std::num::NonZeroU64::new(alpha) {
                None => out[oi..oi + 4].copy_from_slice(&[0, 0, 0, 0]),
                Some(nonzero_alpha) => {
                    let alpha = nonzero_alpha.get();
                    out[oi] = ((premul_r + alpha / 2) / alpha) as u8;
                    out[oi + 1] = ((premul_g + alpha / 2) / alpha) as u8;
                    out[oi + 2] = ((premul_b + alpha / 2) / alpha) as u8;
                    out[oi + 3] = ((alpha + n / 2) / n) as u8;
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_to_buffer(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
    depth: u8,
    rig_overrides: &crate::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
) {
    if depth > MAX_Q0RG_NESTING_DEPTH {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local_frame = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    let rig_pose = crate::rig::rig_for_q0rg(project, q0rg_id).map(|rig| {
        let overrides = rig_overrides
            .get(&q0rg_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        crate::rig::evaluate_rig(rig, f32::from(local_frame), overrides)
    });
    for layer in &q.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let authored_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let local_affine = scene_state.effective_affine(key, authored_affine);
            let composed = Affine::compose(parent, local_affine);
            if active.fx.is_identity() {
                render_placement_target(
                    project,
                    q0rg_id,
                    placement,
                    local_frame,
                    composed,
                    buffer,
                    w,
                    h,
                    depth,
                    rig_overrides,
                    scene_state,
                    crate::rig::rig_for_q0rg(project, q0rg_id),
                    rig_pose.as_ref(),
                );
            } else {
                let mut source = vec![0u8; buffer.len()];
                render_placement_target(
                    project,
                    q0rg_id,
                    placement,
                    local_frame,
                    composed,
                    &mut source,
                    w,
                    h,
                    depth,
                    rig_overrides,
                    scene_state,
                    crate::rig::rig_for_q0rg(project, q0rg_id),
                    rig_pose.as_ref(),
                );
                apply_placement_fx(buffer, &source, w, h, active.fx, composed.uniform_scale());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_placement_target(
    project: &ProjectV2,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    composed: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
    depth: u8,
    rig_overrides: &crate::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
    rig: Option<&crate::v2::RigAsset>,
    rig_pose: Option<&crate::rig::RigPose>,
) {
    let target = match (rig, rig_pose) {
        (Some(rig), Some(pose)) => {
            crate::rig::resolved_variant_target(rig, pose, placement.instance_id, placement.target)
        }
        _ => placement.target,
    };
    match target {
        Target::Asset(id) => {
            if let Some(asset) = project.assets.iter().find(|asset| asset.id() == id) {
                let deformed = match (asset, rig, rig_pose) {
                    (Asset::Vector(vector), Some(rig), Some(pose)) => {
                        crate::rig::deform_vector_for_instance(
                            rig,
                            pose,
                            placement.instance_id,
                            vector,
                        )
                        .map(Asset::Vector)
                    }
                    _ => None,
                };
                let asset = deformed.as_ref().unwrap_or(asset);
                let mut context = AssetRasterContext {
                    transform: composed,
                    buffer,
                    width: w,
                    height: h,
                    elapsed_host_frames: local_frame.saturating_sub(placement.frame),
                    host_fps: project.meta.fps,
                };
                rasterize_asset(asset, project.asset_appearances.get(&id), &mut context);
            }
        }
        Target::Q0rg(child_id) if child_id != host_q0rg_id => {
            render_to_buffer(
                project,
                child_id,
                local_frame,
                composed,
                buffer,
                w,
                h,
                depth + 1,
                rig_overrides,
                scene_state,
            );
        }
        _ => {}
    }
}

fn alpha_channel(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).map(|pixel| pixel[3]).collect()
}

fn gaussian_blur_rgba(source: &[u8], width: u32, height: u32, support_radius: f32) -> Vec<u8> {
    if source.is_empty() || support_radius <= 0.5 {
        return source.to_vec();
    }
    let alpha = alpha_channel(source);
    let blurred_alpha = gaussian_blur_alpha(&alpha, width, height, support_radius);
    let mut premul_channels = [Vec::new(), Vec::new(), Vec::new()];
    for channel in &mut premul_channels {
        channel.reserve(alpha.len());
    }
    for pixel in source.chunks_exact(4) {
        let a = u16::from(pixel[3]);
        for channel in 0..3 {
            premul_channels[channel].push(((u16::from(pixel[channel]) * a + 127) / 255) as u8);
        }
    }
    let blurred_channels =
        premul_channels.map(|channel| gaussian_blur_alpha(&channel, width, height, support_radius));
    let mut out = vec![0u8; source.len()];
    for (index, pixel) in out.chunks_exact_mut(4).enumerate() {
        let a = blurred_alpha[index];
        pixel[3] = a;
        if a == 0 {
            continue;
        }
        for channel in 0..3 {
            pixel[channel] = ((u32::from(blurred_channels[channel][index]) * 255
                + u32::from(a) / 2)
                / u32::from(a))
            .min(255) as u8;
        }
    }
    out
}

fn composite_mask_color(
    target: &mut [u8],
    mask: &[u8],
    width: u32,
    height: u32,
    color: Rgba,
    strength: f32,
    offset: (i32, i32),
) {
    let (offset_x, offset_y) = offset;
    let width_i = width as i32;
    let height_i = height as i32;
    let strength = strength.max(0.0);
    if color.a == 0 || strength <= 0.0 {
        return;
    }
    for sy in 0..height_i {
        let dy = sy + offset_y;
        if dy < 0 || dy >= height_i {
            continue;
        }
        for sx in 0..width_i {
            let dx = sx + offset_x;
            if dx < 0 || dx >= width_i {
                continue;
            }
            let source_index = sy as usize * width as usize + sx as usize;
            let mask_alpha = f32::from(mask[source_index]) / 255.0;
            if mask_alpha <= 0.0 {
                continue;
            }
            let alpha = (mask_alpha * (f32::from(color.a) / 255.0) * strength * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            let destination = ((dy as u32 * width + dx as u32) * 4) as usize;
            blend_pixel(
                target,
                destination,
                Rgba {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: alpha,
                },
            );
        }
    }
}

fn blend_channel(mode: BlendMode, backdrop: u8, source: u8) -> u8 {
    let b = u32::from(backdrop);
    let s = u32::from(source);
    match mode {
        BlendMode::Normal => source,
        BlendMode::Multiply => ((b * s + 127) / 255) as u8,
        BlendMode::Screen => (255 - ((255 - b) * (255 - s) + 127) / 255) as u8,
        BlendMode::Add => (b + s).min(255) as u8,
        BlendMode::Overlay => {
            if b < 128 {
                ((2 * b * s + 127) / 255).min(255) as u8
            } else {
                (255 - ((2 * (255 - b) * (255 - s) + 127) / 255).min(255)) as u8
            }
        }
    }
}

fn composite_rgba_with_blend(destination: &mut [u8], source: &[u8], mode: BlendMode, opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    for (dst, src) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        let sa = (f32::from(src[3]) * opacity / 255.0).clamp(0.0, 1.0);
        if sa <= 0.0 {
            continue;
        }
        let da = f32::from(dst[3]) / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            dst.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        for channel in 0..3 {
            let blended = if da <= 0.0 {
                src[channel]
            } else {
                let mixed = blend_channel(mode, dst[channel], src[channel]);
                (f32::from(src[channel]) * (1.0 - da) + f32::from(mixed) * da)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            let source_premul = f32::from(blended) * sa;
            let destination_premul = f32::from(dst[channel]) * da * (1.0 - sa);
            dst[channel] = ((source_premul + destination_premul) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

fn rgba_alpha_bounds(source: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..height {
        for x in 0..width {
            let alpha = source[((y * width + x) * 4 + 3) as usize];
            if alpha == 0 {
                continue;
            }
            any = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + 1);
            max_y = max_y.max(y + 1);
        }
    }
    any.then_some((min_x, min_y, max_x, max_y))
}

fn copy_rgba_rect(
    source: &[u8],
    source_width: u32,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
) -> Vec<u8> {
    let width = (max_x - min_x) as usize;
    let height = (max_y - min_y) as usize;
    let mut out = vec![0u8; width * height * 4];
    for row in 0..height {
        let source_start = ((min_y as usize + row) * source_width as usize + min_x as usize) * 4;
        let source_end = source_start + width * 4;
        let destination_start = row * width * 4;
        out[destination_start..destination_start + width * 4]
            .copy_from_slice(&source[source_start..source_end]);
    }
    out
}

fn write_rgba_rect(
    destination: &mut [u8],
    destination_width: u32,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    source: &[u8],
) {
    let width = (max_x - min_x) as usize;
    let height = (max_y - min_y) as usize;
    for row in 0..height {
        let destination_start =
            ((min_y as usize + row) * destination_width as usize + min_x as usize) * 4;
        let source_start = row * width * 4;
        destination[destination_start..destination_start + width * 4]
            .copy_from_slice(&source[source_start..source_start + width * 4]);
    }
}

fn placement_fx_in_pixels(fx: PlacementFx, scale: f32) -> PlacementFx {
    let scale = scale.abs().max(1.0e-4);
    PlacementFx {
        opacity: fx.opacity,
        blend_mode: fx.blend_mode,
        blur: fx.blur.map(|blur| crate::v2::BlurFx {
            radius: blur.radius * scale,
        }),
        glow: fx.glow.map(|glow| crate::v2::GlowFx {
            radius: glow.radius * scale,
            ..glow
        }),
        shadow: fx.shadow.map(|shadow| crate::v2::DropShadowFx {
            blur_radius: shadow.blur_radius * scale,
            offset_x: shadow.offset_x * scale,
            offset_y: shadow.offset_y * scale,
            ..shadow
        }),
        audio_gain: fx.audio_gain,
        audio_muted: fx.audio_muted,
    }
}

fn placement_fx_padding(fx: PlacementFx) -> u32 {
    let mut extent = 0.0f32;
    if let Some(blur) = fx.blur {
        extent = extent.max(blur.radius);
    }
    if let Some(glow) = fx.glow {
        extent = extent.max(glow.radius);
    }
    if let Some(shadow) = fx.shadow {
        extent = extent.max(shadow.blur_radius + shadow.offset_x.abs().max(shadow.offset_y.abs()));
    }
    extent.ceil().clamp(0.0, (u32::MAX - 2) as f32) as u32 + 2
}

fn apply_placement_fx_full(
    destination: &mut [u8],
    source: &[u8],
    width: u32,
    height: u32,
    fx: PlacementFx,
) {
    let source_alpha = alpha_channel(source);
    let mut group = vec![0u8; source.len()];

    if let Some(shadow) = fx.shadow {
        let shadow_mask = gaussian_blur_alpha(&source_alpha, width, height, shadow.blur_radius);
        composite_mask_color(
            &mut group,
            &shadow_mask,
            width,
            height,
            shadow.color,
            shadow.strength,
            (
                shadow.offset_x.round() as i32,
                shadow.offset_y.round() as i32,
            ),
        );
    }

    if let Some(glow) = fx.glow {
        let glow_mask = gaussian_blur_alpha(&source_alpha, width, height, glow.radius);
        composite_mask_color(
            &mut group,
            &glow_mask,
            width,
            height,
            glow.color,
            glow.strength,
            (0, 0),
        );
    }

    let body = if let Some(blur) = fx.blur {
        gaussian_blur_rgba(source, width, height, blur.radius)
    } else {
        source.to_vec()
    };
    composite_rgba_with_blend(&mut group, &body, BlendMode::Normal, 1.0);
    composite_rgba_with_blend(destination, &group, fx.blend_mode, fx.opacity);
}

fn apply_placement_fx(
    destination: &mut [u8],
    source: &[u8],
    width: u32,
    height: u32,
    fx: PlacementFx,
    transform_scale: f32,
) {
    let Some((body_min_x, body_min_y, body_max_x, body_max_y)) =
        rgba_alpha_bounds(source, width, height)
    else {
        return;
    };
    let fx = placement_fx_in_pixels(fx, transform_scale);
    let padding = placement_fx_padding(fx);
    let min_x = body_min_x.saturating_sub(padding);
    let min_y = body_min_y.saturating_sub(padding);
    let max_x = body_max_x.saturating_add(padding).min(width);
    let max_y = body_max_y.saturating_add(padding).min(height);
    let tile_width = max_x - min_x;
    let tile_height = max_y - min_y;
    if tile_width == 0 || tile_height == 0 {
        return;
    }
    let source_tile = copy_rgba_rect(source, width, min_x, min_y, max_x, max_y);
    let mut destination_tile = copy_rgba_rect(destination, width, min_x, min_y, max_x, max_y);
    apply_placement_fx_full(
        &mut destination_tile,
        &source_tile,
        tile_width,
        tile_height,
        fx,
    );
    write_rgba_rect(
        destination,
        width,
        min_x,
        min_y,
        max_x,
        max_y,
        &destination_tile,
    );
}

/// Return every placement from the latest complete layer keyframe at or
/// before `frame`, with tween transforms baked for the requested playhead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivePlacementState {
    pub index: usize,
    pub transform: Transform2D,
    pub fx: PlacementFx,
}

pub fn active_placement_states_at(
    layer: &crate::v2::Layer,
    frame: u16,
) -> Vec<ActivePlacementState> {
    let Some(keyframe) = layer
        .keyframe_frames()
        .into_iter()
        .filter(|candidate| *candidate <= frame)
        .max()
    else {
        return Vec::new();
    };

    let source_indices: Vec<usize> = layer
        .placements
        .iter()
        .enumerate()
        .filter_map(|(index, placement)| (placement.frame == keyframe).then_some(index))
        .collect();

    source_indices
        .iter()
        .enumerate()
        .map(|(source_order, index)| {
            let placement = &layer.placements[*index];
            let (transform, fx) = match placement.tween.to_frame() {
                Some(to_frame) if to_frame > keyframe && frame >= keyframe => {
                    let occurrence = source_indices[..source_order]
                        .iter()
                        .filter(|candidate| {
                            layer.placements[**candidate].target == placement.target
                        })
                        .count();
                    let target = layer
                        .placements
                        .iter()
                        .filter(|candidate| {
                            candidate.frame == to_frame && candidate.target == placement.target
                        })
                        .nth(occurrence);
                    let denominator = f32::from(to_frame - keyframe);
                    let raw_t = (f32::from(frame - keyframe) / denominator).clamp(0.0, 1.0);
                    let t = placement.tween.easing().sample(raw_t);
                    let target_transform = target
                        .map(|candidate| candidate.transform)
                        .unwrap_or(placement.transform);
                    let target_fx = target.map(|candidate| candidate.fx).unwrap_or(placement.fx);
                    (
                        lerp_transform(placement.transform, target_transform, t),
                        placement.fx.lerp_to(target_fx, t),
                    )
                }
                Some(_) | None => (placement.transform, placement.fx),
            };
            ActivePlacementState {
                index: *index,
                transform,
                fx,
            }
        })
        .collect()
}

pub fn active_placements_at(layer: &crate::v2::Layer, frame: u16) -> Vec<(usize, Transform2D)> {
    active_placement_states_at(layer, frame)
        .into_iter()
        .map(|active| (active.index, active.transform))
        .collect()
}

pub fn q0rg_frame_has_placement_fx(project: &ProjectV2, q0rg_id: u16, frame: u16) -> bool {
    fn recurse(project: &ProjectV2, q0rg_id: u16, frame: u16, depth: u8) -> bool {
        if depth > MAX_Q0RG_NESTING_DEPTH {
            return false;
        }
        let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
            return false;
        };
        let local_frame = if q0rg.frame_count > 0 {
            frame % q0rg.frame_count
        } else {
            0
        };
        for layer in &q0rg.layers {
            if !project.layer_is_visible(q0rg_id, layer.layer_id) {
                continue;
            }
            for active in active_placement_states_at(layer, local_frame) {
                if !active.fx.is_identity() {
                    return true;
                }
                let Some(placement) = layer.placements.get(active.index) else {
                    continue;
                };
                if let Target::Q0rg(child_id) = placement.target {
                    if child_id != q0rg_id && recurse(project, child_id, local_frame, depth + 1) {
                        return true;
                    }
                }
            }
        }
        false
    }
    recurse(project, q0rg_id, frame, 0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn lerp_transform(a: Transform2D, b: Transform2D, t: f32) -> Transform2D {
    Transform2D {
        tx: lerp(a.tx, b.tx, t),
        ty: lerp(a.ty, b.ty, t),
        sx: lerp(a.sx, b.sx, t),
        sy: lerp(a.sy, b.sy, t),
        rotation: lerp(a.rotation, b.rotation, t),
        skew_x: lerp(a.skew_x, b.skew_x, t),
        skew_y: lerp(a.skew_y, b.skew_y, t),
    }
}

const FLATTEN_SAMPLES: usize = 16;

struct AssetRasterContext<'a> {
    transform: Affine,
    buffer: &'a mut [u8],
    width: u32,
    height: u32,
    elapsed_host_frames: u16,
    host_fps: u16,
}

fn rasterize_asset(
    asset: &Asset,
    appearance: Option<&VectorAppearance>,
    context: &mut AssetRasterContext<'_>,
) {
    match asset {
        Asset::Bitmap(b) => rasterize_bitmap(
            b,
            context.transform,
            context.buffer,
            context.width,
            context.height,
        ),
        Asset::Vector(v) => rasterize_vector(
            v,
            appearance,
            context.transform,
            context.buffer,
            context.width,
            context.height,
        ),
        Asset::Q0v(v) => {
            let Ok(media) = q0video::q0v::Q0vFile::parse(v.bytes.clone()) else {
                return;
            };
            let Some(frame_index) = media.spec.video_frame_for_host_frame(
                u32::from(context.elapsed_host_frames),
                u32::from(context.host_fps.max(1)),
            ) else {
                return;
            };
            let Ok(rgba) = media.decode_frame_rgba(frame_index) else {
                return;
            };
            let Ok(width) = u16::try_from(media.spec.width) else {
                return;
            };
            let Ok(height) = u16::try_from(media.spec.height) else {
                return;
            };
            rasterize_bitmap(
                &BitmapAsset {
                    asset_id: v.asset_id,
                    width,
                    height,
                    rgba,
                },
                context.transform,
                context.buffer,
                context.width,
                context.height,
            );
        }
        Asset::Rig(_) => {}
    }
}

#[derive(Debug, Clone)]
pub struct RasterizedVectorAppearance {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub local_min: Vec2,
    pub pixels_per_unit: f32,
}

/// Rasterize one vector material in asset-local space. The material is resolved
/// first and the erase mask is multiplied afterwards, so erasing never changes
/// source geometry and a regenerated blur cannot grow back into an erased area.
pub fn rasterize_vector_appearance_local(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    pixels_per_unit: f32,
) -> Option<RasterizedVectorAppearance> {
    rasterize_vector_appearance_local_impl(vector, appearance, pixels_per_unit, true)
}

/// Rasterize only the soft material outside the vector body. q0editor draws
/// this layer underneath a real tessellated vector fill, so zooming never
/// turns the actual artwork into a bitmap.
pub fn rasterize_vector_halo_local(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    pixels_per_unit: f32,
) -> Option<RasterizedVectorAppearance> {
    rasterize_vector_appearance_local_impl(vector, appearance, pixels_per_unit, false)
}

fn rasterize_vector_appearance_local_impl(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    pixels_per_unit: f32,
    include_base: bool,
) -> Option<RasterizedVectorAppearance> {
    let fill = vector.fill?;
    let pixels_per_unit = pixels_per_unit.clamp(0.5, 8.0);
    let material_paths: &[crate::v2::Path] = if appearance.material_source.is_empty() {
        &vector.paths
    } else {
        &appearance.material_source
    };
    let path_bounds = |paths: &[crate::v2::Path]| -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for path in paths.iter().filter(|path| path.closed) {
            for point in flatten_path(path, FLATTEN_SAMPLES) {
                min_x = min_x.min(point.x);
                min_y = min_y.min(point.y);
                max_x = max_x.max(point.x);
                max_y = max_y.max(point.y);
            }
        }
        (min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite())
            .then_some((min_x, min_y, max_x, max_y))
    };
    let source_bounds = path_bounds(material_paths)?;
    let material_margin = match appearance.material {
        VectorMaterial::Solid => 0.0,
        VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
    };
    // Keep a transparent texel guard outside the finite filter support. Linear
    // sampling must never touch the texture edge while the halo still has alpha,
    // otherwise the vector/halo seam exposes square holes at high zoom.
    let filter_guard = 4.0 / pixels_per_unit;
    let (work_bounds, work_margin) = if appearance.clip_mask.is_empty() {
        (source_bounds, material_margin + filter_guard)
    } else {
        // A post-material fragment only needs source samples within one finite
        // filter radius of its clip. Rasterising the complete frozen source for
        // every tiny split fragment was the dominant selection/drag stall.
        (
            path_bounds(&appearance.clip_mask)?,
            material_margin + filter_guard,
        )
    };
    let local_min = Vec2::new(work_bounds.0 - work_margin, work_bounds.1 - work_margin);
    let local_max = Vec2::new(work_bounds.2 + work_margin, work_bounds.3 + work_margin);
    let width = (((local_max.x - local_min.x) * pixels_per_unit).ceil() as u32).max(1);
    let height = (((local_max.y - local_min.y) * pixels_per_unit).ceil() as u32).max(1);
    const MAX_SIDE: u32 = 8192;
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }

    let local_to_pixel = Affine {
        a11: pixels_per_unit,
        a12: 0.0,
        a21: 0.0,
        a22: pixels_per_unit,
        tx: -local_min.x * pixels_per_unit,
        ty: -local_min.y * pixels_per_unit,
    };
    let to_contours = |paths: &[crate::v2::Path]| -> Vec<Vec<Vec2>> {
        paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path, FLATTEN_SAMPLES)
                    .into_iter()
                    .map(|point| local_to_pixel.apply(point))
                    .collect()
            })
            .collect()
    };
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let raster_alpha = |contours: &[Vec<Vec2>]| -> Vec<u8> {
        let mut rgba = vec![0u8; pixel_count.saturating_mul(4)];
        scanline_fill_multi(
            contours,
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            &mut rgba,
            width,
            height,
        );
        rgba.chunks_exact(4).map(|pixel| pixel[3]).collect()
    };
    let material_source_alpha = raster_alpha(&to_contours(material_paths));

    let material_alpha = match appearance.material {
        VectorMaterial::Solid => {
            if include_base {
                raster_alpha(&to_contours(&vector.paths))
                    .into_iter()
                    .map(|alpha| ((u16::from(alpha) * u16::from(fill.a) + 127) / 255) as u8)
                    .collect::<Vec<_>>()
            } else {
                vec![0; pixel_count]
            }
        }
        VectorMaterial::SoftHalo { radius, opacity } => {
            let blurred = gaussian_blur_alpha(
                &material_source_alpha,
                width,
                height,
                radius * pixels_per_unit,
            );
            if include_base {
                let body_alpha = raster_alpha(&to_contours(&vector.paths));
                body_alpha
                    .into_iter()
                    .zip(blurred)
                    .map(|(body, blur)| {
                        let base = f32::from(body) * f32::from(fill.a) / 255.0;
                        let halo = f32::from(blur) * f32::from(fill.a) / 255.0 * opacity;
                        base.max(halo).clamp(0.0, 255.0).round() as u8
                    })
                    .collect::<Vec<_>>()
            } else {
                // Keep the halo continuous underneath the real vector. Cutting
                // a bitmap-shaped hole here can never line up exactly with
                // egui/lyon vector AA and exposes square background gaps.
                blurred
                    .into_iter()
                    .map(|blur| {
                        (f32::from(blur) * f32::from(fill.a) / 255.0 * opacity)
                            .clamp(0.0, 255.0)
                            .round() as u8
                    })
                    .collect::<Vec<_>>()
            }
        }
    };

    let erase_alpha = if appearance.erase_mask.is_empty() {
        vec![0u8; pixel_count]
    } else {
        raster_alpha(&to_contours(&appearance.erase_mask))
    };
    let clip_alpha = if appearance.clip_mask.is_empty() {
        vec![255u8; pixel_count]
    } else {
        raster_alpha(&to_contours(&appearance.clip_mask))
    };

    let mut rgba = vec![0u8; pixel_count.saturating_mul(4)];
    for (index, out) in rgba.chunks_exact_mut(4).enumerate() {
        let visible =
            (u32::from(255 - erase_alpha[index]) * u32::from(clip_alpha[index]) + 127) / 255;
        let alpha = ((u32::from(material_alpha[index]) * visible + 127) / 255) as u8;
        out.copy_from_slice(&[fill.r, fill.g, fill.b, alpha]);
    }
    Some(RasterizedVectorAppearance {
        rgba,
        width,
        height,
        local_min,
        pixels_per_unit,
    })
}

fn rasterize_vector_body_local(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    pixels_per_unit: f32,
) -> Option<RasterizedVectorAppearance> {
    let fill = vector.fill?;
    let pixels_per_unit = pixels_per_unit.clamp(0.5, 8.0);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for path in vector.paths.iter().filter(|path| path.closed) {
        for point in flatten_path(path, FLATTEN_SAMPLES) {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
    }
    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return None;
    }
    let guard = 2.0 / pixels_per_unit;
    let local_min = Vec2::new(min_x - guard, min_y - guard);
    let local_max = Vec2::new(max_x + guard, max_y + guard);
    let width = (((local_max.x - local_min.x) * pixels_per_unit).ceil() as u32).max(1);
    let height = (((local_max.y - local_min.y) * pixels_per_unit).ceil() as u32).max(1);
    const MAX_SIDE: u32 = 8192;
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let local_to_pixel = Affine {
        a11: pixels_per_unit,
        a12: 0.0,
        a21: 0.0,
        a22: pixels_per_unit,
        tx: -local_min.x * pixels_per_unit,
        ty: -local_min.y * pixels_per_unit,
    };
    let contours = |paths: &[crate::v2::Path], transform: Affine| -> Vec<Vec<Vec2>> {
        let composed = Affine::compose(local_to_pixel, transform);
        paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path, FLATTEN_SAMPLES)
                    .into_iter()
                    .map(|point| composed.apply(point))
                    .collect()
            })
            .collect()
    };
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let raster_alpha = |polygons: &[Vec<Vec2>]| -> Vec<u8> {
        let mut rgba = vec![0u8; pixel_count.saturating_mul(4)];
        scanline_fill_multi(
            polygons,
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            &mut rgba,
            width,
            height,
        );
        rgba.chunks_exact(4).map(|pixel| pixel[3]).collect()
    };
    let body_alpha = raster_alpha(&contours(&vector.paths, Affine::IDENTITY));
    let clip_alpha = if appearance.clip_mask.is_empty() {
        vec![255u8; pixel_count]
    } else {
        raster_alpha(&contours(&appearance.clip_mask, appearance.field_transform))
    };
    let erase_alpha = if appearance.erase_mask.is_empty() {
        vec![0u8; pixel_count]
    } else {
        raster_alpha(&contours(
            &appearance.erase_mask,
            appearance.field_transform,
        ))
    };
    let mut rgba = vec![0u8; pixel_count.saturating_mul(4)];
    for (index, out) in rgba.chunks_exact_mut(4).enumerate() {
        let visible =
            (u32::from(clip_alpha[index]) * u32::from(255 - erase_alpha[index]) + 127) / 255;
        let alpha = (u32::from(body_alpha[index]) * visible * u32::from(fill.a) + 32_512) / 65_025;
        out.copy_from_slice(&[fill.r, fill.g, fill.b, alpha.min(255) as u8]);
    }
    Some(RasterizedVectorAppearance {
        rgba,
        width,
        height,
        local_min,
        pixels_per_unit,
    })
}

fn gaussian_blur_alpha(source: &[u8], width: u32, height: u32, support_radius: f32) -> Vec<u8> {
    if source.is_empty() || support_radius <= 0.5 {
        return source.to_vec();
    }
    let sigma = (support_radius / 3.5).max(0.45);
    let radii = gaussian_box_radii(sigma, 3);
    let mut current: Vec<f32> = source.iter().map(|value| f32::from(*value)).collect();
    let mut scratch = vec![0.0f32; current.len()];
    for radius in radii {
        box_blur_horizontal_zero(&current, &mut scratch, width, height, radius);
        box_blur_vertical_zero(&scratch, &mut current, width, height, radius);
    }
    current
        .into_iter()
        .map(|value| value.clamp(0.0, 255.0).round() as u8)
        .collect()
}

/// Three box filters approximate a Gaussian very closely while keeping blur
/// cost O(pixel_count), independent of the visual radius. The old convolution
/// was O(pixel_count * radius), which is why crossing a zoom bucket could stall
/// q0editor for half a second on a large glowing raw fill.
fn gaussian_box_radii(sigma: f32, passes: usize) -> Vec<usize> {
    let passes_f = passes as f32;
    let ideal = ((12.0 * sigma * sigma / passes_f) + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    lower = lower.max(1);
    let upper = lower + 2;
    let numerator = 12.0 * sigma * sigma
        - passes_f * (lower * lower) as f32
        - 4.0 * passes_f * lower as f32
        - 3.0 * passes_f;
    let denominator = (-4 * lower - 4) as f32;
    let lower_count = (numerator / denominator).round().clamp(0.0, passes_f) as usize;
    (0..passes)
        .map(|index| {
            let width = if index < lower_count { lower } else { upper };
            ((width - 1) / 2).max(0) as usize
        })
        .collect()
}

fn box_blur_horizontal_zero(
    source: &[f32],
    output: &mut [f32],
    width: u32,
    height: u32,
    radius: usize,
) {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return;
    }
    if radius == 0 {
        output.copy_from_slice(source);
        return;
    }
    let denominator = (radius * 2 + 1) as f32;
    let mut prefix = vec![0.0f32; w + 1];
    for y in 0..h {
        prefix.fill(0.0);
        let row = y * w;
        for x in 0..w {
            prefix[x + 1] = prefix[x] + source[row + x];
        }
        for x in 0..w {
            let left = x.saturating_sub(radius);
            let right = (x + radius + 1).min(w);
            output[row + x] = (prefix[right] - prefix[left]) / denominator;
        }
    }
}

fn box_blur_vertical_zero(
    source: &[f32],
    output: &mut [f32],
    width: u32,
    height: u32,
    radius: usize,
) {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return;
    }
    if radius == 0 {
        output.copy_from_slice(source);
        return;
    }
    let denominator = (radius * 2 + 1) as f32;
    let mut prefix = vec![0.0f32; h + 1];
    for x in 0..w {
        prefix.fill(0.0);
        for y in 0..h {
            prefix[y + 1] = prefix[y] + source[y * w + x];
        }
        for y in 0..h {
            let top = y.saturating_sub(radius);
            let bottom = (y + radius + 1).min(h);
            output[y * w + x] = (prefix[bottom] - prefix[top]) / denominator;
        }
    }
}

fn rasterize_vector(
    v: &VectorAsset,
    appearance: Option<&VectorAppearance>,
    t: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
) {
    if let Some(appearance) = appearance {
        if matches!(appearance.material, VectorMaterial::SoftHalo { .. }) {
            let field_transform = Affine::compose(t, appearance.field_transform);
            let pixels_per_unit = field_transform.uniform_scale().clamp(1.0, 4.0);
            if let Some(tile) = rasterize_vector_halo_local(v, appearance, pixels_per_unit) {
                composite_appearance_tile(&tile, field_transform, buffer, w, h);
            }
        }
        let body_ppu = t.uniform_scale().clamp(1.0, 4.0);
        if let Some(body) = rasterize_vector_body_local(v, appearance, body_ppu) {
            composite_appearance_tile(&body, t, buffer, w, h);
        }
        return;
    }
    if let Some(fill) = v.fill {
        let contours: Vec<Vec<Vec2>> = v
            .paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path, FLATTEN_SAMPLES)
                    .iter()
                    .map(|point| t.apply(*point))
                    .collect()
            })
            .collect();
        scanline_fill_multi(&contours, fill, buffer, w, h);
    }
    for path in &v.paths {
        let polyline_local = flatten_path(path, FLATTEN_SAMPLES);
        if polyline_local.len() < 2 {
            continue;
        }
        let world: Vec<Vec2> = polyline_local.iter().map(|p| t.apply(*p)).collect();

        if let Some(stroke) = &v.stroke {
            let half = (stroke.width * 0.5 * t.uniform_scale()).max(0.5);
            let outline = brush_outline(&world, half, 8);
            if outline.len() >= 3 {
                scanline_fill(&outline, stroke.color, buffer, w, h);
            }
        }
    }
}

fn composite_appearance_tile(
    tile: &RasterizedVectorAppearance,
    t: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
) {
    let local_max = Vec2::new(
        tile.local_min.x + tile.width as f32 / tile.pixels_per_unit,
        tile.local_min.y + tile.height as f32 / tile.pixels_per_unit,
    );
    let corners = [
        tile.local_min,
        Vec2::new(local_max.x, tile.local_min.y),
        local_max,
        Vec2::new(tile.local_min.x, local_max.y),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for corner in corners {
        let point = t.apply(corner);
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    let Some(inv) = t.inverse() else {
        return;
    };
    let x0 = (min_x.floor() as i32).max(0);
    let y0 = (min_y.floor() as i32).max(0);
    let x1 = (max_x.ceil() as i32).min(w as i32 - 1);
    let y1 = (max_y.ceil() as i32).min(h as i32 - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            let local = inv.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            let tx = (local.x - tile.local_min.x) * tile.pixels_per_unit - 0.5;
            let ty = (local.y - tile.local_min.y) * tile.pixels_per_unit - 0.5;
            let Some(color) = sample_tile_bilinear(tile, tx, ty) else {
                continue;
            };
            if color.a == 0 {
                continue;
            }
            let dst = ((y as u32 * w + x as u32) * 4) as usize;
            blend_pixel(buffer, dst, color);
        }
    }
}

fn sample_tile_bilinear(tile: &RasterizedVectorAppearance, x: f32, y: f32) -> Option<Rgba> {
    if x < -0.5 || y < -0.5 || x > tile.width as f32 - 0.5 || y > tile.height as f32 - 0.5 {
        return None;
    }
    let x0 = x.floor().clamp(0.0, tile.width.saturating_sub(1) as f32) as u32;
    let y0 = y.floor().clamp(0.0, tile.height.saturating_sub(1) as f32) as u32;
    let x1 = (x0 + 1).min(tile.width - 1);
    let y1 = (y0 + 1).min(tile.height - 1);
    let fx = (x - x.floor()).clamp(0.0, 1.0);
    let fy = (y - y.floor()).clamp(0.0, 1.0);
    let sample = |sx: u32, sy: u32| -> [u8; 4] {
        let index = ((sy * tile.width + sx) * 4) as usize;
        [
            tile.rgba[index],
            tile.rgba[index + 1],
            tile.rgba[index + 2],
            tile.rgba[index + 3],
        ]
    };
    let a = sample(x0, y0);
    let b = sample(x1, y0);
    let c = sample(x0, y1);
    let d = sample(x1, y1);
    let lerp = |p: u8, q: u8, t: f32| f32::from(p) + (f32::from(q) - f32::from(p)) * t;
    let channel = |index: usize| {
        let top = lerp(a[index], b[index], fx);
        let bottom = lerp(c[index], d[index], fx);
        (top + (bottom - top) * fy).clamp(0.0, 255.0).round() as u8
    };
    Some(Rgba {
        r: channel(0),
        g: channel(1),
        b: channel(2),
        a: channel(3),
    })
}

fn rasterize_bitmap(b: &BitmapAsset, t: Affine, buffer: &mut [u8], w: u32, h: u32) {
    if b.width == 0 || b.height == 0 {
        return;
    }
    let bw = b.width as f32;
    let bh = b.height as f32;
    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(bw, 0.0),
        Vec2::new(bw, bh),
        Vec2::new(0.0, bh),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for c in &corners {
        let p = t.apply(*c);
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let Some(inv) = t.inverse() else {
        return;
    };
    let x_start = (min_x.floor() as i32).max(0);
    let x_end = (max_x.ceil() as i32).min(w as i32 - 1);
    let y_start = (min_y.floor() as i32).max(0);
    let y_end = (max_y.ceil() as i32).min(h as i32 - 1);
    for yi in y_start..=y_end {
        for xi in x_start..=x_end {
            let local = inv.apply(Vec2::new(xi as f32 + 0.5, yi as f32 + 0.5));
            if local.x < 0.0 || local.y < 0.0 || local.x >= bw || local.y >= bh {
                continue;
            }
            let lx = local.x as u32;
            let ly = local.y as u32;
            let src = ((ly * b.width as u32 + lx) * 4) as usize;
            let dst = ((yi as u32 * w + xi as u32) * 4) as usize;
            blend_pixel(
                buffer,
                dst,
                Rgba {
                    r: b.rgba[src],
                    g: b.rgba[src + 1],
                    b: b.rgba[src + 2],
                    a: b.rgba[src + 3],
                },
            );
        }
    }
}

fn scanline_fill(polygon: &[Vec2], color: Rgba, buffer: &mut [u8], w: u32, h: u32) {
    scanline_fill_multi(&[polygon.to_vec()], color, buffer, w, h);
}

fn scanline_fill_multi(polygons: &[Vec<Vec2>], color: Rgba, buffer: &mut [u8], w: u32, h: u32) {
    if polygons.is_empty() || color.a == 0 || h == 0 {
        return;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for polygon in polygons {
        for point in polygon {
            min_y = min_y.min(point.y);
            max_y = max_y.max(point.y);
        }
    }
    if !min_y.is_finite() {
        return;
    }
    let y_start = (min_y.floor() as i32).max(0);
    let y_end = (max_y.ceil() as i32).min(h as i32 - 1);
    if y_start > y_end {
        return;
    }

    // The previous rasterizer scanned every polygon edge for every output row.
    // Bucket the exact same half-open winding crossings by scanline once so a
    // dense brush contour pays for edges only on rows those edges can cross.
    let row_count = (y_end - y_start + 1) as usize;
    let mut crossings_by_row: Vec<Vec<(f32, i32)>> = vec![Vec::new(); row_count];
    for polygon in polygons {
        if polygon.len() < 3 {
            continue;
        }
        for index in 0..polygon.len() {
            let a = polygon[index];
            let b = polygon[(index + 1) % polygon.len()];
            let denom = b.y - a.y;
            if denom.abs() < 1e-6 {
                continue;
            }
            let low_y = a.y.min(b.y);
            let high_y = a.y.max(b.y);
            let edge_start = ((low_y - 0.5).ceil() as i32).max(y_start);
            let edge_end = (((high_y - 0.5).ceil() as i32) - 1).min(y_end);
            if edge_start > edge_end {
                continue;
            }
            let sign = if b.y > a.y { 1 } else { -1 };
            for yi in edge_start..=edge_end {
                let y = yi as f32 + 0.5;
                let t = (y - a.y) / denom;
                let x = a.x + t * (b.x - a.x);
                crossings_by_row[(yi - y_start) as usize].push((x, sign));
            }
        }
    }

    for (row_index, crossings) in crossings_by_row.iter_mut().enumerate() {
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let yi = y_start + row_index as i32;
        let mut winding = 0i32;
        let mut last_x: Option<f32> = None;
        for (x, sign) in crossings.iter() {
            if winding != 0 {
                if let Some(left_x) = last_x {
                    paint_span(buffer, w, yi, left_x, *x, color);
                }
            }
            winding += sign;
            last_x = Some(*x);
        }
    }
}

#[cfg(test)]
fn scanline_fill_multi_reference(
    polygons: &[Vec<Vec2>],
    color: Rgba,
    buffer: &mut [u8],
    w: u32,
    h: u32,
) {
    if polygons.is_empty() || color.a == 0 {
        return;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for polygon in polygons {
        for point in polygon {
            min_y = min_y.min(point.y);
            max_y = max_y.max(point.y);
        }
    }
    if !min_y.is_finite() {
        return;
    }
    let y_start = (min_y.floor() as i32).max(0);
    let y_end = (max_y.ceil() as i32).min(h as i32 - 1);
    if y_start > y_end {
        return;
    }
    let mut crossings: Vec<(f32, i32)> = Vec::with_capacity(8);
    for yi in y_start..=y_end {
        let y = yi as f32 + 0.5;
        crossings.clear();
        for polygon in polygons {
            if polygon.len() < 3 {
                continue;
            }
            for index in 0..polygon.len() {
                let a = polygon[index];
                let b = polygon[(index + 1) % polygon.len()];
                let cross = (a.y <= y && b.y > y) || (b.y <= y && a.y > y);
                if !cross {
                    continue;
                }
                let denom = b.y - a.y;
                if denom.abs() < 1e-6 {
                    continue;
                }
                let t = (y - a.y) / denom;
                let x = a.x + t * (b.x - a.x);
                let sign = if b.y > a.y { 1 } else { -1 };
                crossings.push((x, sign));
            }
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut winding = 0i32;
        let mut last_x: Option<f32> = None;
        for (x, sign) in &crossings {
            if winding != 0 {
                if let Some(left_x) = last_x {
                    paint_span(buffer, w, yi, left_x, *x, color);
                }
            }
            winding += sign;
            last_x = Some(*x);
        }
    }
}
fn paint_span(buffer: &mut [u8], w: u32, yi: i32, x0: f32, x1: f32, color: Rgba) {
    if yi < 0 {
        return;
    }
    let xi0 = x0.max(0.0).round() as i32;
    let xi1 = x1.min((w as i32 - 1) as f32).round() as i32;
    if xi0 > xi1 {
        return;
    }
    for xi in xi0..=xi1 {
        if xi < 0 || xi >= w as i32 {
            continue;
        }
        let dst = ((yi as u32 * w + xi as u32) * 4) as usize;
        blend_pixel(buffer, dst, color);
    }
}

fn blend_pixel(buffer: &mut [u8], idx: usize, c: Rgba) {
    if c.a == 0 {
        return;
    }
    if c.a == 255 {
        buffer[idx] = c.r;
        buffer[idx + 1] = c.g;
        buffer[idx + 2] = c.b;
        buffer[idx + 3] = 255;
        return;
    }

    // Straight-alpha source-over. The old path blended RGB as though it were
    // premultiplied, which was invisible on the editor's opaque white paper
    // but produced dark fringes in transparent PNG exports.
    let sa = u32::from(c.a);
    let da = u32::from(buffer[idx + 3]);
    let inv_sa = 255 - sa;
    let out_a_numerator = sa * 255 + da * inv_sa;
    if out_a_numerator == 0 {
        buffer[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    let blend_channel = |src: u8, dst: u8| {
        let numerator = u32::from(src) * sa * 255 + u32::from(dst) * da * inv_sa;
        ((numerator + out_a_numerator / 2) / out_a_numerator) as u8
    };
    buffer[idx] = blend_channel(c.r, buffer[idx]);
    buffer[idx + 1] = blend_channel(c.g, buffer[idx + 1]);
    buffer[idx + 2] = blend_channel(c.b, buffer[idx + 2]);
    buffer[idx + 3] = ((out_a_numerator + 127) / 255) as u8;
}

// (Affine moved to crate::transform РІР‚вЂќ shared with the editor / player
// renderers so q0rg parent transforms compose identically across all
// three rasterise/draw paths.)

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::v2::{Anchor, Layer, Path as VPath, Placement, ProjectMeta, Q0rg, Tween};

    #[test]
    fn tween_interpolates_alpha_and_filters_with_the_same_progress() {
        let layer = crate::v2::Layer {
            layer_id: 1,
            name: "fx tween".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::Linear { to_frame: 10 },
                    fx: PlacementFx::default(),
                },
                Placement {
                    instance_id: 0,
                    frame: 10,
                    target: Target::Asset(1),
                    transform: Transform2D {
                        tx: 100.0,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: PlacementFx {
                        opacity: 0.0,
                        blur: Some(crate::v2::BlurFx { radius: 20.0 }),
                        glow: Some(crate::v2::GlowFx {
                            color: Rgba {
                                r: 255,
                                g: 0,
                                b: 0,
                                a: 255,
                            },
                            radius: 12.0,
                            strength: 2.0,
                        }),
                        ..Default::default()
                    },
                },
            ],
        };

        let start = active_placement_states_at(&layer, 0);
        assert_eq!(start.len(), 1);
        assert!(start[0].fx.is_identity());

        let middle = active_placement_states_at(&layer, 5);
        assert_eq!(middle.len(), 1);
        assert!((middle[0].transform.tx - 50.0).abs() < 1.0e-5);
        assert!((middle[0].fx.opacity - 0.5).abs() < 1.0e-5);
        assert!((middle[0].fx.blur.expect("blur fades in").radius - 10.0).abs() < 1.0e-5);
        assert!((middle[0].fx.glow.expect("glow fades in").strength - 1.0).abs() < 1.0e-5);

        let end = active_placement_states_at(&layer, 10);
        assert_eq!(end.len(), 1);
        assert_eq!(end[0].fx, layer.placements[1].fx);
    }

    #[test]
    fn fx_frame_detection_follows_tweened_filter_state() {
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "fx detection".to_string(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 11,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Art".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![
                        Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::Linear { to_frame: 10 },
                            fx: PlacementFx::default(),
                        },
                        Placement {
                            instance_id: 0,
                            frame: 10,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: PlacementFx {
                                blur: Some(crate::v2::BlurFx { radius: 8.0 }),
                                ..Default::default()
                            },
                        },
                    ],
                }],
            }],
        };

        assert!(!q0rg_frame_has_placement_fx(&project, 1, 0));
        assert!(q0rg_frame_has_placement_fx(&project, 1, 5));
        assert!(q0rg_frame_has_placement_fx(&project, 1, 10));
    }

    #[test]
    fn local_fx_raster_crops_to_the_placement_and_matches_full_frame_pixels() {
        let placement = Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 120.0,
                ty: 90.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                blur: Some(crate::v2::BlurFx { radius: 6.0 }),
                glow: Some(crate::v2::GlowFx {
                    color: Rgba {
                        r: 255,
                        g: 20,
                        b: 10,
                        a: 255,
                    },
                    radius: 8.0,
                    strength: 1.0,
                }),
                ..Default::default()
            },
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "local fx crop".to_string(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![VPath {
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(0.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(30.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(30.0, 20.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(0.0, 20.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 40,
                    g: 120,
                    b: 220,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Art".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![placement.clone()],
                }],
            }],
        };

        let crop = rasterize_placement_fx_scaled_cropped(
            &project,
            1,
            &placement,
            0,
            Affine::from_transform(placement.transform),
            placement.fx,
            400,
            300,
        )
        .expect("visible local fx crop");
        assert!(crop.width < 100 && crop.height < 100);
        assert!(crop.width < project.meta.stage_width.into());
        assert!(crop.height < project.meta.stage_height.into());

        let mut reconstructed = vec![0u8; 400 * 300 * 4];
        write_rgba_rect(
            &mut reconstructed,
            400,
            crop.x,
            crop.y,
            crop.x + crop.width,
            crop.y + crop.height,
            &crop.rgba,
        );
        let full = rasterize_q0rg_frame(&project, 1, 0, 400, 300, 1, [0, 0, 0, 0]);
        assert_eq!(reconstructed, full);
    }

    #[test]
    fn outer_fx_raster_contains_shadow_but_not_the_vector_body() {
        let placement = Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 80.0,
                ty: 70.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                shadow: Some(crate::v2::DropShadowFx {
                    color: Rgba {
                        r: 240,
                        g: 20,
                        b: 10,
                        a: 255,
                    },
                    blur_radius: 0.0,
                    offset_x: 20.0,
                    offset_y: 0.0,
                    strength: 1.0,
                }),
                ..Default::default()
            },
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "outer fx only".to_string(),
                fps: 24,
                stage_width: 200,
                stage_height: 150,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![rectangle_path(0.0, 0.0, 20.0, 20.0)],
                fill: Some(Rgba {
                    r: 10,
                    g: 220,
                    b: 40,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Art".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![placement.clone()],
                }],
            }],
        };

        let crop = rasterize_placement_outer_fx_scaled_cropped(
            &project,
            1,
            &placement,
            0,
            Affine::from_transform(placement.transform),
            placement.fx,
            200,
            150,
        )
        .expect("shadow-only crop");
        let mut reconstructed = vec![0u8; 200 * 150 * 4];
        write_rgba_rect(
            &mut reconstructed,
            200,
            crop.x,
            crop.y,
            crop.x + crop.width,
            crop.y + crop.height,
            &crop.rgba,
        );
        let pixel = |x: usize, y: usize| -> &[u8] {
            let start = (y * 200 + x) * 4;
            &reconstructed[start..start + 4]
        };
        assert_eq!(
            pixel(90, 80)[3],
            0,
            "outer-fx tile must not contain the original vector/q0rg body",
        );
        assert!(pixel(110, 80)[3] > 200, "offset shadow is missing");
        assert!(
            pixel(110, 80)[0] > pixel(110, 80)[1],
            "shadow colour was not preserved",
        );
    }
    #[test]
    fn placement_opacity_is_group_alpha_not_geometry_mutation() {
        let mut destination = vec![0u8; 4];
        let source = vec![240, 80, 20, 255];
        apply_placement_fx(
            &mut destination,
            &source,
            1,
            1,
            PlacementFx {
                opacity: 0.5,
                ..Default::default()
            },
            1.0,
        );
        assert_eq!(&destination[..3], &[240, 80, 20]);
        assert!(destination[3].abs_diff(128) <= 1);
    }

    #[test]
    fn placement_blur_spreads_alpha_without_black_fringes() {
        let width = 9u32;
        let height = width;
        let mut source = vec![0u8; width as usize * height as usize * 4];
        let center = ((4 * width + 4) * 4) as usize;
        source[center..center + 4].copy_from_slice(&[255, 40, 20, 255]);
        let mut destination = vec![0u8; source.len()];
        apply_placement_fx(
            &mut destination,
            &source,
            width,
            height,
            PlacementFx {
                blur: Some(crate::v2::BlurFx { radius: 4.0 }),
                ..Default::default()
            },
            1.0,
        );
        let neighbor = ((4 * width + 2) * 4) as usize;
        assert!(
            destination[neighbor + 3] > 0,
            "blur must spread alpha outside the source pixel"
        );
        assert!(
            destination[neighbor] > 180,
            "alpha-aware blur must retain the red source colour instead of making a dark fringe"
        );
        assert!(
            destination[center + 3] < 255,
            "blur must soften the original center alpha"
        );
    }

    #[test]
    fn glow_and_shadow_are_generated_from_the_whole_placement_alpha() {
        let width = 11u32;
        let height = 11u32;
        let mut source = vec![0u8; width as usize * height as usize * 4];
        let center = ((5 * width + 5) * 4) as usize;
        source[center..center + 4].copy_from_slice(&[255, 255, 255, 255]);
        let mut destination = vec![0u8; source.len()];
        apply_placement_fx(
            &mut destination,
            &source,
            width,
            height,
            PlacementFx {
                glow: Some(crate::v2::GlowFx {
                    color: Rgba {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    radius: 7.0,
                    strength: 1.0,
                }),
                shadow: Some(crate::v2::DropShadowFx {
                    color: Rgba {
                        r: 0,
                        g: 0,
                        b: 255,
                        a: 255,
                    },
                    blur_radius: 0.0,
                    offset_x: 3.0,
                    offset_y: 0.0,
                    strength: 1.0,
                }),
                ..Default::default()
            },
            1.0,
        );
        let glow_pixel = ((5 * width + 3) * 4) as usize;
        assert!(destination[glow_pixel + 3] > 0);
        assert!(destination[glow_pixel] > destination[glow_pixel + 2]);
        let shadow_pixel = ((5 * width + 8) * 4) as usize;
        assert!(
            destination[shadow_pixel + 2] > 0,
            "offset shadow must appear away from the source body"
        );
        assert_eq!(
            destination[center + 3],
            255,
            "effects must not punch holes in the body"
        );
    }

    #[test]
    fn placement_blend_modes_match_reference_channel_math() {
        assert_eq!(blend_channel(BlendMode::Normal, 200, 100), 100);
        assert_eq!(blend_channel(BlendMode::Multiply, 200, 100), 78);
        assert_eq!(blend_channel(BlendMode::Screen, 200, 100), 222);
        assert_eq!(blend_channel(BlendMode::Add, 200, 100), 255);
        assert_eq!(blend_channel(BlendMode::Overlay, 200, 100), 188);
        assert_eq!(blend_channel(BlendMode::Overlay, 60, 180), 85);
    }

    #[test]
    fn multiply_blend_uses_the_existing_backdrop() {
        let mut destination = vec![200, 100, 50, 255];
        let source = vec![128, 128, 128, 255];
        apply_placement_fx(
            &mut destination,
            &source,
            1,
            1,
            PlacementFx {
                blend_mode: BlendMode::Multiply,
                ..Default::default()
            },
            1.0,
        );
        assert!(destination[0].abs_diff(100) <= 1);
        assert!(destination[1].abs_diff(50) <= 1);
        assert!(destination[2].abs_diff(25) <= 1);
        assert_eq!(destination[3], 255);
    }

    fn placement(frame: u16, asset: u16, tx: f32, tween: Tween) -> Placement {
        Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(asset),
            transform: Transform2D {
                tx,
                ..Transform2D::IDENTITY
            },
            tween,
            fx: Default::default(),
        }
    }

    #[test]
    fn raster_resolver_uses_complete_layer_keyframes() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                placement(0, 1, 0.0, Tween::None),
                placement(0, 2, 0.0, Tween::None),
                placement(4, 3, 0.0, Tween::None),
            ],
        };
        assert_eq!(
            active_placements_at(&layer, 3)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            active_placements_at(&layer, 4)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn raster_resolver_stops_at_blank_keyframe() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: vec![2],
            placements: vec![
                placement(0, 1, 0.0, Tween::None),
                placement(5, 2, 0.0, Tween::None),
            ],
        };

        assert_eq!(active_placements_at(&layer, 1).len(), 1);
        assert!(active_placements_at(&layer, 2).is_empty());
        assert!(active_placements_at(&layer, 4).is_empty());
        assert_eq!(active_placements_at(&layer, 5).len(), 1);
    }

    #[test]
    fn raster_resolver_keeps_duplicate_instances_and_tweens_by_order() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                placement(0, 1, 0.0, Tween::Linear { to_frame: 10 }),
                placement(0, 1, 100.0, Tween::Linear { to_frame: 10 }),
                placement(10, 1, 10.0, Tween::None),
                placement(10, 1, 200.0, Tween::None),
            ],
        };
        let active = active_placements_at(&layer, 5);
        assert_eq!(active.len(), 2);
        assert!((active[0].1.tx - 5.0).abs() < 1.0e-5);
        assert!((active[1].1.tx - 150.0).abs() < 1.0e-5);
    }

    #[test]
    fn transparent_source_over_keeps_straight_rgb() {
        let mut pixel = vec![0, 0, 0, 0];
        blend_pixel(
            &mut pixel,
            0,
            Rgba {
                r: 255,
                g: 40,
                b: 10,
                a: 128,
            },
        );
        assert_eq!(pixel, vec![255, 40, 10, 128]);
    }

    #[test]
    fn transparent_supersampling_does_not_create_dark_fringes() {
        let src = vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let downscaled = downscale_box(&src, 2, 1, 1, 2);
        assert_eq!(&downscaled[..3], &[255, 0, 0]);
        assert_eq!(downscaled[3], 64);
    }

    fn rectangle_path(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> VPath {
        VPath {
            anchors: [
                (min_x, min_y),
                (max_x, min_y),
                (max_x, max_y),
                (min_x, max_y),
            ]
            .into_iter()
            .map(|(x, y)| Anchor {
                point: Vec2::new(x, y),
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        }
    }

    #[test]
    fn bucketed_scanline_fill_matches_reference_pixels() {
        let outer = vec![
            Vec2::new(-4.2, 2.0),
            Vec2::new(25.7, 1.5),
            Vec2::new(29.2, 13.2),
            Vec2::new(17.1, 27.9),
            Vec2::new(3.2, 22.4),
        ];
        let mut hole = vec![
            Vec2::new(8.0, 7.0),
            Vec2::new(18.0, 8.0),
            Vec2::new(17.0, 17.0),
            Vec2::new(7.0, 16.0),
        ];
        hole.reverse();
        let island = vec![
            Vec2::new(10.0, 10.0),
            Vec2::new(14.0, 10.0),
            Vec2::new(14.0, 14.0),
            Vec2::new(10.0, 14.0),
        ];
        let polygons = vec![outer, hole, island];
        let color = Rgba {
            r: 17,
            g: 91,
            b: 203,
            a: 173,
        };
        let mut expected = vec![0u8; 32 * 32 * 4];
        let mut actual = expected.clone();
        scanline_fill_multi_reference(&polygons, color, &mut expected, 32, 32);
        scanline_fill_multi(&polygons, color, &mut actual, 32, 32);
        assert_eq!(actual, expected);
    }
    fn appearance_pixel(tile: &RasterizedVectorAppearance, local: Vec2) -> [u8; 4] {
        let x = ((local.x - tile.local_min.x) * tile.pixels_per_unit)
            .floor()
            .clamp(0.0, tile.width.saturating_sub(1) as f32) as u32;
        let y = ((local.y - tile.local_min.y) * tile.pixels_per_unit)
            .floor()
            .clamp(0.0, tile.height.saturating_sub(1) as f32) as u32;
        let index = ((y * tile.width + x) * 4) as usize;
        [
            tile.rgba[index],
            tile.rgba[index + 1],
            tile.rgba[index + 2],
            tile.rgba[index + 3],
        ]
    }

    #[test]
    fn soft_halo_is_a_continuous_alpha_gradient_not_polygon_bands() {
        let fill = Rgba {
            r: 210,
            g: 70,
            b: 25,
            a: 255,
        };
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![rectangle_path(0.0, 0.0, 20.0, 20.0)],
            fill: Some(fill),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.7,
            },
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let tile =
            rasterize_vector_appearance_local(&vector, &appearance, 4.0).expect("appearance tile");

        let mut outside_levels = std::collections::BTreeSet::new();
        for step in 1..=44 {
            let x = -(step as f32) * 0.25;
            let alpha = appearance_pixel(&tile, Vec2::new(x, 10.0))[3];
            if alpha > 0 && alpha < 255 {
                outside_levels.insert(alpha);
            }
        }
        assert!(
            outside_levels.len() > 16,
            "gaussian halo must expose many alpha levels, got {outside_levels:?}"
        );
        let edge = appearance_pixel(&tile, Vec2::new(-1.0, 10.0));
        assert!(edge[3] > 0 && edge[3] < 255);
        for pixel in tile.rgba.chunks_exact(4).filter(|pixel| pixel[3] > 0) {
            assert_eq!(&pixel[..3], &[fill.r, fill.g, fill.b]);
        }
    }

    #[test]
    fn halo_only_raster_is_continuous_under_the_vector_body() {
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![rectangle_path(0.0, 0.0, 20.0, 20.0)],
            fill: Some(Rgba {
                r: 240,
                g: 80,
                b: 30,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: VectorMaterial::SoftHalo {
                radius: 10.0,
                opacity: 0.7,
            },
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let tile = rasterize_vector_halo_local(&vector, &appearance, 4.0).expect("halo tile");
        assert!(
            appearance_pixel(&tile, Vec2::new(10.0, 10.0))[3] > 0,
            "halo underlay must stay continuous beneath the vector body so AA cannot expose square gaps"
        );
        assert!(
            appearance_pixel(&tile, Vec2::new(-2.0, 10.0))[3] > 0,
            "soft material must remain visible outside the source fill"
        );
        let border_is_clear = (0..tile.width).all(|x| {
            let top = ((x) * 4 + 3) as usize;
            let bottom = (((tile.height - 1) * tile.width + x) * 4 + 3) as usize;
            tile.rgba[top] == 0 && tile.rgba[bottom] == 0
        }) && (0..tile.height).all(|y| {
            let left = ((y * tile.width) * 4 + 3) as usize;
            let right = ((y * tile.width + tile.width - 1) * 4 + 3) as usize;
            tile.rgba[left] == 0 && tile.rgba[right] == 0
        });
        assert!(
            border_is_clear,
            "halo texture needs transparent overscan around its finite support"
        );
    }

    #[test]
    fn appearance_field_affine_transforms_resolved_halo_instead_of_reblurring_skewed_source() {
        fn map_path(path: &VPath, transform: Affine) -> VPath {
            let mut mapped = path.clone();
            for anchor in &mut mapped.anchors {
                anchor.point = transform.apply(anchor.point);
                anchor.in_handle = anchor.in_handle.map(|point| transform.apply(point));
                anchor.out_handle = anchor.out_handle.map(|point| transform.apply(point));
            }
            mapped
        }

        let source_path = rectangle_path(0.0, 0.0, 8.0, 28.0);
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![source_path.clone()],
            fill: Some(Rgba {
                r: 220,
                g: 40,
                b: 30,
                a: 255,
            }),
            stroke: None,
        };
        let base = VectorAppearance {
            material: VectorMaterial::SoftHalo {
                radius: 7.0,
                opacity: 0.7,
            },
            erase_mask: Vec::new(),
            material_source: vec![source_path.clone()],
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
        };
        let shear = Affine {
            a11: 1.0,
            a12: 0.75,
            a21: 0.0,
            a22: 1.0,
            tx: 32.0,
            ty: 18.0,
        };
        let mut transformed_field = base.clone();
        transformed_field.field_transform = shear;

        let base_tile = rasterize_vector_halo_local(&vector, &base, 4.0).unwrap();
        let transformed_tile =
            rasterize_vector_halo_local(&vector, &transformed_field, 4.0).unwrap();
        assert_eq!(base_tile.width, transformed_tile.width);
        assert_eq!(base_tile.height, transformed_tile.height);
        assert_eq!(base_tile.local_min, transformed_tile.local_min);
        assert_eq!(
            base_tile.rgba, transformed_tile.rgba,
            "field affine must not be baked by regenerating a differently shaped gaussian"
        );

        let wrong_reblur = VectorAppearance {
            field_transform: Affine::IDENTITY,
            material_source: vec![map_path(&source_path, shear)],
            ..base.clone()
        };
        let wrong_tile = rasterize_vector_halo_local(&vector, &wrong_reblur, 4.0).unwrap();
        assert!(
            wrong_tile.width != base_tile.width
                || wrong_tile.height != base_tile.height
                || wrong_tile.rgba != base_tile.rgba,
            "skewing the source then re-running gaussian must remain distinguishable from transforming the resolved field"
        );

        let mut rendered = vec![0u8; 128 * 96 * 4];
        composite_appearance_tile(&base_tile, shear, &mut rendered, 128, 96);
        let mut min_x = u32::MAX;
        let mut max_x = 0u32;
        let mut min_y = u32::MAX;
        let mut max_y = 0u32;
        for y in 0..96u32 {
            for x in 0..128u32 {
                if rendered[((y * 128 + x) * 4 + 3) as usize] > 0 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(min_x < max_x && min_y < max_y, "affine halo must render");
        assert!(
            max_x - min_x > max_y - min_y,
            "horizontal shear must visibly skew the already-resolved tall halo: x={min_x}..{max_x}, y={min_y}..{max_y}"
        );
    }

    #[test]
    fn post_material_fragment_split_partitions_one_resolved_glow_field() {
        let fill = Rgba {
            r: 210,
            g: 40,
            b: 30,
            a: 255,
        };
        let original_path = rectangle_path(0.0, 0.0, 20.0, 20.0);
        let original = VectorAsset {
            asset_id: 1,
            paths: vec![original_path.clone()],
            fill: Some(fill),
            stroke: None,
        };
        let left = VectorAsset {
            asset_id: 2,
            paths: vec![rectangle_path(0.0, 0.0, 10.0, 20.0)],
            fill: Some(fill),
            stroke: None,
        };
        let right = VectorAsset {
            asset_id: 3,
            paths: vec![rectangle_path(10.0, 0.0, 20.0, 20.0)],
            fill: Some(fill),
            stroke: None,
        };
        let material = VectorMaterial::SoftHalo {
            radius: 8.0,
            opacity: 0.65,
        };
        let whole = VectorAppearance {
            material,
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let left_appearance = VectorAppearance {
            material,
            erase_mask: Vec::new(),
            material_source: vec![original_path.clone()],
            clip_mask: vec![rectangle_path(-20.0, -20.0, 10.0, 40.0)],
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let right_appearance = VectorAppearance {
            material,
            erase_mask: Vec::new(),
            material_source: vec![original_path],
            clip_mask: vec![rectangle_path(10.0, -20.0, 40.0, 40.0)],
            field_transform: crate::transform::Affine::IDENTITY,
        };

        let whole_tile = rasterize_vector_appearance_local(&original, &whole, 4.0).unwrap();
        let left_tile = rasterize_vector_appearance_local(&left, &left_appearance, 4.0).unwrap();
        let right_tile = rasterize_vector_appearance_local(&right, &right_appearance, 4.0).unwrap();
        let sample_or_clear = |tile: &RasterizedVectorAppearance, local: Vec2| -> u8 {
            let px = (local.x - tile.local_min.x) * tile.pixels_per_unit;
            let py = (local.y - tile.local_min.y) * tile.pixels_per_unit;
            if px < 0.0 || py < 0.0 || px >= tile.width as f32 || py >= tile.height as f32 {
                0
            } else {
                appearance_pixel(tile, local)[3]
            }
        };
        for y in 0..whole_tile.height {
            for x in 0..whole_tile.width {
                let local = Vec2::new(
                    whole_tile.local_min.x + (x as f32 + 0.5) / whole_tile.pixels_per_unit,
                    whole_tile.local_min.y + (y as f32 + 0.5) / whole_tile.pixels_per_unit,
                );
                let whole_alpha = appearance_pixel(&whole_tile, local)[3];
                let split_alpha =
                    sample_or_clear(&left_tile, local).max(sample_or_clear(&right_tile, local));
                assert!(
                    whole_alpha.abs_diff(split_alpha) <= 1,
                    "post-material split changed resolved alpha at ({x},{y}): whole={whole_alpha}, split={split_alpha}"
                );
            }
        }

        let right_halo = rasterize_vector_halo_local(&right, &right_appearance, 4.0).unwrap();
        assert_eq!(
            appearance_pixel(&right_halo, Vec2::new(9.0, 10.0))[3],
            0,
            "split edge must be a post-filter clip, not a fresh glow source"
        );
        assert!(
            appearance_pixel(&right_halo, Vec2::new(22.0, 10.0))[3] > 0,
            "the original external edge must retain its halo"
        );
    }

    #[test]
    fn post_material_clip_limits_raster_work_to_the_fragment_neighbourhood() {
        let fill = Rgba {
            r: 200,
            g: 30,
            b: 20,
            a: 255,
        };
        let source_path = rectangle_path(0.0, 0.0, 300.0, 300.0);
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![source_path.clone()],
            fill: Some(fill),
            stroke: None,
        };
        let whole = VectorAppearance {
            material: VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.6,
            },
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let fragment = VectorAppearance {
            material: whole.material,
            erase_mask: Vec::new(),
            material_source: vec![source_path],
            clip_mask: vec![rectangle_path(-10.0, 120.0, 20.0, 180.0)],
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let whole_tile = rasterize_vector_halo_local(&vector, &whole, 4.0).unwrap();
        let fragment_tile = rasterize_vector_halo_local(&vector, &fragment, 4.0).unwrap();
        assert!(
            fragment_tile.width < whole_tile.width / 4,
            "tiny post-material clip must not allocate the full source width: fragment={} whole={}",
            fragment_tile.width,
            whole_tile.width,
        );
        assert!(
            fragment_tile.height < whole_tile.height / 2,
            "fragment raster height should be bounded around its clip: fragment={} whole={}",
            fragment_tile.height,
            whole_tile.height,
        );
        assert!(appearance_pixel(&fragment_tile, Vec2::new(-5.0, 150.0))[3] > 0);
        assert_eq!(
            appearance_pixel(&fragment_tile, Vec2::new(100.0, 150.0))[3],
            0
        );
    }

    #[test]
    fn appearance_mask_is_applied_after_blur_so_halo_cannot_grow_back() {
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![rectangle_path(0.0, 0.0, 20.0, 20.0)],
            fill: Some(Rgba {
                r: 255,
                g: 80,
                b: 40,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.8,
            },
            erase_mask: vec![rectangle_path(22.0, 7.0, 26.0, 13.0)],
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: crate::transform::Affine::IDENTITY,
        };
        let tile =
            rasterize_vector_appearance_local(&vector, &appearance, 4.0).expect("appearance tile");
        assert_eq!(appearance_pixel(&tile, Vec2::new(24.0, 10.0))[3], 0);
        assert!(
            appearance_pixel(&tile, Vec2::new(21.0, 10.0))[3] > 0,
            "unmasked neighbouring halo must remain visible"
        );
    }

    #[test]
    fn nested_transformed_q0rg_keeps_appearance_and_erase_mask() {
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![rectangle_path(0.0, 0.0, 10.0, 10.0)],
            fill: Some(Rgba {
                r: 230,
                g: 30,
                b: 20,
                a: 255,
            }),
            stroke: None,
        };
        let mut appearances = std::collections::HashMap::new();
        appearances.insert(
            1,
            VectorAppearance {
                material: VectorMaterial::SoftHalo {
                    radius: 6.0,
                    opacity: 0.8,
                },
                erase_mask: vec![rectangle_path(3.0, 3.0, 7.0, 7.0)],
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: crate::transform::Affine::IDENTITY,
            },
        );
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "nested appearance".into(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(vector)],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: appearances,
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "stage".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "instance".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D {
                                tx: 20.0,
                                ty: 15.0,
                                sx: 2.0,
                                sy: 2.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "symbol".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "art".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
            ],
        };

        let frame = rasterize_q0rg_frame(&project, 1, 0, 64, 64, 1, [0, 0, 0, 0]);
        let alpha = |x: u32, y: u32| frame[((y * 64 + x) * 4 + 3) as usize];
        assert_eq!(
            alpha(30, 25),
            0,
            "transformed mask centre must remain erased"
        );
        assert!(alpha(24, 25) > 200, "transformed source must remain opaque");
        assert!(
            alpha(18, 25) > 0,
            "transformed halo must survive q0rg nesting"
        );
    }

    #[test]
    fn scaled_offscreen_render_maps_the_whole_stage_to_output() {
        use crate::v2::{BitmapAsset, ProjectMeta, Q0rg};

        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "scaled".into(),
                fps: 24,
                stage_width: 2,
                stage_height: 2,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: [255, 0, 0, 255].repeat(4),
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![placement(0, 1, 0.0, Tween::None)],
                }],
            }],
        };

        let frame = rasterize_q0rg_frame_scaled(&project, 1, 0, 4, 4, 1, [0, 0, 0, 0]);
        assert_eq!(frame.len(), 4 * 4 * 4);
        assert_eq!(&frame[((3 * 4 + 3) * 4)..][..4], &[255, 0, 0, 255]);
    }
    #[test]
    fn hidden_layer_is_omitted_from_runtime_raster_output() {
        use crate::v2::{BitmapAsset, LayerKey, LayerMetadata, ProjectMeta, Q0rg};

        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "hidden layer".into(),
                fps: 24,
                stage_width: 2,
                stage_height: 2,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: [255, 0, 0, 255].repeat(4),
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![placement(0, 1, 0.0, Tween::None)],
                }],
            }],
        };

        let visible = rasterize_q0rg_frame(&project, 1, 0, 2, 2, 1, [0, 0, 0, 0]);
        assert_eq!(&visible[..4], &[255, 0, 0, 255]);

        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                hidden: true,
                ..Default::default()
            },
        );
        let hidden = rasterize_q0rg_frame(&project, 1, 0, 2, 2, 1, [0, 0, 0, 0]);
        assert!(hidden.chunks_exact(4).all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn software_raster_uses_shared_rig_pose_for_bound_display_object() {
        let vector = Asset::Vector(crate::v2::VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                closed: true,
                anchors: vec![
                    Anchor {
                        point: Vec2::new(0.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(5.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(5.0, 5.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(0.0, 5.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
            }],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        });
        let rig = Asset::Rig(crate::v2::RigAsset {
            asset_id: 2,
            owner_q0rg_id: 1,
            nodes: vec![crate::v2::RigNode {
                node_id: 1,
                name: "root".into(),
                parent: None,
                rest: Transform2D::IDENTITY,
                length: 5.0,
                binding: Some(crate::v2::RigBinding {
                    instance_id: 1,
                    bind_offset: Affine::IDENTITY,
                }),
            }],
            controls: vec![crate::v2::RigControl {
                control_id: 1,
                name: "move".into(),
                kind: crate::v2::RigControlKind::Position2D,
                target_node: Some(1),
                rest_x: 2.0,
                rest_y: 2.0,
                rest_value: 0.0,
                min_value: -100.0,
                max_value: 100.0,
                public_in_simple: true,
            }],
            constraints: Vec::new(),
            channels: vec![
                crate::v2::RigChannel {
                    property: crate::v2::RigPropertyRef::ControlX(1),
                    keys: vec![
                        crate::v2::RigKey {
                            frame: 0,
                            value: 2.0,
                            easing: crate::v2::Easing::Linear,
                        },
                        crate::v2::RigKey {
                            frame: 1,
                            value: 20.0,
                            easing: crate::v2::Easing::Linear,
                        },
                    ],
                },
                crate::v2::RigChannel {
                    property: crate::v2::RigPropertyRef::ControlY(1),
                    keys: vec![crate::v2::RigKey {
                        frame: 0,
                        value: 2.0,
                        easing: crate::v2::Easing::Linear,
                    }],
                },
            ],
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            pose_drivers: Vec::new(),
            mirror_pairs: Vec::new(),
            variants: Vec::new(),
        });
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "rig raster".into(),
                fps: 24,
                stage_width: 32,
                stage_height: 16,
                entry_q0rg_id: 1,
            },
            assets: vec![vector, rig],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 2,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![Placement {
                        instance_id: 1,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        crate::v2::validate(&project).expect("rigged raster fixture validates");

        let bounds = |pixels: &[u8]| {
            let mut xs = Vec::new();
            for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                if pixel[3] != 0 {
                    xs.push((index % 32) as u32);
                }
            }
            (*xs.iter().min().unwrap(), *xs.iter().max().unwrap())
        };
        let frame0 = rasterize_q0rg_frame(&project, 1, 0, 32, 16, 1, [0, 0, 0, 0]);
        let frame1 = rasterize_q0rg_frame(&project, 1, 1, 32, 16, 1, [0, 0, 0, 0]);
        let b0 = bounds(&frame0);
        let b1 = bounds(&frame1);
        assert!(b0.0 <= 3 && b0.1 < 10, "frame0 bounds={b0:?}");
        assert!(b1.0 >= 19 && b1.1 >= 24, "frame1 bounds={b1:?}");
        assert_ne!(frame0, frame1, "rig channels must move rendered pixels");

        // Reordering two placements with the same target must not change which
        // logical display object the rig drives. Persistent binding is by
        // instance_id, never by target occurrence.
        let mut identity_project = project.clone();
        identity_project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 2,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D {
                    tx: 10.0,
                    ty: 2.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
            });
        crate::v2::validate(&identity_project).expect("two-instance rig fixture validates");
        let before_reorder = rasterize_q0rg_frame(&identity_project, 1, 1, 32, 16, 1, [0, 0, 0, 0]);
        identity_project.q0rgs[0].layers[0].placements.swap(0, 1);
        crate::v2::validate(&identity_project).expect("reordered fixture validates");
        let after_reorder = rasterize_q0rg_frame(&identity_project, 1, 1, 32, 16, 1, [0, 0, 0, 0]);
        assert_eq!(
            before_reorder, after_reorder,
            "same-target reorder rebound the rig"
        );

        // FX are applied after the resolved rig transform. The glow therefore
        // has to travel with the bound object instead of staying at its rest pose.
        let mut fx_project = project.clone();
        fx_project.q0rgs[0].layers[0].placements[0].fx = crate::v2::PlacementFx {
            glow: Some(crate::v2::GlowFx {
                color: Rgba {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                radius: 2.0,
                strength: 1.0,
            }),
            ..Default::default()
        };
        let fx0 = rasterize_q0rg_frame(&fx_project, 1, 0, 32, 16, 1, [0, 0, 0, 0]);
        let fx1 = rasterize_q0rg_frame(&fx_project, 1, 1, 32, 16, 1, [0, 0, 0, 0]);
        let fx_b0 = bounds(&fx0);
        let fx_b1 = bounds(&fx1);
        assert!(fx_b0.1 < 12, "frame0 glow drifted: {fx_b0:?}");
        assert!(
            fx_b1.0 >= 16 && fx_b1.1 >= 24,
            "frame1 glow did not follow rig: {fx_b1:?}"
        );

        // The same rig must compose under a parent q0rg transform rather than
        // treating its solved world transform as stage-global.
        let mut nested = project.clone();
        if let Asset::Rig(rig) = &mut nested.assets[1] {
            rig.owner_q0rg_id = 2;
        }
        nested.q0rgs[0].q0rg_id = 2;
        nested.q0rgs[0].name = "rigged child".into();
        nested.q0rgs.insert(
            0,
            Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 2,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "child instance".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Q0rg(2),
                        transform: Transform2D {
                            tx: 5.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            },
        );
        nested.meta.entry_q0rg_id = 1;
        crate::v2::validate(&nested).expect("nested rig fixture validates");
        let nested_frame = rasterize_q0rg_frame(&nested, 1, 1, 32, 16, 1, [0, 0, 0, 0]);
        let nested_bounds = bounds(&nested_frame);
        assert!(
            nested_bounds.0 >= 24 && nested_bounds.1 >= 29,
            "parent transform did not compose with child rig: {nested_bounds:?}"
        );
    }
}
