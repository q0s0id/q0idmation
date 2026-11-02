//! WebGL2 offscreen compositing. Uses the desktop shaders and keeps all mask,
//! group-opacity and blend operations on the GPU at the canvas resolution.
use super::*;
use std::cell::RefCell;

thread_local! {
    static RESOURCES: RefCell<HashMap<usize, Option<Resources>>> = RefCell::new(HashMap::new());
    static ERROR: RefCell<Option<String>> = RefCell::new(None);
}

pub(super) fn take_error() -> Option<String> {
    ERROR.with(|error| error.borrow_mut().take())
}
pub(super) fn paint(
    painter: &Painter,
    rect: Rect,
    masks: Vec<Vec<VectorGroupPrimitive>>,
    placements: Vec<VectorMaskedPlacement>,
) {
    let masks = Arc::new(masks);
    let placements = Arc::new(placements);
    let callback = egui_glow::CallbackFn::new(move |info, painter| {
        let Some(region) = callback_region(
            info.viewport,
            info.clip_rect,
            info.pixels_per_point,
            info.screen_size_px,
        ) else {
            return;
        };
        with_resources(painter.gl(), |resources| unsafe {
            resources.paint(
                painter,
                &masks,
                &placements,
                region,
                info.pixels_per_point,
                info.screen_size_px,
            );
        });
    });
    painter.add(Shape::Callback(PaintCallback {
        rect,
        callback: Arc::new(callback),
    }));
}

pub(super) fn gradient(painter: &Painter, rect: Rect, mesh: Arc<Mesh>, paint: GradientMeshPaint) {
    let callback = egui_glow::CallbackFn::new(move |info, painter| {
        let Some(region) = callback_region(
            info.viewport,
            info.clip_rect,
            info.pixels_per_point,
            info.screen_size_px,
        ) else {
            return;
        };
        let gl = painter.gl();
        with_resources(gl, |resources| unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(
                0,
                0,
                info.screen_size_px[0] as i32,
                info.screen_size_px[1] as i32,
            );
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(region.left, region.bottom, region.width, region.height);
            resources.blending(gl);
            resources.gradient_mesh(
                gl,
                &mesh,
                &paint,
                [
                    info.screen_size_px[0] as f32 / info.pixels_per_point,
                    info.screen_size_px[1] as f32 / info.pixels_per_point,
                ],
                [0.0; 2],
                false,
            );
        });
    });
    painter.add(Shape::Callback(PaintCallback {
        rect,
        callback: Arc::new(callback),
    }));
}

fn with_resources(gl: &Arc<glow::Context>, draw: impl FnOnce(&mut Resources)) {
    RESOURCES.with(|all| {
        let mut all = all.borrow_mut();
        let entry = all
            .entry(Arc::as_ptr(gl) as usize)
            .or_insert_with(|| unsafe {
                Resources::new(gl)
                    .map_err(|error| {
                        ERROR.with(|slot| {
                            *slot.borrow_mut() = Some(format!("webgl compositor: {error}"))
                        })
                    })
                    .ok()
            });
        if let Some(resources) = entry {
            draw(resources);
        }
    });
}

struct Target {
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
}
struct GradientBuffers {
    source: std::sync::Weak<Mesh>,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
}
struct Resources {
    solid: glow::Program,
    textured: glow::Program,
    multiply: glow::Program,
    composite: glow::Program,
    gradient: glow::Program,
    stops: glow::Texture,
    stop_data: Vec<f32>,
    gradients: HashMap<usize, GradientBuffers>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    targets: Vec<Target>,
    msaa: glow::Framebuffer,
    color: glow::Renderbuffer,
    size: (i32, i32),
    backdrop_size: (i32, i32),
    samples: i32,
}

unsafe fn bytes<T>(data: &[T]) -> &[u8] {
    std::slice::from_raw_parts(data.as_ptr().cast(), std::mem::size_of_val(data))
}

impl Resources {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let mut targets = Vec::new();
        for _ in 0..4 {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::NEAREST as i32);
            }
            for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::CLAMP_TO_EDGE as i32);
            }
            targets.push(Target {
                texture,
                framebuffer: gl.create_framebuffer()?,
            });
        }
        let stops = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(stops));
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
        Ok(Self {
            solid: create_group_program(gl)?,
            textured: create_group_textured_program(gl)?,
            multiply: create_mask_apply_program(gl)?,
            composite: create_program(gl)?,
            gradient: create_gradient_program(gl)?,
            stops,
            stop_data: vec![0.0; 256 * 8],
            gradients: HashMap::new(),
            vao: gl.create_vertex_array()?,
            vbo: gl.create_buffer()?,
            ebo: gl.create_buffer()?,
            targets,
            msaa: gl.create_framebuffer()?,
            color: gl.create_renderbuffer()?,
            size: (0, 0),
            backdrop_size: (0, 0),
            samples: gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(1, 4),
        })
    }

    unsafe fn ensure_size(&mut self, gl: &glow::Context, width: i32, height: i32) {
        if self.size.0 >= width && self.size.1 >= height {
            return;
        }
        self.size = (
            self.size.0.max((width + 127) / 128 * 128),
            self.size.1.max((height + 127) / 128 * 128),
        );
        gl.active_texture(glow::TEXTURE0);
        for target in self.targets.iter().take(3) {
            gl.bind_texture(glow::TEXTURE_2D, Some(target.texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                self.size.0,
                self.size.1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(target.texture),
                0,
            );
        }
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(self.color));
        gl.renderbuffer_storage_multisample(
            glow::RENDERBUFFER,
            self.samples,
            glow::RGBA8,
            self.size.0,
            self.size.1,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.msaa));
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::RENDERBUFFER,
            Some(self.color),
        );
    }

    unsafe fn ensure_backdrop(&mut self, gl: &glow::Context, screen: [u32; 2]) {
        let size = (screen[0] as i32, screen[1] as i32);
        if self.backdrop_size == size {
            return;
        }
        self.backdrop_size = size;
        let target = &self.targets[3];
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(target.texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            size.0,
            size.1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(target.texture),
            0,
        );
    }

    unsafe fn blending(&self, gl: &glow::Context) {
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.enable(glow::BLEND);
        gl.blend_equation(glow::FUNC_ADD);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
    }

    unsafe fn group(
        &mut self,
        painter: &egui_glow::Painter,
        primitives: &[VectorGroupPrimitive],
        slot: usize,
        region: CallbackRegion,
        ppp: f32,
        screen: [u32; 2],
    ) {
        let gl = painter.gl();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.msaa));
        gl.viewport(0, 0, region.width, region.height);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(0, 0, region.width, region.height);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        self.blending(gl);
        let origin = [
            region.left as f32,
            screen[1] as f32 - (region.bottom + region.height) as f32,
        ];
        for primitive in primitives {
            if let VectorGroupPrimitive::Gradient { mesh, paint } = primitive {
                self.gradient_mesh(
                    gl,
                    mesh,
                    paint,
                    [region.width as f32 / ppp, region.height as f32 / ppp],
                    [origin[0] / ppp, origin[1] / ppp],
                    true,
                );
                continue;
            }
            let (mesh, effect, textured) = match primitive {
                VectorGroupPrimitive::Solid(mesh) => (mesh, ColorTransform::default(), false),
                VectorGroupPrimitive::Textured {
                    mesh,
                    color_transform,
                } => (mesh.as_ref(), *color_transform, true),
                VectorGroupPrimitive::Gradient { .. } => unreachable!(),
            };
            let program = if textured { self.textured } else { self.solid };
            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
            let mut vertices =
                Vec::with_capacity(mesh.vertices.len() * if textured { 8 } else { 6 });
            for vertex in &mesh.vertices {
                vertices.extend_from_slice(&[vertex.pos.x, vertex.pos.y]);
                if textured {
                    vertices.extend_from_slice(&[vertex.uv.x, vertex.uv.y]);
                }
                vertices.extend(vertex.color.to_array().map(|c| f32::from(c) / 255.0));
            }
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&vertices), glow::STREAM_DRAW);
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytes(&mesh.indices),
                glow::STREAM_DRAW,
            );
            let stride = if textured { 32 } else { 24 };
            gl.enable_vertex_attrib_array(0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.vertex_attrib_pointer_f32(
                1,
                if textured { 2 } else { 4 },
                glow::FLOAT,
                false,
                stride,
                8,
            );
            if textured {
                gl.enable_vertex_attrib_array(2);
                gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, stride, 16);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, painter.texture(mesh.texture_id));
                gl.uniform_1_i32(gl.get_uniform_location(program, "u_texture").as_ref(), 0);
                gl.uniform_1_i32(
                    gl.get_uniform_location(program, "u_srgb_texture").as_ref(),
                    1,
                );
                gl.uniform_3_f32_slice(
                    gl.get_uniform_location(program, "u_color_scale").as_ref(),
                    &effect.scale,
                );
                gl.uniform_3_f32_slice(
                    gl.get_uniform_location(program, "u_color_offset").as_ref(),
                    &effect.offset,
                );
            } else {
                gl.disable_vertex_attrib_array(2);
            }
            gl.uniform_1_f32(
                gl.get_uniform_location(program, "u_pixels_per_point")
                    .as_ref(),
                ppp,
            );
            gl.uniform_2_f32(
                gl.get_uniform_location(program, "u_region_origin_px")
                    .as_ref(),
                origin[0],
                origin[1],
            );
            gl.uniform_2_f32(
                gl.get_uniform_location(program, "u_region_size_px")
                    .as_ref(),
                region.width as f32,
                region.height as f32,
            );
            gl.draw_elements(
                glow::TRIANGLES,
                mesh.indices.len() as i32,
                glow::UNSIGNED_INT,
                0,
            );
        }
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.msaa));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.targets[slot].framebuffer));
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

    unsafe fn multiply(
        &self,
        gl: &glow::Context,
        source: usize,
        mask: usize,
        output: usize,
        region: CallbackRegion,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.targets[output].framebuffer));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::BLEND);
        gl.use_program(Some(self.multiply));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[source].texture));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[mask].texture));
        gl.uniform_1_i32(
            gl.get_uniform_location(self.multiply, "u_source").as_ref(),
            0,
        );
        gl.uniform_1_i32(gl.get_uniform_location(self.multiply, "u_mask").as_ref(), 1);
        gl.uniform_2_f32(
            gl.get_uniform_location(self.multiply, "u_texture_size")
                .as_ref(),
            self.size.0 as f32,
            self.size.1 as f32,
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    unsafe fn composite(
        &self,
        gl: &glow::Context,
        slot: usize,
        placement: &VectorMaskedPlacement,
        region: CallbackRegion,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[3].texture));
        let fixed = matches!(
            placement.blend_mode,
            BlendMode::Normal | BlendMode::Multiply | BlendMode::Screen
        );
        if !fixed {
            // ES3 requires identical source/destination coordinates when resolving
            // MSAA. Keep the backdrop in screen coordinates; copy only this region.
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.targets[3].framebuffer));
            gl.blit_framebuffer(
                region.left,
                region.bottom,
                region.left + region.width,
                region.bottom + region.height,
                region.left,
                region.bottom,
                region.left + region.width,
                region.bottom + region.height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.disable(glow::BLEND);
        gl.disable(glow::SCISSOR_TEST);
        if fixed {
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            let (source, destination) = match placement.blend_mode {
                BlendMode::Multiply => (glow::DST_COLOR, glow::ONE_MINUS_SRC_ALPHA),
                BlendMode::Screen => (glow::ONE, glow::ONE_MINUS_SRC_COLOR),
                _ => (glow::ONE, glow::ONE_MINUS_SRC_ALPHA),
            };
            gl.blend_func_separate(source, destination, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
        let p = self.composite;
        gl.use_program(Some(p));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[slot].texture));
        for (name, value) in [
            ("u_source", 0),
            ("u_backdrop", 1),
            ("u_mode", blend_mode_tag(placement.blend_mode)),
            ("u_passthrough", i32::from(fixed)),
            ("u_source_srgb", 0),
        ] {
            gl.uniform_1_i32(gl.get_uniform_location(p, name).as_ref(), value);
        }
        let uv = [
            region.width as f32 / self.size.0 as f32,
            region.height as f32 / self.size.1 as f32,
        ];
        gl.uniform_2_f32(
            gl.get_uniform_location(p, "u_source_uv_min").as_ref(),
            0.0,
            0.0,
        );
        gl.uniform_2_f32(
            gl.get_uniform_location(p, "u_source_uv_max").as_ref(),
            uv[0],
            uv[1],
        );
        gl.uniform_2_f32(
            gl.get_uniform_location(p, "u_backdrop_uv_max").as_ref(),
            region.width as f32 / self.backdrop_size.0 as f32,
            region.height as f32 / self.backdrop_size.1 as f32,
        );
        gl.uniform_2_f32(
            gl.get_uniform_location(p, "u_backdrop_uv_min").as_ref(),
            region.left as f32 / self.backdrop_size.0 as f32,
            region.bottom as f32 / self.backdrop_size.1 as f32,
        );
        gl.uniform_1_f32(
            gl.get_uniform_location(p, "u_opacity").as_ref(),
            placement.opacity.clamp(0.0, 1.0),
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    unsafe fn paint(
        &mut self,
        painter: &egui_glow::Painter,
        masks: &[Vec<VectorGroupPrimitive>],
        placements: &[VectorMaskedPlacement],
        region: CallbackRegion,
        ppp: f32,
        screen: [u32; 2],
    ) {
        let gl = painter.gl();
        self.ensure_size(gl, region.width, region.height);
        self.ensure_backdrop(gl, screen);
        let mut combined = 0;
        if let Some(first) = masks.first() {
            self.group(painter, first, 0, region, ppp, screen);
            for mask in masks.iter().skip(1) {
                self.group(painter, mask, 1, region, ppp, screen);
                let output = if combined == 0 { 2 } else { 0 };
                self.multiply(gl, combined, 1, output, region);
                combined = output;
            }
        }
        for placement in placements {
            self.group(painter, &placement.primitives, 1, region, ppp, screen);
            let output = if masks.is_empty() {
                1
            } else {
                let output = if combined == 0 { 2 } else { 0 };
                self.multiply(gl, 1, combined, output, region);
                output
            };
            self.composite(gl, output, placement, region);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(None);
    }

    unsafe fn gradient_mesh(
        &mut self,
        gl: &glow::Context,
        mesh: &Arc<Mesh>,
        paint: &GradientMeshPaint,
        size: [f32; 2],
        offset: [f32; 2],
        offscreen: bool,
    ) {
        let key = Arc::as_ptr(mesh) as usize;
        let reusable = self
            .gradients
            .get(&key)
            .and_then(|b| b.source.upgrade())
            .is_some_and(|source| Arc::ptr_eq(&source, mesh));
        if !reusable {
            if self.gradients.len() >= 64 {
                for (_, b) in self.gradients.drain() {
                    gl.delete_buffer(b.vbo);
                    gl.delete_buffer(b.ebo);
                }
            }
            if let Some(b) = self.gradients.remove(&key) {
                gl.delete_buffer(b.vbo);
                gl.delete_buffer(b.ebo);
            }
            let b = GradientBuffers {
                source: Arc::downgrade(mesh),
                vbo: gl.create_buffer().expect("gradient vertices"),
                ebo: gl.create_buffer().expect("gradient indices"),
            };
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(b.vbo));
            let positions: Vec<f32> = mesh
                .vertices
                .iter()
                .flat_map(|v| [v.pos.x, v.pos.y])
                .collect();
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&positions), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(b.ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytes(&mesh.indices),
                glow::STATIC_DRAW,
            );
            self.gradients.insert(key, b);
        }
        for (i, stop) in paint.stops.iter().take(256).enumerate() {
            self.stop_data[i * 4] = stop.position.clamp(0.0, 1.0);
            self.stop_data[1024 + i * 4..1028 + i * 4].copy_from_slice(
                &[stop.color.r, stop.color.g, stop.color.b, stop.color.a]
                    .map(|v| f32::from(v) / 255.0),
            );
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.stops));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            256,
            2,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(bytes(&self.stop_data)),
        );
        let p = self.gradient;
        gl.use_program(Some(p));
        gl.bind_vertex_array(Some(self.vao));
        let b = &self.gradients[&key];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(b.vbo));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(b.ebo));
        gl.enable_vertex_attrib_array(0);
        gl.disable_vertex_attrib_array(1);
        gl.disable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
        for (name, value) in [
            ("u_screen_size", size),
            ("u_screen_offset", offset),
            (
                "u_source_origin",
                [
                    paint.transform.source_origin.x,
                    paint.transform.source_origin.y,
                ],
            ),
            (
                "u_target_origin",
                [
                    paint.transform.target_origin.x,
                    paint.transform.target_origin.y,
                ],
            ),
            (
                "u_gradient_inverse_translation",
                [paint.gradient_inverse[4], paint.gradient_inverse[5]],
            ),
        ] {
            gl.uniform_2_f32(
                gl.get_uniform_location(p, name).as_ref(),
                value[0],
                value[1],
            );
        }
        for (name, value) in [
            ("u_offscreen", i32::from(offscreen)),
            (
                "u_gradient_kind",
                i32::from(paint.kind == GradientKind::Radial),
            ),
            ("u_stop_count", paint.stops.len().min(256) as i32),
            ("u_stops", 0),
            ("u_additive", i32::from(paint.additive)),
        ] {
            gl.uniform_1_i32(gl.get_uniform_location(p, name).as_ref(), value);
        }
        gl.uniform_4_f32_slice(
            gl.get_uniform_location(p, "u_linear").as_ref(),
            &paint.transform.linear,
        );
        gl.uniform_4_f32_slice(
            gl.get_uniform_location(p, "u_gradient_inverse_linear")
                .as_ref(),
            &paint.gradient_inverse[..4],
        );
        gl.uniform_4_f32_slice(
            gl.get_uniform_location(p, "u_tint").as_ref(),
            &paint.tint.to_array().map(|c| f32::from(c) / 255.0),
        );
        gl.uniform_3_f32_slice(
            gl.get_uniform_location(p, "u_color_scale").as_ref(),
            &paint.color_transform.scale,
        );
        gl.uniform_3_f32_slice(
            gl.get_uniform_location(p, "u_color_offset").as_ref(),
            &paint.color_transform.offset,
        );
        gl.draw_elements(
            glow::TRIANGLES,
            mesh.indices.len() as i32,
            glow::UNSIGNED_INT,
            0,
        );
    }
}
