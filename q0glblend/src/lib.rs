use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Weak};

use egui::{pos2, Color32, Mesh, PaintCallback, Painter, Pos2, Rect, Shape, TextureId};
use glow::HasContext as _;
use q0s_format::v2::{BlendMode, GradientKind, GradientStop};

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static GL_RESOURCES: RefCell<HashMap<usize, GlResources>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintDiagnostics {
    pub callbacks: u64,
    pub filtered_callbacks: u64,
    pub blur_passes: u64,
    pub glow_passes: u64,
    /// Sum of visible callback region area times the number of full-screen
    /// filter passes, in logical point-pixels. This is a workload estimate,
    /// not a GPU timer.
    pub filter_pixel_passes: u64,
    pub max_filter_region_pixels: u64,
}

/// Asynchronous timing results for filtered vector-group paint callbacks.
/// GPU samples come from OpenGL timer queries and can arrive a frame later.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuCallbackTiming {
    pub timer_queries_initialized: bool,
    pub timer_queries_supported: bool,
    pub gpu_samples: u64,
    pub gpu_total_ns: u64,
    pub gpu_max_ns: u64,
    pub gpu_dropped_samples: u64,
    pub cpu_samples: u64,
    pub cpu_total_ns: u64,
    pub cpu_max_ns: u64,
}

/// Timer queries sample one of every sixteen filtered vector-group callbacks.
pub const GPU_FILTER_SAMPLE_STRIDE: u64 = 16;

thread_local! {
    static PAINT_DIAGNOSTICS: RefCell<PaintDiagnostics> = RefCell::new(PaintDiagnostics::default());
    static GPU_CALLBACK_TIMING: RefCell<GpuCallbackTiming> = RefCell::new(GpuCallbackTiming::default());
    static GPU_TIMER_QUERY_SUPPORT: Cell<Option<bool>> = const { Cell::new(None) };
}

pub fn reset_paint_diagnostics() {
    PAINT_DIAGNOSTICS.with(|diagnostics| *diagnostics.borrow_mut() = PaintDiagnostics::default());
}

pub fn take_paint_diagnostics() -> PaintDiagnostics {
    PAINT_DIAGNOSTICS.with(|diagnostics| std::mem::take(&mut *diagnostics.borrow_mut()))
}

/// Take timer results produced by previously executed paint callbacks.
/// The query support flag remains known after the counters are drained.
pub fn take_gpu_callback_timing() -> GpuCallbackTiming {
    let mut timing =
        GPU_CALLBACK_TIMING.with(|diagnostics| std::mem::take(&mut *diagnostics.borrow_mut()));
    GPU_TIMER_QUERY_SUPPORT.with(|support| {
        if let Some(supported) = support.get() {
            timing.timer_queries_initialized = true;
            timing.timer_queries_supported = supported;
        }
    });
    timing
}

fn set_gpu_timer_query_support(supported: bool) {
    GPU_TIMER_QUERY_SUPPORT.with(|support| support.set(Some(supported)));
}

fn record_gpu_filter_sample(ns: u64) {
    GPU_CALLBACK_TIMING.with(|diagnostics| {
        let mut diagnostics = diagnostics.borrow_mut();
        diagnostics.gpu_samples = diagnostics.gpu_samples.saturating_add(1);
        diagnostics.gpu_total_ns = diagnostics.gpu_total_ns.saturating_add(ns);
        diagnostics.gpu_max_ns = diagnostics.gpu_max_ns.max(ns);
    });
}

fn record_gpu_filter_sample_drop() {
    GPU_CALLBACK_TIMING.with(|diagnostics| {
        let mut diagnostics = diagnostics.borrow_mut();
        diagnostics.gpu_dropped_samples = diagnostics.gpu_dropped_samples.saturating_add(1);
    });
}

fn record_gpu_filter_callback_cpu(ns: u64) {
    GPU_CALLBACK_TIMING.with(|diagnostics| {
        let mut diagnostics = diagnostics.borrow_mut();
        diagnostics.cpu_samples = diagnostics.cpu_samples.saturating_add(1);
        diagnostics.cpu_total_ns = diagnostics.cpu_total_ns.saturating_add(ns);
        diagnostics.cpu_max_ns = diagnostics.cpu_max_ns.max(ns);
    });
}

fn record_paint_callback(painter: &Painter, rect: Rect, fx: Option<VectorGroupFx>) {
    let Some(fx) = fx else {
        PAINT_DIAGNOSTICS.with(|diagnostics| {
            let mut diagnostics = diagnostics.borrow_mut();
            diagnostics.callbacks = diagnostics.callbacks.saturating_add(1);
        });
        return;
    };

    let blur_passes = if fx.has_blur() {
        u64::from(fx.blur_quality.clamp(1, 15)).saturating_mul(2)
    } else {
        0
    };
    let glow_passes = if fx.has_glow() {
        u64::from(fx.glow_quality.clamp(1, 15)).saturating_mul(2)
    } else {
        0
    };
    let filter_passes = blur_passes.saturating_add(glow_passes);
    let clipped = rect.intersect(painter.clip_rect());
    let region_pixels = if clipped.is_positive() {
        (clipped.width().ceil().max(0.0) as u64)
            .saturating_mul(clipped.height().ceil().max(0.0) as u64)
    } else {
        0
    };
    PAINT_DIAGNOSTICS.with(|diagnostics| {
        let mut diagnostics = diagnostics.borrow_mut();
        diagnostics.callbacks = diagnostics.callbacks.saturating_add(1);
        if filter_passes > 0 {
            diagnostics.filtered_callbacks = diagnostics.filtered_callbacks.saturating_add(1);
            diagnostics.blur_passes = diagnostics.blur_passes.saturating_add(blur_passes);
            diagnostics.glow_passes = diagnostics.glow_passes.saturating_add(glow_passes);
            diagnostics.filter_pixel_passes = diagnostics
                .filter_pixel_passes
                .saturating_add(region_pixels.saturating_mul(filter_passes));
            diagnostics.max_filter_region_pixels =
                diagnostics.max_filter_region_pixels.max(region_pixels);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct CallbackRegion {
    left: i32,
    bottom: i32,
    width: i32,
    height: i32,
    source_uv_min: [f32; 2],
    source_uv_max: [f32; 2],
}

#[cfg(not(target_arch = "wasm32"))]
fn callback_region(
    viewport: Rect,
    clip_rect: Rect,
    pixels_per_point: f32,
    screen_size_px: [u32; 2],
) -> Option<CallbackRegion> {
    let ppp = pixels_per_point.max(f32::EPSILON);
    let to_px = |value: f32| (value * ppp).round() as i32;
    let full_left = to_px(viewport.left());
    let full_top = to_px(viewport.top());
    let full_right = to_px(viewport.right());
    let full_bottom = to_px(viewport.bottom());
    let full_width = full_right - full_left;
    let full_height = full_bottom - full_top;
    if full_width <= 0 || full_height <= 0 {
        return None;
    }
    let screen_width = screen_size_px[0] as i32;
    let screen_height = screen_size_px[1] as i32;
    let left = full_left.max(to_px(clip_rect.left())).max(0);
    let top = full_top.max(to_px(clip_rect.top())).max(0);
    let right = full_right.min(to_px(clip_rect.right())).min(screen_width);
    let bottom_from_top = full_bottom
        .min(to_px(clip_rect.bottom()))
        .min(screen_height);
    let width = right - left;
    let height = bottom_from_top - top;
    if width <= 0 || height <= 0 {
        return None;
    }
    let inv_width = 1.0 / full_width as f32;
    let inv_height = 1.0 / full_height as f32;
    Some(CallbackRegion {
        left,
        bottom: screen_height - bottom_from_top,
        width,
        height,
        source_uv_min: [
            (left - full_left) as f32 * inv_width,
            (top - full_top) as f32 * inv_height,
        ],
        source_uv_max: [
            (right - full_left) as f32 * inv_width,
            (bottom_from_top - full_top) as f32 * inv_height,
        ],
    })
}
#[cfg(not(target_arch = "wasm32"))]
struct CachedSolidMeshBuffers {
    source: Weak<Mesh>,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_count: i32,
}

#[cfg(not(target_arch = "wasm32"))]
struct GpuFilterTimerSlot {
    query: glow::Query,
    pending: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct GpuFilterTimer {
    slots: Vec<GpuFilterTimerSlot>,
    next_write: usize,
    next_poll: usize,
    filtered_callbacks: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl GpuFilterTimer {
    const QUERY_POOL_SIZE: usize = 32;

    unsafe fn new(gl: &glow::Context) -> Self {
        let version = gl.version();
        let core_support = !version.is_embedded
            && (version.major > 3 || (version.major == 3 && version.minor >= 3));
        let extension_support = gl.supported_extensions().contains("GL_ARB_timer_query")
            || gl.supported_extensions().contains("GL_EXT_timer_query");
        if !core_support && !extension_support {
            set_gpu_timer_query_support(false);
            return Self::default();
        }

        let mut slots = Vec::with_capacity(Self::QUERY_POOL_SIZE);
        for _ in 0..Self::QUERY_POOL_SIZE {
            match gl.create_query() {
                Ok(query) => slots.push(GpuFilterTimerSlot {
                    query,
                    pending: false,
                }),
                Err(_) => break,
            }
        }
        set_gpu_timer_query_support(!slots.is_empty());
        Self {
            slots,
            next_write: 0,
            next_poll: 0,
            filtered_callbacks: 0,
        }
    }

    unsafe fn poll_one(&mut self, gl: &glow::Context) {
        if self.slots.is_empty() {
            return;
        }
        for _ in 0..self.slots.len() {
            let index = self.next_poll;
            self.next_poll = (self.next_poll + 1) % self.slots.len();
            let slot = &mut self.slots[index];
            if !slot.pending {
                continue;
            }
            if gl.get_query_parameter_u32(slot.query, glow::QUERY_RESULT_AVAILABLE) != 0 {
                let nanoseconds =
                    u64::from(gl.get_query_parameter_u32(slot.query, glow::QUERY_RESULT));
                slot.pending = false;
                record_gpu_filter_sample(nanoseconds);
            }
            return;
        }
    }

    unsafe fn begin_sample(&mut self, gl: &glow::Context, filtered: bool) -> Option<usize> {
        if !filtered {
            return None;
        }
        self.filtered_callbacks = self.filtered_callbacks.wrapping_add(1);
        if !self
            .filtered_callbacks
            .is_multiple_of(GPU_FILTER_SAMPLE_STRIDE)
            || self.slots.is_empty()
        {
            return None;
        }
        for offset in 0..self.slots.len() {
            let index = (self.next_write + offset) % self.slots.len();
            if self.slots[index].pending {
                continue;
            }
            gl.begin_query(glow::TIME_ELAPSED, self.slots[index].query);
            self.next_write = (index + 1) % self.slots.len();
            return Some(index);
        }
        record_gpu_filter_sample_drop();
        None
    }

    unsafe fn end_sample(&mut self, gl: &glow::Context, index: usize) {
        gl.end_query(glow::TIME_ELAPSED);
        self.slots[index].pending = true;
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct GlResources {
    program: glow::Program,
    vao: glow::VertexArray,
    backdrop_texture: glow::Texture,
    backdrop_fbo: glow::Framebuffer,
    backdrop_size: (i32, i32),
    source_uniform: Option<glow::UniformLocation>,
    backdrop_uniform: Option<glow::UniformLocation>,
    backdrop_uv_max_uniform: Option<glow::UniformLocation>,
    mode_uniform: Option<glow::UniformLocation>,
    passthrough_uniform: Option<glow::UniformLocation>,
    source_srgb_uniform: Option<glow::UniformLocation>,
    source_opacity_uniform: Option<glow::UniformLocation>,
    source_uv_min_uniform: Option<glow::UniformLocation>,
    source_uv_max_uniform: Option<glow::UniformLocation>,
    source_srgb: bool,
    group_program: glow::Program,
    group_vao: glow::VertexArray,
    group_vbo: glow::Buffer,
    group_ebo: glow::Buffer,
    group_texture: glow::Texture,
    group_fbo: glow::Framebuffer,
    group_msaa_fbo: glow::Framebuffer,
    group_msaa_color: glow::Renderbuffer,
    group_size: (i32, i32),
    group_ppp_uniform: Option<glow::UniformLocation>,
    group_origin_uniform: Option<glow::UniformLocation>,
    group_size_uniform: Option<glow::UniformLocation>,
    group_textured_program: glow::Program,
    group_textured_ppp_uniform: Option<glow::UniformLocation>,
    group_textured_origin_uniform: Option<glow::UniformLocation>,
    group_textured_size_uniform: Option<glow::UniformLocation>,
    group_textured_texture_uniform: Option<glow::UniformLocation>,
    group_textured_srgb_uniform: Option<glow::UniformLocation>,
    cached_solid_program: glow::Program,
    cached_solid_vao: glow::VertexArray,
    cached_solid_screen_size_uniform: Option<glow::UniformLocation>,
    cached_solid_source_origin_uniform: Option<glow::UniformLocation>,
    cached_solid_target_origin_uniform: Option<glow::UniformLocation>,
    cached_solid_linear_uniform: Option<glow::UniformLocation>,
    cached_solid_fixed_screen_offset_uniform: Option<glow::UniformLocation>,
    cached_solid_offscreen_uniform: Option<glow::UniformLocation>,
    cached_solid_screen_uv_tile_uniform: Option<glow::UniformLocation>,
    cached_solid_texture_uniform: Option<glow::UniformLocation>,
    cached_solid_srgb_texture_uniform: Option<glow::UniformLocation>,
    cached_solid_buffers: HashMap<usize, CachedSolidMeshBuffers>,
    gradient_program: glow::Program,
    gradient_vao: glow::VertexArray,
    gradient_stop_texture: glow::Texture,
    gradient_screen_size_uniform: Option<glow::UniformLocation>,
    gradient_source_origin_uniform: Option<glow::UniformLocation>,
    gradient_target_origin_uniform: Option<glow::UniformLocation>,
    gradient_linear_uniform: Option<glow::UniformLocation>,
    gradient_inverse_linear_uniform: Option<glow::UniformLocation>,
    gradient_inverse_translation_uniform: Option<glow::UniformLocation>,
    gradient_kind_uniform: Option<glow::UniformLocation>,
    gradient_stop_count_uniform: Option<glow::UniformLocation>,
    gradient_stops_uniform: Option<glow::UniformLocation>,
    gradient_tint_uniform: Option<glow::UniformLocation>,
    gradient_screen_offset_uniform: Option<glow::UniformLocation>,
    gradient_offscreen_uniform: Option<glow::UniformLocation>,
    gradient_buffers: HashMap<usize, CachedSolidMeshBuffers>,
    filter_program: glow::Program,
    filter_source_uniform: Option<glow::UniformLocation>,
    filter_uv_max_uniform: Option<glow::UniformLocation>,
    filter_texel_uniform: Option<glow::UniformLocation>,
    filter_direction_uniform: Option<glow::UniformLocation>,
    filter_radius_uniform: Option<glow::UniformLocation>,
    combine_program: glow::Program,
    combine_body_uniform: Option<glow::UniformLocation>,
    combine_glow_uniform: Option<glow::UniformLocation>,
    combine_uv_max_uniform: Option<glow::UniformLocation>,
    combine_glow_color_uniform: Option<glow::UniformLocation>,
    combine_glow_strength_uniform: Option<glow::UniformLocation>,
    filter_textures: [glow::Texture; 3],
    filter_fbos: [glow::Framebuffer; 3],
    filter_size: (i32, i32),
    gpu_filter_timer: GpuFilterTimer,
}

/// Paint an egui texture with a q0s placement blend mode without rasterising
/// the rest of the stage. `Normal` stays on egui's ordinary textured-mesh path.
/// Other modes use a tiny OpenGL callback that resolves only this rectangle's
/// current framebuffer into a local texture, blends against it, and writes the
/// result back to the same rectangle.
pub fn paint_texture(painter: &Painter, rect: Rect, texture_id: TextureId, blend_mode: BlendMode) {
    paint_texture_impl(painter, rect, texture_id, blend_mode, false);
}

/// The editor stage and Library preview are painted over an opaque paper/background.
/// On an opaque backdrop, Multiply and Screen can use the fixed-function blend
/// unit exactly, avoiding the expensive framebuffer resolve/copy used by the
/// generic compositor.
pub fn paint_texture_on_opaque_backdrop(
    painter: &Painter,
    rect: Rect,
    texture_id: TextureId,
    blend_mode: BlendMode,
) {
    paint_texture_impl(painter, rect, texture_id, blend_mode, true);
}

/// Composite a group of already-tessellated solid egui meshes without first
/// asking q0s-format to CPU-rasterize the q0rg. The group is rasterized only by
/// the GPU into a small 4x-MSAA offscreen target, then blended once as a display
/// object so overlapping children keep group blend semantics.
pub fn paint_solid_mesh_group_on_opaque_backdrop(
    painter: &Painter,
    rect: Rect,
    meshes: Vec<Mesh>,
    blend_mode: BlendMode,
) {
    paint_solid_mesh_group_with_opacity_on_opaque_backdrop(painter, rect, meshes, blend_mode, 1.0);
}

/// GPU-compose one vector mesh group with display-object opacity. Children are
/// first composited at full opacity into the offscreen target, then group alpha
/// is applied once so overlaps do not darken each other.
pub fn paint_solid_mesh_group_with_opacity_on_opaque_backdrop(
    painter: &Painter,
    rect: Rect,
    meshes: Vec<Mesh>,
    blend_mode: BlendMode,
    opacity: f32,
) {
    if meshes.is_empty() || opacity <= 1.0e-6 {
        return;
    }
    if blend_mode == BlendMode::Normal && opacity >= 0.999_999 {
        for mesh in meshes {
            painter.add(Shape::Mesh(mesh));
        }
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        for mesh in meshes {
            painter.add(Shape::Mesh(mesh));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let meshes = Arc::new(meshes);
        let mode = blend_mode_tag(blend_mode);
        let fixed_opaque =
            blend_mode == BlendMode::Normal || uses_opaque_fixed_path(blend_mode, true);
        let opacity = opacity.clamp(0.0, 1.0);
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(region) = callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };
            let gl = glow_painter.gl();
            let context_key = Arc::as_ptr(gl) as usize;
            GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    resources.entry(context_key)
                {
                    let Ok(created) = (unsafe { GlResources::new(gl) }) else {
                        return;
                    };
                    entry.insert(created);
                }
                let Some(resources) = resources.get_mut(&context_key) else {
                    return;
                };
                unsafe {
                    resources.paint_solid_group(
                        gl,
                        meshes.as_slice(),
                        mode,
                        fixed_opaque,
                        opacity,
                        region,
                        info.pixels_per_point,
                        info.screen_size_px,
                    );
                }
            });
        });
        record_paint_callback(painter, rect, None);
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn apply_cached_mesh_linear(vector: egui::Vec2, linear: [f32; 4]) -> egui::Vec2 {
    egui::vec2(
        linear[0] * vector.x + linear[1] * vector.y,
        linear[2] * vector.x + linear[3] * vector.y,
    )
}

#[cfg(any(test, target_arch = "wasm32"))]
fn transform_fixed_screen_offset(offset: egui::Vec2, linear: [f32; 4]) -> egui::Vec2 {
    let length = offset.length();
    if length <= 1.0e-12 {
        return egui::Vec2::ZERO;
    }
    let det = linear[0] * linear[3] - linear[1] * linear[2];
    if det.abs() <= 1.0e-12 || !det.is_finite() {
        let fallback = apply_cached_mesh_linear(offset, linear);
        let fallback_length = fallback.length();
        return if fallback_length > 1.0e-12 {
            fallback * (length / fallback_length)
        } else {
            offset
        };
    }
    let normal = egui::vec2(
        (linear[3] * offset.x - linear[2] * offset.y) / det,
        (-linear[1] * offset.x + linear[0] * offset.y) / det,
    );
    let normal_length = normal.length();
    if normal_length > 1.0e-12 && normal_length.is_finite() {
        normal * (length / normal_length)
    } else {
        offset
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn reproject_cached_mesh_point_affine(
    point: Pos2,
    fixed_offset: egui::Vec2,
    source_origin: Pos2,
    target_origin: Pos2,
    linear: [f32; 4],
) -> Pos2 {
    let base = point - fixed_offset;
    target_origin
        + apply_cached_mesh_linear(base - source_origin, linear)
        + transform_fixed_screen_offset(fixed_offset, linear)
}

#[derive(Clone, Copy, Debug)]
pub struct CachedMeshTransform {
    pub source_origin: Pos2,
    pub target_origin: Pos2,
    pub linear: [f32; 4],
    pub fixed_screen_offset: bool,
}

/// Paint an already-tessellated egui mesh without feeding its full vertex/index
/// payload through egui again every frame. The first native draw uploads the
/// exact mesh to a persistent GL buffer; later draws only change the cheap camera
/// reprojection uniforms. This deliberately preserves the mesh triangles, colors,
/// UVs and draw order byte-for-byte.
///
/// This is intended for large stable editor overlays such as raw selection
/// outlines. It is not a geometry simplifier.
pub fn paint_cached_mesh(
    painter: &Painter,
    rect: Rect,
    mesh: Arc<Mesh>,
    source_origin: Pos2,
    target_origin: Pos2,
    scale: f32,
    screen_uv_tile: Option<f32>,
) {
    let scale = if scale.is_finite() { scale } else { 1.0 };
    paint_cached_mesh_affine(
        painter,
        rect,
        mesh,
        CachedMeshTransform {
            source_origin,
            target_origin,
            linear: [scale, 0.0, 0.0, scale],
            fixed_screen_offset: false,
        },
        screen_uv_tile,
    );
}

/// Affine variant used by selection overlays. `fixed_screen_offset` treats each
/// vertex UV as a screen-space stroke extrusion from its zero-width centerline.
/// The centerline follows the full affine transform while the extrusion keeps
/// its original pixel length and rotates by the inverse-transpose normal map.
/// That keeps 1 px / 2 px editor outlines exact through zoom, rotation, scale,
/// reflection and skew without rebuilding the mesh on the CPU.
pub fn paint_cached_mesh_affine(
    painter: &Painter,
    rect: Rect,
    mesh: Arc<Mesh>,
    transform: CachedMeshTransform,
    screen_uv_tile: Option<f32>,
) {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() || !rect.is_positive() {
        return;
    }
    let CachedMeshTransform {
        source_origin,
        target_origin,
        linear,
        fixed_screen_offset,
    } = transform;
    let linear = if linear.iter().all(|value| value.is_finite()) {
        linear
    } else {
        [1.0, 0.0, 0.0, 1.0]
    };

    #[cfg(target_arch = "wasm32")]
    {
        let mut fallback = (*mesh).clone();
        for vertex in &mut fallback.vertices {
            let fixed_offset = if fixed_screen_offset {
                egui::vec2(vertex.uv.x, vertex.uv.y)
            } else {
                egui::Vec2::ZERO
            };
            vertex.pos = reproject_cached_mesh_point_affine(
                vertex.pos,
                fixed_offset,
                source_origin,
                target_origin,
                linear,
            );
            if let Some(tile) = screen_uv_tile.filter(|tile| *tile > 0.0) {
                vertex.uv = pos2(vertex.pos.x / tile, vertex.pos.y / tile);
            } else if fixed_screen_offset {
                vertex.uv = Pos2::ZERO;
            }
        }
        painter.add(Shape::Mesh(fallback));
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(region) = callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };
            let Some(texture) = glow_painter.texture(mesh.texture_id) else {
                return;
            };
            let gl = glow_painter.gl();
            let context_key = Arc::as_ptr(gl) as usize;
            GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    resources.entry(context_key)
                {
                    let Ok(created) = (unsafe { GlResources::new(gl) }) else {
                        return;
                    };
                    entry.insert(created);
                }
                let Some(resources) = resources.get_mut(&context_key) else {
                    return;
                };
                unsafe {
                    resources.paint_cached_mesh(
                        gl,
                        &mesh,
                        texture,
                        region,
                        info.pixels_per_point,
                        info.screen_size_px,
                        source_origin,
                        target_origin,
                        linear,
                        screen_uv_tile.unwrap_or(0.0),
                        fixed_screen_offset,
                        false,
                    );
                }
            });
        });
        record_paint_callback(painter, rect, None);
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}

/// Analytic vector gradient paint. The shape stays a triangulated vector mesh;
/// the gradient field is evaluated per-fragment on the GPU. No spatial raster
/// texture is generated, so zoom cannot reveal pixels or force re-rasterization.
#[derive(Clone)]
pub struct GradientMeshPaint {
    pub transform: CachedMeshTransform,
    /// Bounds of the source mesh in the same local coordinates as its vertices.
    /// Group callbacks transform these four corners instead of scanning every
    /// vertex on every repaint.
    pub source_bounds: Rect,
    /// Inverse authored gradient field: local vector coordinate -> normalized
    /// gradient coordinate. Stored as [a11, a12, a21, a22, tx, ty].
    pub gradient_inverse: [f32; 6],
    pub kind: GradientKind,
    pub stops: Arc<Vec<GradientStop>>,
    pub tint: Color32,
}

#[derive(Clone)]
pub enum VectorGroupPrimitive {
    Solid(Mesh),
    Textured(Arc<Mesh>),
    Gradient {
        mesh: Arc<Mesh>,
        paint: GradientMeshPaint,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct VectorGroupFx {
    pub blur_radius_points: [f32; 2],
    pub blur_quality: u8,
    pub glow_radius_points: [f32; 2],
    pub glow_quality: u8,
    pub glow_color: Color32,
    pub glow_strength: f32,
}

impl Default for VectorGroupFx {
    fn default() -> Self {
        Self {
            blur_radius_points: [0.0, 0.0],
            blur_quality: 1,
            glow_radius_points: [0.0, 0.0],
            glow_quality: 1,
            glow_color: Color32::TRANSPARENT,
            glow_strength: 0.0,
        }
    }
}

impl VectorGroupFx {
    fn has_blur(self) -> bool {
        self.blur_radius_points[0] > 0.5 || self.blur_radius_points[1] > 0.5
    }

    fn has_glow(self) -> bool {
        self.glow_strength > 1.0e-6
            && self.glow_color.a() > 0
            && (self.glow_radius_points[0] > 0.5 || self.glow_radius_points[1] > 0.5)
    }
}

fn transformed_mesh_point(point: Pos2, transform: CachedMeshTransform) -> Pos2 {
    let delta = point - transform.source_origin;
    pos2(
        transform.target_origin.x + transform.linear[0] * delta.x + transform.linear[1] * delta.y,
        transform.target_origin.y + transform.linear[2] * delta.x + transform.linear[3] * delta.y,
    )
}

fn vector_group_rect(primitives: &[VectorGroupPrimitive]) -> Option<Rect> {
    let mut min = pos2(f32::INFINITY, f32::INFINITY);
    let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut any = false;
    let mut include = |point: Pos2| {
        if point.x.is_finite() && point.y.is_finite() {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            any = true;
        }
    };
    for primitive in primitives {
        match primitive {
            VectorGroupPrimitive::Solid(mesh) => {
                for vertex in &mesh.vertices {
                    include(vertex.pos);
                }
            }
            VectorGroupPrimitive::Textured(mesh) => {
                for vertex in &mesh.vertices {
                    include(vertex.pos);
                }
            }
            VectorGroupPrimitive::Gradient { paint, .. } => {
                for point in [
                    paint.source_bounds.left_top(),
                    paint.source_bounds.right_top(),
                    paint.source_bounds.right_bottom(),
                    paint.source_bounds.left_bottom(),
                ] {
                    include(transformed_mesh_point(point, paint.transform));
                }
            }
        }
    }
    (any && min.x < max.x && min.y < max.y).then(|| Rect::from_min_max(min, max).expand(1.0))
}

/// Composite an ordered vector display-object group on the GPU. Solid meshes and
/// analytic gradients stay vector; opacity is applied once after child overlap.
pub fn paint_vector_group_on_opaque_backdrop(
    painter: &Painter,
    primitives: Vec<VectorGroupPrimitive>,
    blend_mode: BlendMode,
    opacity: f32,
) {
    paint_vector_group_fx_on_opaque_backdrop(
        painter,
        primitives,
        blend_mode,
        opacity,
        VectorGroupFx::default(),
    );
}

/// GPU-compose a vector display-object group and apply Animate-style placement
/// blur/glow in the same transient framebuffer. The source remains vector/analytic
/// until the current-frame offscreen pass; no persistent bitmap sprite is created.
pub fn paint_vector_group_fx_on_opaque_backdrop(
    painter: &Painter,
    primitives: Vec<VectorGroupPrimitive>,
    blend_mode: BlendMode,
    opacity: f32,
    fx: VectorGroupFx,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if primitives.is_empty() || opacity <= 1.0e-6 {
        return;
    }
    let Some(mut rect) = vector_group_rect(&primitives) else {
        return;
    };
    if fx.has_blur() || fx.has_glow() {
        let pad_x = fx.blur_radius_points[0]
            .max(fx.glow_radius_points[0])
            .max(0.0)
            + 2.0;
        let pad_y = fx.blur_radius_points[1]
            .max(fx.glow_radius_points[1])
            .max(0.0)
            + 2.0;
        rect = Rect::from_min_max(
            pos2(rect.min.x - pad_x, rect.min.y - pad_y),
            pos2(rect.max.x + pad_x, rect.max.y + pad_y),
        );
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = fx;
        for primitive in primitives {
            match primitive {
                VectorGroupPrimitive::Solid(mut mesh) => {
                    for vertex in &mut mesh.vertices {
                        let [r, g, b, a] = vertex.color.to_array();
                        vertex.color = Color32::from_rgba_unmultiplied(
                            r,
                            g,
                            b,
                            ((f32::from(a) * opacity).round().clamp(0.0, 255.0)) as u8,
                        );
                    }
                    painter.add(Shape::Mesh(mesh));
                }
                VectorGroupPrimitive::Textured(mesh) => {
                    let mut mesh = (*mesh).clone();
                    for vertex in &mut mesh.vertices {
                        let [r, g, b, a] = vertex.color.to_array();
                        vertex.color = Color32::from_rgba_unmultiplied(
                            r,
                            g,
                            b,
                            ((f32::from(a) * opacity).round().clamp(0.0, 255.0)) as u8,
                        );
                    }
                    painter.add(Shape::Mesh(mesh));
                }
                VectorGroupPrimitive::Gradient { mesh, mut paint } => {
                    paint.tint = Color32::from_rgba_unmultiplied(
                        paint.tint.r(),
                        paint.tint.g(),
                        paint.tint.b(),
                        ((f32::from(paint.tint.a()) * opacity)
                            .round()
                            .clamp(0.0, 255.0)) as u8,
                    );
                    paint_gradient_mesh(painter, rect, mesh, paint);
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let primitives = Arc::new(primitives);
        let mode = blend_mode_tag(blend_mode);
        let fixed_opaque =
            blend_mode == BlendMode::Normal || uses_opaque_fixed_path(blend_mode, true);
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(region) = callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };
            let gl = glow_painter.gl();
            let context_key = Arc::as_ptr(gl) as usize;
            GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    resources.entry(context_key)
                {
                    let Ok(created) = (unsafe { GlResources::new(gl) }) else {
                        return;
                    };
                    entry.insert(created);
                }
                let Some(resources) = resources.get_mut(&context_key) else {
                    return;
                };
                unsafe {
                    resources.paint_vector_group(
                        glow_painter,
                        gl,
                        primitives.as_slice(),
                        mode,
                        fixed_opaque,
                        opacity,
                        fx,
                        region,
                        info.pixels_per_point,
                        info.screen_size_px,
                    );
                }
            });
        });
        record_paint_callback(painter, rect, Some(fx));
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}
pub fn paint_gradient_mesh(
    painter: &Painter,
    rect: Rect,
    mesh: Arc<Mesh>,
    paint: GradientMeshPaint,
) {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() || !rect.is_positive() {
        return;
    }
    let stop_count = paint.stops.len().min(256);
    if stop_count == 0 {
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Functional fallback for wasm: keep vector geometry, but interpolate at
        // vertices because the native OpenGL analytic callback is unavailable.
        let mut fallback = (*mesh).clone();
        let inv = paint.gradient_inverse;
        for vertex in &mut fallback.vertices {
            let local = vertex.pos;
            let gx = inv[0] * local.x + inv[1] * local.y + inv[4];
            let gy = inv[2] * local.x + inv[3] * local.y + inv[5];
            let position = match paint.kind {
                GradientKind::Linear => ((gx + 1.0) * 0.5).clamp(0.0, 1.0),
                GradientKind::Radial => (gx * gx + gy * gy).sqrt().clamp(0.0, 1.0),
            };
            let color = sample_stops(paint.stops.as_slice(), position);
            vertex.color = Color32::from_rgba_unmultiplied(
                ((u16::from(color[0]) * u16::from(paint.tint.r())) / 255) as u8,
                ((u16::from(color[1]) * u16::from(paint.tint.g())) / 255) as u8,
                ((u16::from(color[2]) * u16::from(paint.tint.b())) / 255) as u8,
                ((u16::from(color[3]) * u16::from(paint.tint.a())) / 255) as u8,
            );
            vertex.pos = reproject_cached_mesh_point_affine(
                vertex.pos,
                egui::Vec2::ZERO,
                paint.transform.source_origin,
                paint.transform.target_origin,
                paint.transform.linear,
            );
        }
        painter.add(Shape::Mesh(fallback));
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(region) = callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };
            let gl = glow_painter.gl();
            let context_key = Arc::as_ptr(gl) as usize;
            GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    resources.entry(context_key)
                {
                    let Ok(created) = (unsafe { GlResources::new(gl) }) else {
                        return;
                    };
                    entry.insert(created);
                }
                let Some(resources) = resources.get_mut(&context_key) else {
                    return;
                };
                unsafe {
                    resources.paint_gradient(
                        gl,
                        &mesh,
                        &paint,
                        region,
                        info.pixels_per_point,
                        info.screen_size_px,
                    );
                }
            });
        });
        record_paint_callback(painter, rect, None);
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}

#[cfg(target_arch = "wasm32")]
fn sample_stops(stops: &[GradientStop], position: f32) -> [u8; 4] {
    let Some(first) = stops.first() else {
        return [0, 0, 0, 0];
    };
    if position <= first.position {
        return [first.color.r, first.color.g, first.color.b, first.color.a];
    }
    for pair in stops.windows(2) {
        if position <= pair[1].position {
            let span = pair[1].position - pair[0].position;
            let t = if span.abs() <= f32::EPSILON {
                1.0
            } else {
                ((position - pair[0].position) / span).clamp(0.0, 1.0)
            };
            let mix =
                |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
            return [
                mix(pair[0].color.r, pair[1].color.r),
                mix(pair[0].color.g, pair[1].color.g),
                mix(pair[0].color.b, pair[1].color.b),
                mix(pair[0].color.a, pair[1].color.a),
            ];
        }
    }
    let last = stops.last().unwrap();
    [last.color.r, last.color.g, last.color.b, last.color.a]
}

fn paint_texture_impl(
    painter: &Painter,
    rect: Rect,
    texture_id: TextureId,
    blend_mode: BlendMode,
    opaque_backdrop: bool,
) {
    if blend_mode == BlendMode::Normal {
        painter.image(
            texture_id,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        // q0idmation's current publication target is native. Keep wasm builds
        // functional rather than silently dropping the placement.
        let _ = opaque_backdrop;
        painter.image(
            texture_id,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mode = blend_mode_tag(blend_mode);
        let fixed_opaque = uses_opaque_fixed_path(blend_mode, opaque_backdrop);
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(source_texture) = glow_painter.texture(texture_id) else {
                return;
            };
            let gl = glow_painter.gl();
            let context_key = Arc::as_ptr(gl) as usize;
            let Some(region) = callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };

            GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    resources.entry(context_key)
                {
                    let Ok(created) = (unsafe { GlResources::new(gl) }) else {
                        return;
                    };
                    entry.insert(created);
                }
                let Some(resources) = resources.get_mut(&context_key) else {
                    return;
                };
                unsafe {
                    if fixed_opaque {
                        resources.paint_opaque_fixed(
                            gl,
                            source_texture,
                            mode,
                            region,
                            resources.source_srgb,
                            1.0,
                        );
                    } else {
                        resources.paint(
                            gl,
                            source_texture,
                            mode,
                            region,
                            resources.source_srgb,
                            1.0,
                        );
                    }
                }
            });
        });
        record_paint_callback(painter, rect, None);
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}
fn uses_opaque_fixed_path(mode: BlendMode, opaque_backdrop: bool) -> bool {
    opaque_backdrop && matches!(mode, BlendMode::Multiply | BlendMode::Screen)
}

fn blend_mode_tag(mode: BlendMode) -> i32 {
    match mode {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Add => 3,
        BlendMode::Overlay => 4,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn grown_target_capacity(current: (i32, i32), needed: (i32, i32)) -> (i32, i32) {
    const BLOCK: i32 = 256;
    let grow = |have: i32, need: i32| {
        if have >= need {
            have
        } else {
            ((need.max(1) + BLOCK - 1) / BLOCK) * BLOCK
        }
    };
    (grow(current.0, needed.0), grow(current.1, needed.1))
}
#[cfg(not(target_arch = "wasm32"))]
fn gaussian_box_radii(support_radius_px: f32, passes: usize) -> Vec<i32> {
    if support_radius_px <= 0.5 || passes == 0 {
        return vec![0; passes.max(1)];
    }
    let sigma = (support_radius_px / 3.5).max(0.45);
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
            ((width - 1) / 2).clamp(0, 128)
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
impl GlResources {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = create_program(gl)?;
        let vao = gl
            .create_vertex_array()
            .map_err(|error| error.to_string())?;
        let backdrop_texture = gl.create_texture().map_err(|error| error.to_string())?;
        let backdrop_fbo = gl.create_framebuffer().map_err(|error| error.to_string())?;
        let source_uniform = gl.get_uniform_location(program, "u_source");
        let backdrop_uniform = gl.get_uniform_location(program, "u_backdrop");
        let backdrop_uv_max_uniform = gl.get_uniform_location(program, "u_backdrop_uv_max");
        let mode_uniform = gl.get_uniform_location(program, "u_mode");
        let passthrough_uniform = gl.get_uniform_location(program, "u_passthrough");
        let source_srgb_uniform = gl.get_uniform_location(program, "u_source_srgb");
        let source_opacity_uniform = gl.get_uniform_location(program, "u_opacity");
        let source_uv_min_uniform = gl.get_uniform_location(program, "u_source_uv_min");
        let source_uv_max_uniform = gl.get_uniform_location(program, "u_source_uv_max");
        let source_srgb = gl
            .supported_extensions()
            .iter()
            .any(|extension| extension.contains("sRGB"));
        let group_program = create_group_program(gl)?;
        let group_vao = gl
            .create_vertex_array()
            .map_err(|error| error.to_string())?;
        let group_vbo = gl.create_buffer().map_err(|error| error.to_string())?;
        let group_ebo = gl.create_buffer().map_err(|error| error.to_string())?;
        let group_texture = gl.create_texture().map_err(|error| error.to_string())?;
        let group_fbo = gl.create_framebuffer().map_err(|error| error.to_string())?;
        let group_msaa_fbo = gl.create_framebuffer().map_err(|error| error.to_string())?;
        let group_msaa_color = gl
            .create_renderbuffer()
            .map_err(|error| error.to_string())?;
        let group_ppp_uniform = gl.get_uniform_location(group_program, "u_pixels_per_point");
        let group_origin_uniform = gl.get_uniform_location(group_program, "u_region_origin_px");
        let group_size_uniform = gl.get_uniform_location(group_program, "u_region_size_px");
        let group_textured_program = create_group_textured_program(gl)?;
        let group_textured_ppp_uniform =
            gl.get_uniform_location(group_textured_program, "u_pixels_per_point");
        let group_textured_origin_uniform =
            gl.get_uniform_location(group_textured_program, "u_region_origin_px");
        let group_textured_size_uniform =
            gl.get_uniform_location(group_textured_program, "u_region_size_px");
        let group_textured_texture_uniform =
            gl.get_uniform_location(group_textured_program, "u_texture");
        let group_textured_srgb_uniform =
            gl.get_uniform_location(group_textured_program, "u_srgb_texture");
        let cached_solid_program = create_cached_solid_program(gl)?;
        let cached_solid_vao = gl
            .create_vertex_array()
            .map_err(|error| error.to_string())?;
        let cached_solid_screen_size_uniform =
            gl.get_uniform_location(cached_solid_program, "u_screen_size");
        let cached_solid_source_origin_uniform =
            gl.get_uniform_location(cached_solid_program, "u_source_origin");
        let cached_solid_target_origin_uniform =
            gl.get_uniform_location(cached_solid_program, "u_target_origin");
        let cached_solid_linear_uniform = gl.get_uniform_location(cached_solid_program, "u_linear");
        let cached_solid_fixed_screen_offset_uniform =
            gl.get_uniform_location(cached_solid_program, "u_fixed_screen_offset");
        let cached_solid_offscreen_uniform =
            gl.get_uniform_location(cached_solid_program, "u_offscreen");
        let cached_solid_screen_uv_tile_uniform =
            gl.get_uniform_location(cached_solid_program, "u_screen_uv_tile");
        let cached_solid_texture_uniform =
            gl.get_uniform_location(cached_solid_program, "u_texture");
        let cached_solid_srgb_texture_uniform =
            gl.get_uniform_location(cached_solid_program, "u_srgb_texture");

        let gradient_program = create_gradient_program(gl)?;
        let gradient_vao = gl
            .create_vertex_array()
            .map_err(|error| error.to_string())?;
        let gradient_stop_texture = gl.create_texture().map_err(|error| error.to_string())?;
        gl.bind_texture(glow::TEXTURE_2D, Some(gradient_stop_texture));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA32F as i32,
            256,
            2,
            0,
            glow::RGBA,
            glow::FLOAT,
            None,
        );
        let gradient_screen_size_uniform =
            gl.get_uniform_location(gradient_program, "u_screen_size");
        let gradient_source_origin_uniform =
            gl.get_uniform_location(gradient_program, "u_source_origin");
        let gradient_target_origin_uniform =
            gl.get_uniform_location(gradient_program, "u_target_origin");
        let gradient_linear_uniform = gl.get_uniform_location(gradient_program, "u_linear");
        let gradient_inverse_linear_uniform =
            gl.get_uniform_location(gradient_program, "u_gradient_inverse_linear");
        let gradient_inverse_translation_uniform =
            gl.get_uniform_location(gradient_program, "u_gradient_inverse_translation");
        let gradient_kind_uniform = gl.get_uniform_location(gradient_program, "u_gradient_kind");
        let gradient_stop_count_uniform = gl.get_uniform_location(gradient_program, "u_stop_count");
        let gradient_stops_uniform = gl.get_uniform_location(gradient_program, "u_stops");
        let gradient_tint_uniform = gl.get_uniform_location(gradient_program, "u_tint");
        let gradient_screen_offset_uniform =
            gl.get_uniform_location(gradient_program, "u_screen_offset");
        let gradient_offscreen_uniform = gl.get_uniform_location(gradient_program, "u_offscreen");
        let filter_program = create_filter_program(gl)?;
        let filter_source_uniform = gl.get_uniform_location(filter_program, "u_source");
        let filter_uv_max_uniform = gl.get_uniform_location(filter_program, "u_uv_max");
        let filter_texel_uniform = gl.get_uniform_location(filter_program, "u_texel");
        let filter_direction_uniform = gl.get_uniform_location(filter_program, "u_direction");
        let filter_radius_uniform = gl.get_uniform_location(filter_program, "u_radius");
        let combine_program = create_filter_combine_program(gl)?;
        let combine_body_uniform = gl.get_uniform_location(combine_program, "u_body");
        let combine_glow_uniform = gl.get_uniform_location(combine_program, "u_glow");
        let combine_uv_max_uniform = gl.get_uniform_location(combine_program, "u_uv_max");
        let combine_glow_color_uniform = gl.get_uniform_location(combine_program, "u_glow_color");
        let combine_glow_strength_uniform =
            gl.get_uniform_location(combine_program, "u_glow_strength");
        let filter_textures = [
            gl.create_texture().map_err(|error| error.to_string())?,
            gl.create_texture().map_err(|error| error.to_string())?,
            gl.create_texture().map_err(|error| error.to_string())?,
        ];
        let filter_fbos = [
            gl.create_framebuffer().map_err(|error| error.to_string())?,
            gl.create_framebuffer().map_err(|error| error.to_string())?,
            gl.create_framebuffer().map_err(|error| error.to_string())?,
        ];
        let gpu_filter_timer = GpuFilterTimer::new(gl);
        Ok(Self {
            program,
            vao,
            backdrop_texture,
            backdrop_fbo,
            backdrop_size: (0, 0),
            source_uniform,
            backdrop_uniform,
            backdrop_uv_max_uniform,
            mode_uniform,
            passthrough_uniform,
            source_srgb_uniform,
            source_opacity_uniform,
            source_uv_min_uniform,
            source_uv_max_uniform,
            source_srgb,
            group_program,
            group_vao,
            group_vbo,
            group_ebo,
            group_texture,
            group_fbo,
            group_msaa_fbo,
            group_msaa_color,
            group_size: (0, 0),
            group_ppp_uniform,
            group_origin_uniform,
            group_size_uniform,
            group_textured_program,
            group_textured_ppp_uniform,
            group_textured_origin_uniform,
            group_textured_size_uniform,
            group_textured_texture_uniform,
            group_textured_srgb_uniform,
            cached_solid_program,
            cached_solid_vao,
            cached_solid_screen_size_uniform,
            cached_solid_source_origin_uniform,
            cached_solid_target_origin_uniform,
            cached_solid_linear_uniform,
            cached_solid_fixed_screen_offset_uniform,
            cached_solid_offscreen_uniform,
            cached_solid_screen_uv_tile_uniform,
            cached_solid_texture_uniform,
            cached_solid_srgb_texture_uniform,
            cached_solid_buffers: HashMap::new(),
            gradient_program,
            gradient_vao,
            gradient_stop_texture,
            gradient_screen_size_uniform,
            gradient_source_origin_uniform,
            gradient_target_origin_uniform,
            gradient_linear_uniform,
            gradient_inverse_linear_uniform,
            gradient_inverse_translation_uniform,
            gradient_kind_uniform,
            gradient_stop_count_uniform,
            gradient_stops_uniform,
            gradient_tint_uniform,
            gradient_screen_offset_uniform,
            gradient_offscreen_uniform,
            gradient_buffers: HashMap::new(),
            filter_program,
            filter_source_uniform,
            filter_uv_max_uniform,
            filter_texel_uniform,
            filter_direction_uniform,
            filter_radius_uniform,
            combine_program,
            combine_body_uniform,
            combine_glow_uniform,
            combine_uv_max_uniform,
            combine_glow_color_uniform,
            combine_glow_strength_uniform,
            filter_textures,
            filter_fbos,
            filter_size: (0, 0),
            gpu_filter_timer,
        })
    }

    unsafe fn ensure_group_targets(&mut self, gl: &glow::Context, width: i32, height: i32) {
        let capacity = grown_target_capacity(self.group_size, (width, height));
        if self.group_size == capacity {
            return;
        }
        let (capacity_width, capacity_height) = capacity;
        gl.bind_texture(glow::TEXTURE_2D, Some(self.group_texture));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            capacity_width,
            capacity_height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.group_fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(self.group_texture),
            0,
        );

        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(self.group_msaa_color));
        gl.renderbuffer_storage_multisample(
            glow::RENDERBUFFER,
            4,
            glow::RGBA8,
            capacity_width,
            capacity_height,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::RENDERBUFFER,
            Some(self.group_msaa_color),
        );
        self.group_size = capacity;
    }

    unsafe fn ensure_filter_targets(&mut self, gl: &glow::Context, width: i32, height: i32) {
        let capacity = grown_target_capacity(self.filter_size, (width, height));
        if self.filter_size == capacity {
            return;
        }
        for (texture, fbo) in self.filter_textures.iter().zip(self.filter_fbos.iter()) {
            gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                capacity.0,
                capacity.1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(*fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(*texture),
                0,
            );
        }
        self.filter_size = capacity;
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_cached_mesh(
        &mut self,
        gl: &glow::Context,
        mesh: &Arc<Mesh>,
        texture: glow::Texture,
        region: CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
        source_origin: Pos2,
        target_origin: Pos2,
        linear: [f32; 4],
        screen_uv_tile: f32,
        fixed_screen_offset: bool,
        offscreen: bool,
    ) {
        let key = Arc::as_ptr(mesh) as usize;
        let reusable = self
            .cached_solid_buffers
            .get(&key)
            .and_then(|cached| cached.source.upgrade())
            .is_some_and(|cached| Arc::ptr_eq(&cached, mesh));
        if !reusable {
            if let Some(stale) = self.cached_solid_buffers.remove(&key) {
                gl.delete_buffer(stale.vbo);
                gl.delete_buffer(stale.ebo);
            }
            let dead: Vec<usize> = self
                .cached_solid_buffers
                .iter()
                .filter_map(|(key, cached)| cached.source.upgrade().is_none().then_some(*key))
                .collect();
            for dead_key in dead {
                if let Some(stale) = self.cached_solid_buffers.remove(&dead_key) {
                    gl.delete_buffer(stale.vbo);
                    gl.delete_buffer(stale.ebo);
                }
            }
            // Selection paint cache itself keeps only the current pose. Bound the
            // backend too so old zoom/theme poses cannot retain giant GL buffers.
            if self.cached_solid_buffers.len() >= 4 {
                let stale: Vec<_> = self
                    .cached_solid_buffers
                    .drain()
                    .map(|(_, value)| value)
                    .collect();
                for stale in stale {
                    gl.delete_buffer(stale.vbo);
                    gl.delete_buffer(stale.ebo);
                }
            }

            let vbo = gl.create_buffer().expect("create cached solid mesh VBO");
            let ebo = gl.create_buffer().expect("create cached solid mesh EBO");
            let mut packed = Vec::with_capacity(mesh.vertices.len() * 8);
            for vertex in &mesh.vertices {
                let [r, g, b, a] = vertex.color.to_array();
                packed.extend_from_slice(&[
                    vertex.pos.x,
                    vertex.pos.y,
                    vertex.uv.x,
                    vertex.uv.y,
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                    f32::from(a) / 255.0,
                ]);
            }
            let vertex_bytes = std::slice::from_raw_parts(
                packed.as_ptr().cast::<u8>(),
                packed.len() * std::mem::size_of::<f32>(),
            );
            let index_bytes = std::slice::from_raw_parts(
                mesh.indices.as_ptr().cast::<u8>(),
                mesh.indices.len() * std::mem::size_of::<u32>(),
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, index_bytes, glow::STATIC_DRAW);
            self.cached_solid_buffers.insert(
                key,
                CachedSolidMeshBuffers {
                    source: Arc::downgrade(mesh),
                    vbo,
                    ebo,
                    index_count: mesh.indices.len() as i32,
                },
            );
        }

        let cached = self
            .cached_solid_buffers
            .get(&key)
            .expect("cached mesh inserted above");
        let vbo = cached.vbo;
        let ebo = cached.ebo;
        let index_count = cached.index_count;

        gl.viewport(0, 0, screen_size_px[0] as i32, screen_size_px[1] as i32);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(region.left, region.bottom, region.width, region.height);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        // Match egui_glow exactly: egui vertices/textures are premultiplied.
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE_MINUS_DST_ALPHA,
            glow::ONE,
        );
        gl.use_program(Some(self.cached_solid_program));
        gl.bind_vertex_array(Some(self.cached_solid_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
        gl.enable_vertex_attrib_array(0);
        gl.enable_vertex_attrib_array(1);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 32, 0);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 32, 8);
        gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, 32, 16);
        let screen_size_points = [
            screen_size_px[0] as f32 / pixels_per_point,
            screen_size_px[1] as f32 / pixels_per_point,
        ];
        gl.uniform_2_f32(
            self.cached_solid_screen_size_uniform.as_ref(),
            screen_size_points[0],
            screen_size_points[1],
        );
        gl.uniform_2_f32(
            self.cached_solid_source_origin_uniform.as_ref(),
            source_origin.x,
            source_origin.y,
        );
        gl.uniform_2_f32(
            self.cached_solid_target_origin_uniform.as_ref(),
            target_origin.x,
            target_origin.y,
        );
        gl.uniform_4_f32(
            self.cached_solid_linear_uniform.as_ref(),
            linear[0],
            linear[1],
            linear[2],
            linear[3],
        );
        gl.uniform_1_i32(
            self.cached_solid_fixed_screen_offset_uniform.as_ref(),
            i32::from(fixed_screen_offset),
        );
        gl.uniform_1_i32(
            self.cached_solid_offscreen_uniform.as_ref(),
            i32::from(offscreen),
        );
        gl.uniform_1_f32(
            self.cached_solid_screen_uv_tile_uniform.as_ref(),
            screen_uv_tile,
        );
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.uniform_1_i32(self.cached_solid_texture_uniform.as_ref(), 0);
        gl.uniform_1_i32(
            self.cached_solid_srgb_texture_uniform.as_ref(),
            i32::from(self.source_srgb),
        );
        gl.draw_elements(glow::TRIANGLES, index_count, glow::UNSIGNED_INT, 0);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_gradient(
        &mut self,
        gl: &glow::Context,
        mesh: &Arc<Mesh>,
        paint: &GradientMeshPaint,
        region: CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
    ) {
        let key = Arc::as_ptr(mesh) as usize;
        let reusable = self
            .gradient_buffers
            .get(&key)
            .and_then(|cached| cached.source.upgrade())
            .is_some_and(|source| Arc::ptr_eq(&source, mesh));
        if !reusable {
            if let Some(stale) = self.gradient_buffers.remove(&key) {
                gl.delete_buffer(stale.vbo);
                gl.delete_buffer(stale.ebo);
            }
            if self.gradient_buffers.len() >= 64 {
                let stale: Vec<_> = self
                    .gradient_buffers
                    .drain()
                    .map(|(_, value)| value)
                    .collect();
                for stale in stale {
                    gl.delete_buffer(stale.vbo);
                    gl.delete_buffer(stale.ebo);
                }
            }
            let vbo = gl.create_buffer().expect("create gradient mesh VBO");
            let ebo = gl.create_buffer().expect("create gradient mesh EBO");
            let mut positions = Vec::with_capacity(mesh.vertices.len() * 2);
            for vertex in &mesh.vertices {
                positions.extend_from_slice(&[vertex.pos.x, vertex.pos.y]);
            }
            let vertex_bytes = std::slice::from_raw_parts(
                positions.as_ptr().cast::<u8>(),
                positions.len() * std::mem::size_of::<f32>(),
            );
            let index_bytes = std::slice::from_raw_parts(
                mesh.indices.as_ptr().cast::<u8>(),
                mesh.indices.len() * std::mem::size_of::<u32>(),
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, index_bytes, glow::STATIC_DRAW);
            self.gradient_buffers.insert(
                key,
                CachedSolidMeshBuffers {
                    source: Arc::downgrade(mesh),
                    vbo,
                    ebo,
                    index_count: mesh.indices.len() as i32,
                },
            );
        }
        let cached = self
            .gradient_buffers
            .get(&key)
            .expect("gradient mesh inserted above");

        // Stop data is a tiny 256x2 floating-point data table, not a spatial
        // gradient bitmap. The fragment shader still evaluates the field at the
        // exact screen fragment, so zoom never reveals texels or requires a reraster.
        let mut stop_data = vec![0.0f32; 256 * 2 * 4];
        let stop_count = paint.stops.len().min(256);
        for (index, stop) in paint.stops.iter().take(stop_count).enumerate() {
            let position = index * 4;
            stop_data[position] = stop.position.clamp(0.0, 1.0);
            let color = 256 * 4 + position;
            stop_data[color] = f32::from(stop.color.r) / 255.0;
            stop_data[color + 1] = f32::from(stop.color.g) / 255.0;
            stop_data[color + 2] = f32::from(stop.color.b) / 255.0;
            stop_data[color + 3] = f32::from(stop.color.a) / 255.0;
        }
        let stop_bytes = std::slice::from_raw_parts(
            stop_data.as_ptr().cast::<u8>(),
            stop_data.len() * std::mem::size_of::<f32>(),
        );
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.gradient_stop_texture));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            256,
            2,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(stop_bytes),
        );

        gl.viewport(0, 0, screen_size_px[0] as i32, screen_size_px[1] as i32);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(region.left, region.bottom, region.width, region.height);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE_MINUS_DST_ALPHA,
            glow::ONE,
        );
        gl.use_program(Some(self.gradient_program));
        gl.bind_vertex_array(Some(self.gradient_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(cached.vbo));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(cached.ebo));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);

        let screen_size_points = [
            screen_size_px[0] as f32 / pixels_per_point,
            screen_size_px[1] as f32 / pixels_per_point,
        ];
        gl.uniform_2_f32(
            self.gradient_screen_size_uniform.as_ref(),
            screen_size_points[0],
            screen_size_points[1],
        );
        gl.uniform_2_f32(self.gradient_screen_offset_uniform.as_ref(), 0.0, 0.0);
        gl.uniform_1_i32(self.gradient_offscreen_uniform.as_ref(), 0);
        gl.uniform_2_f32(
            self.gradient_source_origin_uniform.as_ref(),
            paint.transform.source_origin.x,
            paint.transform.source_origin.y,
        );
        gl.uniform_2_f32(
            self.gradient_target_origin_uniform.as_ref(),
            paint.transform.target_origin.x,
            paint.transform.target_origin.y,
        );
        gl.uniform_4_f32(
            self.gradient_linear_uniform.as_ref(),
            paint.transform.linear[0],
            paint.transform.linear[1],
            paint.transform.linear[2],
            paint.transform.linear[3],
        );
        let inv = paint.gradient_inverse;
        gl.uniform_4_f32(
            self.gradient_inverse_linear_uniform.as_ref(),
            inv[0],
            inv[1],
            inv[2],
            inv[3],
        );
        gl.uniform_2_f32(
            self.gradient_inverse_translation_uniform.as_ref(),
            inv[4],
            inv[5],
        );
        gl.uniform_1_i32(
            self.gradient_kind_uniform.as_ref(),
            match paint.kind {
                GradientKind::Linear => 0,
                GradientKind::Radial => 1,
            },
        );
        gl.uniform_1_i32(self.gradient_stop_count_uniform.as_ref(), stop_count as i32);
        gl.uniform_1_i32(self.gradient_stops_uniform.as_ref(), 0);
        let [tr, tg, tb, ta] = paint.tint.to_srgba_unmultiplied();
        gl.uniform_4_f32(
            self.gradient_tint_uniform.as_ref(),
            f32::from(tr) / 255.0,
            f32::from(tg) / 255.0,
            f32::from(tb) / 255.0,
            f32::from(ta) / 255.0,
        );
        gl.draw_elements(glow::TRIANGLES, cached.index_count, glow::UNSIGNED_INT, 0);
    }

    unsafe fn run_filter_pass(
        &mut self,
        gl: &glow::Context,
        source: glow::Texture,
        destination_fbo: glow::Framebuffer,
        region: CallbackRegion,
        direction: [f32; 2],
        radius: i32,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destination_fbo));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::BLEND);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.use_program(Some(self.filter_program));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(source));
        gl.uniform_1_i32(self.filter_source_uniform.as_ref(), 0);
        gl.uniform_2_f32(
            self.filter_uv_max_uniform.as_ref(),
            region.width as f32 / self.filter_size.0 as f32,
            region.height as f32 / self.filter_size.1 as f32,
        );
        gl.uniform_2_f32(
            self.filter_texel_uniform.as_ref(),
            1.0 / self.filter_size.0 as f32,
            1.0 / self.filter_size.1 as f32,
        );
        gl.uniform_2_f32(
            self.filter_direction_uniform.as_ref(),
            direction[0],
            direction[1],
        );
        gl.uniform_1_i32(self.filter_radius_uniform.as_ref(), radius.clamp(0, 128));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    unsafe fn blur_group_texture(
        &mut self,
        gl: &glow::Context,
        source: glow::Texture,
        radius_points: [f32; 2],
        quality: u8,
        region: CallbackRegion,
        pixels_per_point: f32,
    ) -> glow::Texture {
        let passes = usize::from(quality.clamp(1, 15));
        let radii_x = gaussian_box_radii(radius_points[0].max(0.0) * pixels_per_point, passes);
        let radii_y = gaussian_box_radii(radius_points[1].max(0.0) * pixels_per_point, passes);
        let mut current = source;
        for (radius_x, radius_y) in radii_x.into_iter().zip(radii_y) {
            self.run_filter_pass(
                gl,
                current,
                self.filter_fbos[0],
                region,
                [1.0, 0.0],
                radius_x,
            );
            current = self.filter_textures[0];
            self.run_filter_pass(
                gl,
                current,
                self.filter_fbos[1],
                region,
                [0.0, 1.0],
                radius_y,
            );
            current = self.filter_textures[1];
        }
        current
    }

    unsafe fn copy_filter_result_to_slot(
        &mut self,
        gl: &glow::Context,
        source_slot: usize,
        destination_slot: usize,
        region: CallbackRegion,
    ) {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.filter_fbos[source_slot]));
        gl.bind_framebuffer(
            glow::DRAW_FRAMEBUFFER,
            Some(self.filter_fbos[destination_slot]),
        );
        gl.blit_framebuffer(
            0,
            0,
            region.width,
            region.height,
            0,
            0,
            region.width,
            region.height,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
    }

    unsafe fn combine_body_and_glow(
        &mut self,
        gl: &glow::Context,
        body: glow::Texture,
        glow_mask: glow::Texture,
        destination_fbo: glow::Framebuffer,
        fx: VectorGroupFx,
        region: CallbackRegion,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destination_fbo));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::BLEND);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.use_program(Some(self.combine_program));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(body));
        gl.uniform_1_i32(self.combine_body_uniform.as_ref(), 0);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(glow_mask));
        gl.uniform_1_i32(self.combine_glow_uniform.as_ref(), 1);
        gl.uniform_2_f32(
            self.combine_uv_max_uniform.as_ref(),
            region.width as f32 / self.filter_size.0 as f32,
            region.height as f32 / self.filter_size.1 as f32,
        );
        let [r, g, b, a] = fx.glow_color.to_array();
        gl.uniform_4_f32(
            self.combine_glow_color_uniform.as_ref(),
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        );
        gl.uniform_1_f32(
            self.combine_glow_strength_uniform.as_ref(),
            fx.glow_strength.max(0.0),
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_vector_group(
        &mut self,
        glow_painter: &egui_glow::Painter,
        gl: &glow::Context,
        primitives: &[VectorGroupPrimitive],
        mode: i32,
        fixed_opaque: bool,
        opacity: f32,
        fx: VectorGroupFx,
        region: CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
    ) {
        self.gpu_filter_timer.poll_one(gl);
        let filtered = fx.has_blur() || fx.has_glow();
        let callback_cpu_started = filtered.then(std::time::Instant::now);
        let gpu_timer_query = self.gpu_filter_timer.begin_sample(gl, filtered);

        let read_binding = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
        let draw_binding = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        let read_framebuffer = native_framebuffer(read_binding);
        let draw_framebuffer = native_framebuffer(draw_binding);

        self.ensure_group_targets(gl, region.width, region.height);
        if fx.has_blur() || fx.has_glow() {
            self.ensure_filter_targets(gl, self.group_size.0, self.group_size.1);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );

        let screen_height = screen_size_px[1] as f32;
        let top_px = screen_height - (region.bottom + region.height) as f32;
        let group_size_points = [
            region.width as f32 / pixels_per_point,
            region.height as f32 / pixels_per_point,
        ];
        let group_offset_points = [
            region.left as f32 / pixels_per_point,
            top_px / pixels_per_point,
        ];

        for primitive in primitives {
            match primitive {
                VectorGroupPrimitive::Solid(mesh) => {
                    if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                        continue;
                    }
                    gl.use_program(Some(self.group_program));
                    gl.bind_vertex_array(Some(self.group_vao));
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.group_vbo));
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.group_ebo));
                    gl.enable_vertex_attrib_array(0);
                    gl.enable_vertex_attrib_array(1);
                    gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 24, 0);
                    gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 24, 8);
                    gl.uniform_1_f32(self.group_ppp_uniform.as_ref(), pixels_per_point);
                    gl.uniform_2_f32(
                        self.group_origin_uniform.as_ref(),
                        region.left as f32,
                        top_px,
                    );
                    gl.uniform_2_f32(
                        self.group_size_uniform.as_ref(),
                        region.width as f32,
                        region.height as f32,
                    );
                    let mut packed = Vec::with_capacity(mesh.vertices.len() * 6);
                    for vertex in &mesh.vertices {
                        let [r, g, b, a] = vertex.color.to_array();
                        packed.extend_from_slice(&[
                            vertex.pos.x,
                            vertex.pos.y,
                            f32::from(r) / 255.0,
                            f32::from(g) / 255.0,
                            f32::from(b) / 255.0,
                            f32::from(a) / 255.0,
                        ]);
                    }
                    let vertex_bytes = std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * std::mem::size_of::<f32>(),
                    );
                    let index_bytes = std::slice::from_raw_parts(
                        mesh.indices.as_ptr().cast::<u8>(),
                        mesh.indices.len() * std::mem::size_of::<u32>(),
                    );
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STREAM_DRAW);
                    gl.buffer_data_u8_slice(
                        glow::ELEMENT_ARRAY_BUFFER,
                        index_bytes,
                        glow::STREAM_DRAW,
                    );
                    gl.draw_elements(
                        glow::TRIANGLES,
                        mesh.indices.len() as i32,
                        glow::UNSIGNED_INT,
                        0,
                    );
                }
                VectorGroupPrimitive::Textured(mesh) => {
                    if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                        continue;
                    }
                    let Some(texture) = glow_painter.texture(mesh.texture_id) else {
                        continue;
                    };
                    gl.use_program(Some(self.group_textured_program));
                    gl.bind_vertex_array(Some(self.group_vao));
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.group_vbo));
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.group_ebo));
                    gl.enable_vertex_attrib_array(0);
                    gl.enable_vertex_attrib_array(1);
                    gl.enable_vertex_attrib_array(2);
                    gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 32, 0);
                    gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 32, 8);
                    gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, 32, 16);
                    gl.uniform_1_f32(self.group_textured_ppp_uniform.as_ref(), pixels_per_point);
                    gl.uniform_2_f32(
                        self.group_textured_origin_uniform.as_ref(),
                        region.left as f32,
                        top_px,
                    );
                    gl.uniform_2_f32(
                        self.group_textured_size_uniform.as_ref(),
                        region.width as f32,
                        region.height as f32,
                    );
                    let mut packed = Vec::with_capacity(mesh.vertices.len() * 8);
                    for vertex in &mesh.vertices {
                        let [r, g, b, a] = vertex.color.to_array();
                        packed.extend_from_slice(&[
                            vertex.pos.x,
                            vertex.pos.y,
                            vertex.uv.x,
                            vertex.uv.y,
                            f32::from(r) / 255.0,
                            f32::from(g) / 255.0,
                            f32::from(b) / 255.0,
                            f32::from(a) / 255.0,
                        ]);
                    }
                    let vertex_bytes = std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * std::mem::size_of::<f32>(),
                    );
                    let index_bytes = std::slice::from_raw_parts(
                        mesh.indices.as_ptr().cast::<u8>(),
                        mesh.indices.len() * std::mem::size_of::<u32>(),
                    );
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STREAM_DRAW);
                    gl.buffer_data_u8_slice(
                        glow::ELEMENT_ARRAY_BUFFER,
                        index_bytes,
                        glow::STREAM_DRAW,
                    );
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.uniform_1_i32(self.group_textured_texture_uniform.as_ref(), 0);
                    gl.uniform_1_i32(
                        self.group_textured_srgb_uniform.as_ref(),
                        i32::from(self.source_srgb),
                    );
                    gl.draw_elements(
                        glow::TRIANGLES,
                        mesh.indices.len() as i32,
                        glow::UNSIGNED_INT,
                        0,
                    );
                }
                VectorGroupPrimitive::Gradient { mesh, paint } => {
                    if mesh.indices.is_empty() || mesh.vertices.is_empty() || paint.stops.is_empty()
                    {
                        continue;
                    }
                    let key = Arc::as_ptr(mesh) as usize;
                    let reusable = self
                        .gradient_buffers
                        .get(&key)
                        .and_then(|cached| cached.source.upgrade())
                        .is_some_and(|source| Arc::ptr_eq(&source, mesh));
                    if !reusable {
                        if let Some(stale) = self.gradient_buffers.remove(&key) {
                            gl.delete_buffer(stale.vbo);
                            gl.delete_buffer(stale.ebo);
                        }
                        if self.gradient_buffers.len() >= 64 {
                            let stale: Vec<_> = self
                                .gradient_buffers
                                .drain()
                                .map(|(_, value)| value)
                                .collect();
                            for stale in stale {
                                gl.delete_buffer(stale.vbo);
                                gl.delete_buffer(stale.ebo);
                            }
                        }
                        let vbo = gl.create_buffer().expect("create group gradient VBO");
                        let ebo = gl.create_buffer().expect("create group gradient EBO");
                        let mut positions = Vec::with_capacity(mesh.vertices.len() * 2);
                        for vertex in &mesh.vertices {
                            positions.extend_from_slice(&[vertex.pos.x, vertex.pos.y]);
                        }
                        let vertex_bytes = std::slice::from_raw_parts(
                            positions.as_ptr().cast::<u8>(),
                            positions.len() * std::mem::size_of::<f32>(),
                        );
                        let index_bytes = std::slice::from_raw_parts(
                            mesh.indices.as_ptr().cast::<u8>(),
                            mesh.indices.len() * std::mem::size_of::<u32>(),
                        );
                        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                        gl.buffer_data_u8_slice(
                            glow::ARRAY_BUFFER,
                            vertex_bytes,
                            glow::STATIC_DRAW,
                        );
                        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
                        gl.buffer_data_u8_slice(
                            glow::ELEMENT_ARRAY_BUFFER,
                            index_bytes,
                            glow::STATIC_DRAW,
                        );
                        self.gradient_buffers.insert(
                            key,
                            CachedSolidMeshBuffers {
                                source: Arc::downgrade(mesh),
                                vbo,
                                ebo,
                                index_count: mesh.indices.len() as i32,
                            },
                        );
                    }
                    let cached = self
                        .gradient_buffers
                        .get(&key)
                        .expect("group gradient buffers");
                    let mut stop_data = vec![0.0f32; 256 * 2 * 4];
                    let stop_count = paint.stops.len().min(256);
                    for (index, stop) in paint.stops.iter().take(stop_count).enumerate() {
                        let position = index * 4;
                        stop_data[position] = stop.position.clamp(0.0, 1.0);
                        let color = 256 * 4 + position;
                        stop_data[color] = f32::from(stop.color.r) / 255.0;
                        stop_data[color + 1] = f32::from(stop.color.g) / 255.0;
                        stop_data[color + 2] = f32::from(stop.color.b) / 255.0;
                        stop_data[color + 3] = f32::from(stop.color.a) / 255.0;
                    }
                    let stop_bytes = std::slice::from_raw_parts(
                        stop_data.as_ptr().cast::<u8>(),
                        stop_data.len() * std::mem::size_of::<f32>(),
                    );
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.gradient_stop_texture));
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        256,
                        2,
                        glow::RGBA,
                        glow::FLOAT,
                        glow::PixelUnpackData::Slice(stop_bytes),
                    );
                    gl.use_program(Some(self.gradient_program));
                    gl.bind_vertex_array(Some(self.gradient_vao));
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(cached.vbo));
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(cached.ebo));
                    gl.enable_vertex_attrib_array(0);
                    gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
                    gl.uniform_2_f32(
                        self.gradient_screen_size_uniform.as_ref(),
                        group_size_points[0],
                        group_size_points[1],
                    );
                    gl.uniform_2_f32(
                        self.gradient_screen_offset_uniform.as_ref(),
                        group_offset_points[0],
                        group_offset_points[1],
                    );
                    gl.uniform_1_i32(self.gradient_offscreen_uniform.as_ref(), 1);
                    gl.uniform_2_f32(
                        self.gradient_source_origin_uniform.as_ref(),
                        paint.transform.source_origin.x,
                        paint.transform.source_origin.y,
                    );
                    gl.uniform_2_f32(
                        self.gradient_target_origin_uniform.as_ref(),
                        paint.transform.target_origin.x,
                        paint.transform.target_origin.y,
                    );
                    gl.uniform_4_f32(
                        self.gradient_linear_uniform.as_ref(),
                        paint.transform.linear[0],
                        paint.transform.linear[1],
                        paint.transform.linear[2],
                        paint.transform.linear[3],
                    );
                    let inv = paint.gradient_inverse;
                    gl.uniform_4_f32(
                        self.gradient_inverse_linear_uniform.as_ref(),
                        inv[0],
                        inv[1],
                        inv[2],
                        inv[3],
                    );
                    gl.uniform_2_f32(
                        self.gradient_inverse_translation_uniform.as_ref(),
                        inv[4],
                        inv[5],
                    );
                    gl.uniform_1_i32(
                        self.gradient_kind_uniform.as_ref(),
                        match paint.kind {
                            GradientKind::Linear => 0,
                            GradientKind::Radial => 1,
                        },
                    );
                    gl.uniform_1_i32(self.gradient_stop_count_uniform.as_ref(), stop_count as i32);
                    gl.uniform_1_i32(self.gradient_stops_uniform.as_ref(), 0);
                    let [tr, tg, tb, ta] = paint.tint.to_srgba_unmultiplied();
                    gl.uniform_4_f32(
                        self.gradient_tint_uniform.as_ref(),
                        f32::from(tr) / 255.0,
                        f32::from(tg) / 255.0,
                        f32::from(tb) / 255.0,
                        f32::from(ta) / 255.0,
                    );
                    gl.draw_elements(glow::TRIANGLES, cached.index_count, glow::UNSIGNED_INT, 0);
                }
            }
        }

        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.group_fbo));
        gl.blit_framebuffer(
            0,
            0,
            region.width,
            region.height,
            0,
            0,
            region.width,
            region.height,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );

        let mut source_texture = self.group_texture;
        let mut source_size = self.group_size;
        let has_glow = fx.has_glow();
        let has_blur = fx.has_blur();

        if has_glow {
            let glow_result = self.blur_group_texture(
                gl,
                self.group_texture,
                fx.glow_radius_points,
                fx.glow_quality,
                region,
                pixels_per_point,
            );
            debug_assert_eq!(glow_result, self.filter_textures[1]);
            if has_blur {
                self.copy_filter_result_to_slot(gl, 1, 2, region);
            }
        }

        if has_blur {
            source_texture = self.blur_group_texture(
                gl,
                self.group_texture,
                fx.blur_radius_points,
                fx.blur_quality,
                region,
                pixels_per_point,
            );
            source_size = self.filter_size;
        }

        if has_glow {
            let glow_mask = if has_blur {
                self.filter_textures[2]
            } else {
                self.filter_textures[1]
            };
            if has_blur {
                self.combine_body_and_glow(
                    gl,
                    source_texture,
                    glow_mask,
                    self.group_fbo,
                    fx,
                    region,
                );
                source_texture = self.group_texture;
                source_size = self.group_size;
            } else {
                self.combine_body_and_glow(
                    gl,
                    self.group_texture,
                    glow_mask,
                    self.filter_fbos[0],
                    fx,
                    region,
                );
                source_texture = self.filter_textures[0];
                source_size = self.filter_size;
            }
        }

        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw_framebuffer);

        let group_region = CallbackRegion {
            source_uv_min: [0.0, 0.0],
            source_uv_max: [
                region.width as f32 / source_size.0 as f32,
                region.height as f32 / source_size.1 as f32,
            ],
            ..region
        };
        if fixed_opaque {
            self.paint_opaque_fixed(gl, source_texture, mode, group_region, false, opacity);
        } else {
            self.paint(gl, source_texture, mode, group_region, false, opacity);
        }
        if let Some(index) = gpu_timer_query {
            self.gpu_filter_timer.end_sample(gl, index);
        }
        if let Some(started) = callback_cpu_started {
            let nanoseconds = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            record_gpu_filter_callback_cpu(nanoseconds);
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_solid_group(
        &mut self,
        gl: &glow::Context,
        meshes: &[Mesh],
        mode: i32,
        fixed_opaque: bool,
        opacity: f32,
        region: CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
    ) {
        let read_binding = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
        let draw_binding = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        let read_framebuffer = native_framebuffer(read_binding);
        let draw_framebuffer = native_framebuffer(draw_binding);

        self.ensure_group_targets(gl, region.width, region.height);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        gl.use_program(Some(self.group_program));
        gl.bind_vertex_array(Some(self.group_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.group_vbo));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.group_ebo));
        gl.enable_vertex_attrib_array(0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 24, 0);
        gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 24, 8);
        let screen_height = screen_size_px[1] as f32;
        let top_px = screen_height - (region.bottom + region.height) as f32;
        gl.uniform_1_f32(self.group_ppp_uniform.as_ref(), pixels_per_point);
        gl.uniform_2_f32(
            self.group_origin_uniform.as_ref(),
            region.left as f32,
            top_px,
        );
        gl.uniform_2_f32(
            self.group_size_uniform.as_ref(),
            region.width as f32,
            region.height as f32,
        );

        for mesh in meshes {
            if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                continue;
            }
            let mut packed = Vec::with_capacity(mesh.vertices.len() * 6);
            for vertex in &mesh.vertices {
                let [r, g, b, a] = vertex.color.to_array();
                packed.extend_from_slice(&[
                    vertex.pos.x,
                    vertex.pos.y,
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                    f32::from(a) / 255.0,
                ]);
            }
            let vertex_bytes = std::slice::from_raw_parts(
                packed.as_ptr().cast::<u8>(),
                packed.len() * std::mem::size_of::<f32>(),
            );
            let index_bytes = std::slice::from_raw_parts(
                mesh.indices.as_ptr().cast::<u8>(),
                mesh.indices.len() * std::mem::size_of::<u32>(),
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STREAM_DRAW);
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, index_bytes, glow::STREAM_DRAW);
            gl.draw_elements(
                glow::TRIANGLES,
                mesh.indices.len() as i32,
                glow::UNSIGNED_INT,
                0,
            );
        }

        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.group_fbo));
        gl.blit_framebuffer(
            0,
            0,
            region.width,
            region.height,
            0,
            0,
            region.width,
            region.height,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw_framebuffer);

        let group_region = CallbackRegion {
            source_uv_min: [0.0, 0.0],
            source_uv_max: [
                region.width as f32 / self.group_size.0 as f32,
                region.height as f32 / self.group_size.1 as f32,
            ],
            ..region
        };
        if fixed_opaque {
            self.paint_opaque_fixed(gl, self.group_texture, mode, group_region, false, opacity);
        } else {
            self.paint(gl, self.group_texture, mode, group_region, false, opacity);
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_opaque_fixed(
        &mut self,
        gl: &glow::Context,
        source_texture: glow::Texture,
        mode: i32,
        region: CallbackRegion,
        source_srgb: bool,
        opacity: f32,
    ) {
        debug_assert!(
            mode == blend_mode_tag(BlendMode::Normal)
                || mode == blend_mode_tag(BlendMode::Multiply)
                || mode == blend_mode_tag(BlendMode::Screen)
        );
        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        match mode {
            0 => gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
            1 => gl.blend_func_separate(
                glow::DST_COLOR,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
            2 => gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_COLOR,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
            _ => return,
        }
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(source_texture));
        gl.uniform_1_i32(self.source_uniform.as_ref(), 0);
        gl.uniform_1_i32(self.passthrough_uniform.as_ref(), 1);
        gl.uniform_1_i32(self.source_srgb_uniform.as_ref(), i32::from(source_srgb));
        gl.uniform_1_f32(
            self.source_opacity_uniform.as_ref(),
            opacity.clamp(0.0, 1.0),
        );
        gl.uniform_2_f32(
            self.source_uv_min_uniform.as_ref(),
            region.source_uv_min[0],
            region.source_uv_min[1],
        );
        gl.uniform_2_f32(
            self.source_uv_max_uniform.as_ref(),
            region.source_uv_max[0],
            region.source_uv_max[1],
        );
        gl.uniform_2_f32(self.backdrop_uv_max_uniform.as_ref(), 1.0, 1.0);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        source_texture: glow::Texture,
        mode: i32,
        region: CallbackRegion,
        source_srgb: bool,
        opacity: f32,
    ) {
        let read_binding = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
        let draw_binding = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        let read_framebuffer = native_framebuffer(read_binding);
        let draw_framebuffer = native_framebuffer(draw_binding);

        let backdrop_capacity =
            grown_target_capacity(self.backdrop_size, (region.width, region.height));
        if self.backdrop_size != backdrop_capacity {
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.backdrop_texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                backdrop_capacity.0,
                backdrop_capacity.1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.backdrop_fbo));
            gl.framebuffer_texture_2d(
                glow::DRAW_FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.backdrop_texture),
                0,
            );
            self.backdrop_size = backdrop_capacity;
        }

        // Resolve/copy only the placement rectangle. This is the key difference
        // from the old whole-stage fallback and also works with MSAA framebuffers.
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.backdrop_fbo));
        gl.blit_framebuffer(
            region.left,
            region.bottom,
            region.left + region.width,
            region.bottom + region.height,
            0,
            0,
            region.width,
            region.height,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw_framebuffer);

        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.disable(glow::BLEND);
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(source_texture));
        gl.uniform_1_i32(self.source_uniform.as_ref(), 0);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.backdrop_texture));
        gl.uniform_1_i32(self.backdrop_uniform.as_ref(), 1);
        gl.uniform_1_i32(self.mode_uniform.as_ref(), mode);
        gl.uniform_2_f32(
            self.backdrop_uv_max_uniform.as_ref(),
            region.width as f32 / self.backdrop_size.0 as f32,
            region.height as f32 / self.backdrop_size.1 as f32,
        );
        gl.uniform_1_i32(self.passthrough_uniform.as_ref(), 0);
        gl.uniform_1_i32(self.source_srgb_uniform.as_ref(), i32::from(source_srgb));
        gl.uniform_1_f32(
            self.source_opacity_uniform.as_ref(),
            opacity.clamp(0.0, 1.0),
        );
        gl.uniform_2_f32(
            self.source_uv_min_uniform.as_ref(),
            region.source_uv_min[0],
            region.source_uv_min[1],
        );
        gl.uniform_2_f32(
            self.source_uv_max_uniform.as_ref(),
            region.source_uv_max[0],
            region.source_uv_max[1],
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_framebuffer(raw: i32) -> Option<glow::Framebuffer> {
    NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
uniform vec2 u_source_uv_min;
uniform vec2 u_source_uv_max;
uniform vec2 u_backdrop_uv_max;
out vec2 v_source_uv;
out vec2 v_backdrop_uv;
void main() {
    vec2 p;
    if (gl_VertexID == 0) p = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) p = vec2(1.0, -1.0);
    else if (gl_VertexID == 2) p = vec2(-1.0, 1.0);
    else p = vec2(1.0, 1.0);
    gl_Position = vec4(p, 0.0, 1.0);
    vec2 source_t = vec2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    v_source_uv = mix(u_source_uv_min, u_source_uv_max, source_t);
    v_backdrop_uv = vec2((p.x + 1.0) * 0.5, (p.y + 1.0) * 0.5) * u_backdrop_uv_max;
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_source;
uniform sampler2D u_backdrop;
uniform int u_mode;
uniform int u_passthrough;
uniform int u_source_srgb;
uniform float u_opacity;
in vec2 v_source_uv;
in vec2 v_backdrop_uv;
out vec4 out_color;

vec3 srgb_gamma_from_linear(vec3 rgb) {
    bvec3 cutoff = lessThan(rgb, vec3(0.0031308));
    vec3 lower = rgb * vec3(12.92);
    vec3 higher = vec3(1.055) * pow(max(rgb, vec3(0.0)), vec3(1.0 / 2.4)) - vec3(0.055);
    return mix(higher, lower, vec3(cutoff));
}

vec3 srgb_linear_from_gamma(vec3 rgb) {
    bvec3 cutoff = lessThanEqual(rgb, vec3(0.04045));
    vec3 lower = rgb / vec3(12.92);
    vec3 higher = pow((rgb + vec3(0.055)) / vec3(1.055), vec3(2.4));
    return mix(higher, lower, vec3(cutoff));
}

vec4 source_straight_gamma() {
    vec4 sampled = texture(u_source, v_source_uv);
    float sa = clamp(sampled.a, 0.0, 1.0);
    if (sa <= 0.00001) return vec4(0.0);
    vec3 linear_premul = u_source_srgb != 0
        ? sampled.rgb
        : srgb_linear_from_gamma(sampled.rgb);
    vec3 straight_linear = clamp(linear_premul / sa, 0.0, 1.0);
    return vec4(srgb_gamma_from_linear(straight_linear), sa);
}

vec3 blend_rgb(vec3 back, vec3 src) {
    if (u_mode == 1) return back * src;
    if (u_mode == 2) return 1.0 - (1.0 - back) * (1.0 - src);
    if (u_mode == 3) return min(vec3(1.0), back + src);
    if (u_mode == 4) {
        vec3 low = 2.0 * back * src;
        vec3 high = 1.0 - 2.0 * (1.0 - back) * (1.0 - src);
        return mix(low, high, step(vec3(0.5), back));
    }
    return src;
}

void main() {
    vec4 source = source_straight_gamma();
    float sa = source.a * clamp(u_opacity, 0.0, 1.0);
    if (sa <= 0.00001) discard;
    vec3 src = source.rgb;
    vec4 src_premul = vec4(src * sa, sa);
    if (u_passthrough != 0) {
        out_color = src_premul;
        return;
    }

    vec4 back_premul = texture(u_backdrop, v_backdrop_uv);
    float da = clamp(back_premul.a, 0.0, 1.0);
    vec3 back = da > 0.00001 ? back_premul.rgb / da : vec3(0.0);
    vec3 mixed = blend_rgb(clamp(back, 0.0, 1.0), src);
    float out_a = sa + da - sa * da;
    vec3 out_premul = (1.0 - sa) * back_premul.rgb
        + (1.0 - da) * sa * src
        + sa * da * mixed;
    out_color = vec4(out_premul, out_a);
}
"#;

    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, VERTEX);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, FRAGMENT);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_group_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
in vec2 a_pos;
in vec4 a_color;
uniform float u_pixels_per_point;
uniform vec2 u_region_origin_px;
uniform vec2 u_region_size_px;
out vec4 v_color;
void main() {
    vec2 pixel = a_pos * u_pixels_per_point;
    vec2 local = (pixel - u_region_origin_px) / u_region_size_px;
    // Store screen-top at texture v=0 so the existing texture compositor and
    // CPU-uploaded egui textures share one orientation.
    gl_Position = vec4(local.x * 2.0 - 1.0, local.y * 2.0 - 1.0, 0.0, 1.0);
    v_color = a_color;
}
"#;
    const FRAGMENT: &str = r#"#version 140
in vec4 v_color;
out vec4 out_color;
void main() {
    if (v_color.a <= 0.00001) discard;
    out_color = v_color;
}
"#;

    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, VERTEX);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, FRAGMENT);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.bind_attrib_location(program, 1, "a_color");
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_group_textured_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
in vec2 a_pos;
in vec2 a_uv;
in vec4 a_color;
uniform float u_pixels_per_point;
uniform vec2 u_region_origin_px;
uniform vec2 u_region_size_px;
out vec2 v_uv;
out vec4 v_color;
void main() {
    vec2 pixel = a_pos * u_pixels_per_point;
    vec2 local = (pixel - u_region_origin_px) / u_region_size_px;
    gl_Position = vec4(local.x * 2.0 - 1.0, local.y * 2.0 - 1.0, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_texture;
uniform int u_srgb_texture;
in vec2 v_uv;
in vec4 v_color;
out vec4 out_color;
vec3 srgb_gamma_from_linear(vec3 rgb) {
    bvec3 cutoff = lessThan(rgb, vec3(0.0031308));
    vec3 lower = rgb * vec3(12.92);
    vec3 higher = vec3(1.055) * pow(rgb, vec3(1.0 / 2.4)) - vec3(0.055);
    return mix(higher, lower, vec3(cutoff));
}
void main() {
    vec4 sampled = texture(u_texture, v_uv);
    if (u_srgb_texture != 0) {
        sampled = vec4(srgb_gamma_from_linear(sampled.rgb), sampled.a);
    }
    vec4 color = v_color * sampled;
    if (color.a <= 0.00001 && dot(color.rgb, color.rgb) <= 0.0000001) discard;
    out_color = color;
}
"#;
    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, VERTEX);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, FRAGMENT);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.bind_attrib_location(program, 1, "a_uv");
    gl.bind_attrib_location(program, 2, "a_color");
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_gradient_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
in vec2 a_pos;
uniform vec2 u_screen_size;
uniform vec2 u_screen_offset;
uniform int u_offscreen;
uniform vec2 u_source_origin;
uniform vec2 u_target_origin;
uniform vec4 u_linear;
out vec2 v_local;
vec2 apply_linear(vec2 value) {
    return vec2(
        u_linear.x * value.x + u_linear.y * value.y,
        u_linear.z * value.x + u_linear.w * value.y
    );
}
void main() {
    vec2 screen = u_target_origin + apply_linear(a_pos - u_source_origin) - u_screen_offset;
    float ndc_y = u_offscreen != 0
        ? 2.0 * screen.y / u_screen_size.y - 1.0
        : 1.0 - 2.0 * screen.y / u_screen_size.y;
    gl_Position = vec4(
        2.0 * screen.x / u_screen_size.x - 1.0,
        ndc_y,
        0.0,
        1.0
    );
    v_local = a_pos;
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform vec4 u_gradient_inverse_linear;
uniform vec2 u_gradient_inverse_translation;
uniform int u_gradient_kind;
uniform int u_stop_count;
uniform sampler2D u_stops;
uniform vec4 u_tint;
in vec2 v_local;
out vec4 out_color;
vec4 stop_color(int index) {
    return texelFetch(u_stops, ivec2(index, 1), 0);
}
float stop_position(int index) {
    return texelFetch(u_stops, ivec2(index, 0), 0).r;
}
vec4 sample_gradient(float t) {
    t = clamp(t, 0.0, 1.0);
    if (u_stop_count <= 0) return vec4(0.0);
    float p0 = stop_position(0);
    vec4 c0 = stop_color(0);
    if (t <= p0 || u_stop_count == 1) return c0;
    for (int i = 1; i < 256; ++i) {
        if (i >= u_stop_count) break;
        float p1 = stop_position(i);
        vec4 c1 = stop_color(i);
        if (t <= p1) {
            float span = p1 - p0;
            float f = abs(span) <= 0.000001 ? 1.0 : clamp((t - p0) / span, 0.0, 1.0);
            return mix(c0, c1, f);
        }
        p0 = p1;
        c0 = c1;
    }
    return c0;
}
void main() {
    vec2 g = vec2(
        u_gradient_inverse_linear.x * v_local.x
            + u_gradient_inverse_linear.y * v_local.y
            + u_gradient_inverse_translation.x,
        u_gradient_inverse_linear.z * v_local.x
            + u_gradient_inverse_linear.w * v_local.y
            + u_gradient_inverse_translation.y
    );
    float t = u_gradient_kind == 0 ? (g.x + 1.0) * 0.5 : length(g);
    vec4 color = sample_gradient(t);
    color *= u_tint;
    // q0/egui compositing is premultiplied-alpha.
    out_color = vec4(color.rgb * color.a, color.a);
}
"#;
    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, VERTEX);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, FRAGMENT);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_filter_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
out vec2 v_uv;
void main() {
    vec2 p;
    if (gl_VertexID == 0) p = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) p = vec2(1.0, -1.0);
    else if (gl_VertexID == 2) p = vec2(-1.0, 1.0);
    else p = vec2(1.0, 1.0);
    gl_Position = vec4(p, 0.0, 1.0);
    v_uv = vec2((p.x + 1.0) * 0.5, (p.y + 1.0) * 0.5);
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_source;
uniform vec2 u_uv_max;
uniform vec2 u_texel;
uniform vec2 u_direction;
uniform int u_radius;
in vec2 v_uv;
out vec4 out_color;
void main() {
    vec2 center = v_uv * u_uv_max;
    vec4 sum = vec4(0.0);
    float count = 0.0;
    for (int i = -128; i <= 128; ++i) {
        if (abs(i) > u_radius) continue;
        vec2 uv = center + u_direction * float(i) * u_texel;
        if (uv.x >= 0.0 && uv.y >= 0.0 && uv.x <= u_uv_max.x && uv.y <= u_uv_max.y) {
            sum += texture(u_source, uv);
        }
        count += 1.0;
    }
    out_color = count > 0.0 ? sum / count : vec4(0.0);
}
"#;
    create_fullscreen_program(gl, VERTEX, FRAGMENT)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_filter_combine_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
out vec2 v_uv;
void main() {
    vec2 p;
    if (gl_VertexID == 0) p = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) p = vec2(1.0, -1.0);
    else if (gl_VertexID == 2) p = vec2(-1.0, 1.0);
    else p = vec2(1.0, 1.0);
    gl_Position = vec4(p, 0.0, 1.0);
    v_uv = vec2((p.x + 1.0) * 0.5, (p.y + 1.0) * 0.5);
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_body;
uniform sampler2D u_glow;
uniform vec2 u_uv_max;
uniform vec4 u_glow_color;
uniform float u_glow_strength;
in vec2 v_uv;
out vec4 out_color;
void main() {
    vec2 uv = v_uv * u_uv_max;
    vec4 body = texture(u_body, uv);
    float glow_alpha = clamp(texture(u_glow, uv).a * u_glow_color.a * u_glow_strength, 0.0, 1.0);
    vec4 glow = vec4(u_glow_color.rgb * glow_alpha, glow_alpha);
    out_color = body + glow * (1.0 - body.a);
}
"#;
    create_fullscreen_program(gl, VERTEX, FRAGMENT)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_fullscreen_program(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> Result<glow::Program, String> {
    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, vertex_source);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, fragment_source);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn create_cached_solid_program(gl: &glow::Context) -> Result<glow::Program, String> {
    const VERTEX: &str = r#"#version 140
in vec2 a_pos;
in vec2 a_uv;
in vec4 a_color;
uniform vec2 u_screen_size;
uniform vec2 u_source_origin;
uniform vec2 u_target_origin;
uniform vec4 u_linear;
uniform int u_fixed_screen_offset;
uniform int u_offscreen;
uniform float u_screen_uv_tile;
out vec2 v_uv;
out vec4 v_color;
vec2 apply_linear(vec2 value) {
    return vec2(
        u_linear.x * value.x + u_linear.y * value.y,
        u_linear.z * value.x + u_linear.w * value.y
    );
}
vec2 transform_fixed_offset(vec2 offset) {
    float offset_len = length(offset);
    if (offset_len <= 0.000001) return vec2(0.0);
    float det = u_linear.x * u_linear.w - u_linear.y * u_linear.z;
    vec2 mapped;
    if (abs(det) > 0.00000001) {
        mapped = vec2(
            (u_linear.w * offset.x - u_linear.z * offset.y) / det,
            (-u_linear.y * offset.x + u_linear.x * offset.y) / det
        );
    } else {
        mapped = apply_linear(offset);
    }
    float mapped_len = length(mapped);
    return mapped_len > 0.000001 ? mapped * (offset_len / mapped_len) : offset;
}
void main() {
    vec2 fixed_offset = u_fixed_screen_offset != 0 ? a_uv : vec2(0.0);
    vec2 base = a_pos - fixed_offset;
    vec2 screen = u_target_origin
        + apply_linear(base - u_source_origin)
        + transform_fixed_offset(fixed_offset);
    float ndc_y = u_offscreen != 0
        ? 2.0 * screen.y / u_screen_size.y - 1.0
        : 1.0 - 2.0 * screen.y / u_screen_size.y;
    gl_Position = vec4(
        2.0 * screen.x / u_screen_size.x - 1.0,
        ndc_y,
        0.0,
        1.0
    );
    v_uv = u_screen_uv_tile > 0.0
        ? screen / u_screen_uv_tile
        : (u_fixed_screen_offset != 0 ? vec2(0.0) : a_uv);
    v_color = a_color;
}
"#;
    const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_texture;
uniform int u_srgb_texture;
in vec2 v_uv;
in vec4 v_color;
out vec4 out_color;
vec3 srgb_gamma_from_linear(vec3 rgb) {
    bvec3 cutoff = lessThan(rgb, vec3(0.0031308));
    vec3 lower = rgb * vec3(12.92);
    vec3 higher = vec3(1.055) * pow(rgb, vec3(1.0 / 2.4)) - vec3(0.055);
    return mix(higher, lower, vec3(cutoff));
}
void main() {
    vec4 sampled = texture(u_texture, v_uv);
    if (u_srgb_texture != 0) {
        sampled = vec4(srgb_gamma_from_linear(sampled.rgb), sampled.a);
    }
    out_color = v_color * sampled;
}
"#;

    let vertex = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(vertex, VERTEX);
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let log = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(log);
    }
    let fragment = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|error| error.to_string())?;
    gl.shader_source(fragment, FRAGMENT);
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let log = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(log);
    }
    let program = gl.create_program().map_err(|error| error.to_string())?;
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.bind_attrib_location(program, 1, "a_uv");
    gl.bind_attrib_location(program, 2, "a_color");
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_diagnostics_count_filter_passes_and_visible_work() {
        reset_paint_diagnostics();
        let ctx = egui::Context::default();
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        let mut expected_region_pixels = 0;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("paint-diagnostics"),
                ));
                let clip = painter.clip_rect();
                expected_region_pixels =
                    (clip.width().ceil() as u64).saturating_mul(clip.height().ceil() as u64);
                record_paint_callback(
                    &painter,
                    clip,
                    Some(VectorGroupFx {
                        blur_radius_points: [8.0, 0.0],
                        blur_quality: 2,
                        glow_radius_points: [0.0, 6.0],
                        glow_quality: 3,
                        glow_color: Color32::WHITE,
                        glow_strength: 1.0,
                    }),
                );
            },
        );

        let diagnostics = take_paint_diagnostics();
        assert_eq!(diagnostics.callbacks, 1);
        assert_eq!(diagnostics.filtered_callbacks, 1);
        assert_eq!(diagnostics.blur_passes, 4);
        assert_eq!(diagnostics.glow_passes, 6);
        assert_eq!(
            diagnostics.filter_pixel_passes,
            expected_region_pixels.saturating_mul(10)
        );
        assert_eq!(diagnostics.max_filter_region_pixels, expected_region_pixels);
    }

    #[test]
    fn cached_mesh_reprojection_matches_cpu_selection_transform() {
        let source = pos2(120.0, 80.0);
        let target = pos2(42.0, -15.0);
        let point = pos2(150.0, 100.0);
        let actual = reproject_cached_mesh_point_affine(
            point,
            egui::Vec2::ZERO,
            source,
            target,
            [1.75, 0.0, 0.0, 1.75],
        );
        assert!((actual.x - 94.5).abs() < 1.0e-6);
        assert!((actual.y - 20.0).abs() < 1.0e-6);
    }

    #[test]
    fn gradient_group_bounds_transform_cached_source_corners() {
        let paint = GradientMeshPaint {
            transform: CachedMeshTransform {
                source_origin: Pos2::ZERO,
                target_origin: pos2(100.0, 50.0),
                linear: [2.0, 0.5, -0.25, 3.0],
                fixed_screen_offset: false,
            },
            source_bounds: Rect::from_min_max(pos2(-20.0, -10.0), pos2(40.0, 30.0)),
            gradient_inverse: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            kind: GradientKind::Linear,
            stops: Arc::new(vec![GradientStop {
                position: 0.0,
                color: q0s_format::v2::Rgba {
                    r: 240,
                    g: 120,
                    b: 30,
                    a: 255,
                },
            }]),
            tint: Color32::WHITE,
        };
        let primitive = VectorGroupPrimitive::Gradient {
            mesh: Arc::new(Mesh::default()),
            paint,
        };

        let bounds = vector_group_rect(&[primitive]).expect("transformed vector bounds");
        assert!((bounds.min.x - 54.0).abs() < 1.0e-5);
        assert!((bounds.min.y - 9.0).abs() < 1.0e-5);
        assert!((bounds.max.x - 196.0).abs() < 1.0e-5);
        assert!((bounds.max.y - 146.0).abs() < 1.0e-5);
    }

    #[test]
    fn fixed_screen_stroke_offset_stays_pixel_sized_under_affine_scale_and_skew() {
        let tangent = egui::vec2(3.0, 1.0);
        let offset = egui::vec2(-1.0, 3.0).normalized() * 2.0;
        let linear = [2.5, 0.8, -0.3, 0.6];
        let mapped_tangent = apply_cached_mesh_linear(tangent, linear);
        let mapped_offset = transform_fixed_screen_offset(offset, linear);
        assert!((mapped_offset.length() - 2.0).abs() < 1.0e-5);
        assert!(mapped_tangent.dot(mapped_offset).abs() < 1.0e-4);
    }

    #[test]
    fn gpu_group_targets_grow_in_blocks_and_never_thrash_between_smaller_objects() {
        assert_eq!(grown_target_capacity((0, 0), (101, 257)), (256, 512));
        assert_eq!(grown_target_capacity((256, 512), (200, 300)), (256, 512));
        assert_eq!(grown_target_capacity((256, 512), (300, 280)), (512, 512));
        assert_eq!(grown_target_capacity((512, 512), (260, 900)), (512, 1024));
    }

    #[test]
    fn zoomed_callback_keeps_sampling_the_visible_source_subrect() {
        let screen = [1000, 800];
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0));
        let region = callback_region(
            Rect::from_min_max(pos2(-500.0, -250.0), pos2(1500.0, 1250.0)),
            clip,
            1.0,
            screen,
        )
        .expect("the oversized blend rectangle still intersects the window");
        assert_eq!(
            (region.left, region.bottom, region.width, region.height),
            (0, 0, 1000, 800)
        );
        assert!((region.source_uv_min[0] - 0.25).abs() < 1.0e-6);
        assert!((region.source_uv_max[0] - 0.75).abs() < 1.0e-6);
        assert!((region.source_uv_min[1] - (250.0 / 1500.0)).abs() < 1.0e-6);
        assert!((region.source_uv_max[1] - (1050.0 / 1500.0)).abs() < 1.0e-6);
    }

    #[test]
    fn increasing_zoom_narrows_uv_window_instead_of_freezing_q0rg_size() {
        let screen = [1000, 800];
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0));
        let near = callback_region(
            Rect::from_center_size(pos2(500.0, 400.0), egui::vec2(2000.0, 1600.0)),
            clip,
            1.0,
            screen,
        )
        .unwrap();
        let far = callback_region(
            Rect::from_center_size(pos2(500.0, 400.0), egui::vec2(4000.0, 3200.0)),
            clip,
            1.0,
            screen,
        )
        .unwrap();
        let near_u_span = near.source_uv_max[0] - near.source_uv_min[0];
        let far_u_span = far.source_uv_max[0] - far.source_uv_min[0];
        let near_v_span = near.source_uv_max[1] - near.source_uv_min[1];
        let far_v_span = far.source_uv_max[1] - far.source_uv_min[1];
        assert!(far_u_span < near_u_span);
        assert!(far_v_span < near_v_span);
        assert!((near_u_span - 0.5).abs() < 1.0e-6);
        assert!((far_u_span - 0.25).abs() < 1.0e-6);
    }
    fn generic_opaque_reference(back: [f32; 3], src: [f32; 4], mode: BlendMode) -> [f32; 3] {
        let sa = src[3];
        let blend = |back: f32, src: f32| match mode {
            BlendMode::Multiply => back * src,
            BlendMode::Screen => 1.0 - (1.0 - back) * (1.0 - src),
            _ => unreachable!(),
        };
        [
            (1.0 - sa) * back[0] + sa * blend(back[0], src[0]),
            (1.0 - sa) * back[1] + sa * blend(back[1], src[1]),
            (1.0 - sa) * back[2] + sa * blend(back[2], src[2]),
        ]
    }

    fn fixed_opaque_reference(back: [f32; 3], src: [f32; 4], mode: BlendMode) -> [f32; 3] {
        let sp = [src[0] * src[3], src[1] * src[3], src[2] * src[3]];
        match mode {
            BlendMode::Multiply => [
                sp[0] * back[0] + back[0] * (1.0 - src[3]),
                sp[1] * back[1] + back[1] * (1.0 - src[3]),
                sp[2] * back[2] + back[2] * (1.0 - src[3]),
            ],
            BlendMode::Screen => [
                sp[0] + back[0] * (1.0 - sp[0]),
                sp[1] + back[1] * (1.0 - sp[1]),
                sp[2] + back[2] * (1.0 - sp[2]),
            ],
            _ => unreachable!(),
        }
    }

    #[test]
    fn opaque_fast_paths_match_group_blend_math_for_partial_alpha() {
        let back = [0.23, 0.61, 0.82];
        let src = [0.91, 0.37, 0.12, 0.43];
        for mode in [BlendMode::Multiply, BlendMode::Screen] {
            let generic = generic_opaque_reference(back, src, mode);
            let fixed = fixed_opaque_reference(back, src, mode);
            for channel in 0..3 {
                assert!((generic[channel] - fixed[channel]).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn only_multiply_and_screen_skip_the_backdrop_copy_on_opaque_editor_paper() {
        assert!(uses_opaque_fixed_path(BlendMode::Multiply, true));
        assert!(uses_opaque_fixed_path(BlendMode::Screen, true));
        assert!(!uses_opaque_fixed_path(BlendMode::Add, true));
        assert!(!uses_opaque_fixed_path(BlendMode::Overlay, true));
        assert!(!uses_opaque_fixed_path(BlendMode::Multiply, false));
    }
    #[test]
    fn every_persisted_blend_mode_has_a_stable_shader_tag() {
        assert_eq!(blend_mode_tag(BlendMode::Normal), 0);
        assert_eq!(blend_mode_tag(BlendMode::Multiply), 1);
        assert_eq!(blend_mode_tag(BlendMode::Screen), 2);
        assert_eq!(blend_mode_tag(BlendMode::Add), 3);
        assert_eq!(blend_mode_tag(BlendMode::Overlay), 4);
    }
}
