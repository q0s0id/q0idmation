use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui::{pos2, Color32, Mesh, PaintCallback, Painter, Rect, Shape, TextureId};
use glow::HasContext as _;
use q0s_format::v2::BlendMode;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static GL_RESOURCES: RefCell<HashMap<usize, GlResources>> = RefCell::new(HashMap::new());
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
    if meshes.is_empty() {
        return;
    }
    if blend_mode == BlendMode::Normal {
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
        let fixed_opaque = uses_opaque_fixed_path(blend_mode, true);
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
                        region,
                        info.pixels_per_point,
                        info.screen_size_px,
                    );
                }
            });
        });
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
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
                        );
                    } else {
                        resources.paint(gl, source_texture, mode, region, resources.source_srgb);
                    }
                }
            });
        });
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
        gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, 4, glow::RGBA8, width, height);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.group_msaa_fbo));
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::RENDERBUFFER,
            Some(self.group_msaa_color),
        );
        self.group_size = capacity;
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_solid_group(
        &mut self,
        gl: &glow::Context,
        meshes: &[Mesh],
        mode: i32,
        fixed_opaque: bool,
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
            self.paint_opaque_fixed(gl, self.group_texture, mode, group_region, false);
        } else {
            self.paint(gl, self.group_texture, mode, group_region, false);
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
    ) {
        debug_assert!(
            mode == blend_mode_tag(BlendMode::Multiply)
                || mode == blend_mode_tag(BlendMode::Screen)
        );
        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        match mode {
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
    float sa = source.a;
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

#[cfg(test)]
mod tests {
    use super::*;

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
