//! Lights on the GPU: one pass multiplies what is drawn by the light that
//! reaches it, a second adds the glows. The shader follows
//! `q0s_format::lighting::shade`, so the editor, q0player and exports agree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use egui::{PaintCallback, Painter, Rect, Shape};
use glow::HasContext as _;
use q0s_format::lighting::{FrameLighting, MAX_GAIN, MAX_LIGHTS};

/// A frame's lighting as the GPU draws it.
#[derive(Debug, Clone)]
pub struct LightingPaint {
    /// Screen points to the lit timeline's own space: a11, a12, tx, then
    /// a21, a22, ty.
    pub screen_to_world: [[f32; 3]; 2],
    pub lighting: FrameLighting,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static PROGRAMS: RefCell<HashMap<usize, Option<LightingProgram>>> = RefCell::new(HashMap::new());
}

/// Lights everything painted so far inside `rect`.
pub fn paint_lighting(painter: &Painter, rect: Rect, paint: LightingPaint) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (painter, rect, paint);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rect = rect.intersect(painter.clip_rect());
        if !rect.is_positive() {
            return;
        }
        let paint = Arc::new(paint);
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(region) = crate::callback_region(
                info.viewport,
                info.clip_rect,
                info.pixels_per_point,
                info.screen_size_px,
            ) else {
                return;
            };
            let gl = glow_painter.gl();
            let key = Arc::as_ptr(gl) as usize;
            PROGRAMS.with(|programs| {
                let mut programs = programs.borrow_mut();
                let program = programs.entry(key).or_insert_with(|| {
                    // SAFETY: called on the GL thread inside egui's paint
                    // callback, with its context current.
                    unsafe { LightingProgram::new(gl) }
                        .map_err(|error| eprintln!("lighting shader: {error}"))
                        .ok()
                });
                if let Some(program) = program {
                    // SAFETY: as above.
                    unsafe {
                        program.draw(
                            gl,
                            &paint,
                            region,
                            info.pixels_per_point,
                            info.screen_size_px,
                        )
                    };
                }
            });
        });
        painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct LightingProgram {
    program: glow::Program,
    vao: glow::VertexArray,
    shade: Option<glow::UniformLocation>,
    light_a: Option<glow::UniformLocation>,
    light_b: Option<glow::UniformLocation>,
    light_c: Option<glow::UniformLocation>,
    count: Option<glow::UniformLocation>,
    world_x: Option<glow::UniformLocation>,
    world_y: Option<glow::UniformLocation>,
    ppp: Option<glow::UniformLocation>,
    screen_h: Option<glow::UniformLocation>,
    pass: Option<glow::UniformLocation>,
    max_gain: Option<glow::UniformLocation>,
}

#[cfg(not(target_arch = "wasm32"))]
const VERTEX: &str = r#"#version 140
void main() {
    vec2 p;
    if (gl_VertexID == 0) p = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) p = vec2(1.0, -1.0);
    else if (gl_VertexID == 2) p = vec2(-1.0, 1.0);
    else p = vec2(1.0, 1.0);
    gl_Position = vec4(p, 0.0, 1.0);
}
"#;

#[cfg(not(target_arch = "wasm32"))]
const FRAGMENT: &str = r#"#version 140
uniform vec3 u_shade;
uniform vec4 u_light_a[32];
uniform vec4 u_light_b[32];
uniform vec4 u_light_c[32];
uniform int u_count;
uniform vec3 u_world_x;
uniform vec3 u_world_y;
uniform float u_ppp;
uniform float u_screen_h;
uniform int u_pass;
uniform float u_max_gain;
out vec4 out_color;

float edge(float e0, float e1, float x) {
    if (e1 <= e0) return x < e0 ? 0.0 : 1.0;
    float t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

void main() {
    vec3 screen = vec3(gl_FragCoord.x / u_ppp, (u_screen_h - gl_FragCoord.y) / u_ppp, 1.0);
    vec2 p = vec2(dot(u_world_x, screen), dot(u_world_y, screen));
    vec3 gain = u_shade;
    vec3 added = vec3(0.0);
    for (int i = 0; i < 32; ++i) {
        if (i >= u_count) break;
        vec4 a = u_light_a[i];
        vec4 b = u_light_b[i];
        vec4 c = u_light_c[i];
        vec2 d = p - a.xy;
        float dist = length(d);
        float radius = a.z;
        if (dist >= radius) continue;
        float soft = max(a.w, 0.02);
        float reach = 1.0 - edge(radius * (1.0 - soft), radius, dist);
        if (c.w > 0.5) {
            float off = atan(d.y, d.x) - c.x;
            off = mod(off + 3.14159265, 6.28318531) - 3.14159265;
            float cone_soft = max(c.z, 0.02);
            reach *= 1.0 - edge(c.y * (1.0 - cone_soft), c.y, abs(off));
        }
        gain += b.rgb * reach;
        float halo = radius * 0.45;
        if (b.w > 0.0 && dist < halo) {
            float t = 1.0 - dist / halo;
            added += b.rgb * b.w * t * t;
        }
    }
    gain = clamp(gain, 0.0, u_max_gain);
    added = clamp(added, 0.0, 1.0);
    if (u_pass == 0) {
        // Blended as source * destination * 2, so 1 leaves it as drawn.
        out_color = vec4(gain * 0.5, 1.0);
    } else {
        out_color = vec4(added, max(added.r, max(added.g, added.b)));
    }
}
"#;

#[cfg(not(target_arch = "wasm32"))]
impl LightingProgram {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let compile = |kind: u32, source: &str| -> Result<glow::Shader, String> {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &crate::glsl_for(gl, source));
            gl.compile_shader(shader);
            if gl.get_shader_compile_status(shader) {
                Ok(shader)
            } else {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                Err(log)
            }
        };
        let vertex = compile(glow::VERTEX_SHADER, VERTEX)?;
        let fragment = match compile(glow::FRAGMENT_SHADER, FRAGMENT) {
            Ok(fragment) => fragment,
            Err(error) => {
                gl.delete_shader(vertex);
                return Err(error);
            }
        };
        let program = gl.create_program()?;
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
        let vao = gl.create_vertex_array()?;
        let at = |name: &str| gl.get_uniform_location(program, name);
        Ok(Self {
            program,
            vao,
            shade: at("u_shade"),
            light_a: at("u_light_a[0]").or_else(|| at("u_light_a")),
            light_b: at("u_light_b[0]").or_else(|| at("u_light_b")),
            light_c: at("u_light_c[0]").or_else(|| at("u_light_c")),
            count: at("u_count"),
            world_x: at("u_world_x"),
            world_y: at("u_world_y"),
            ppp: at("u_ppp"),
            screen_h: at("u_screen_h"),
            pass: at("u_pass"),
            max_gain: at("u_max_gain"),
        })
    }

    unsafe fn draw(
        &self,
        gl: &glow::Context,
        paint: &LightingPaint,
        region: crate::CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
    ) {
        let lighting = &paint.lighting;
        let count = lighting.lights.len().min(MAX_LIGHTS);
        let mut a = vec![0.0f32; MAX_LIGHTS * 4];
        let mut b = vec![0.0f32; MAX_LIGHTS * 4];
        let mut c = vec![0.0f32; MAX_LIGHTS * 4];
        for (index, light) in lighting.lights.iter().take(count).enumerate() {
            let at = index * 4;
            a[at..at + 4].copy_from_slice(&[
                light.position[0],
                light.position[1],
                light.radius,
                light.softness,
            ]);
            b[at..at + 4].copy_from_slice(&[
                light.color[0],
                light.color[1],
                light.color[2],
                light.glow,
            ]);
            if let Some(spot) = light.spot {
                c[at..at + 4].copy_from_slice(&[
                    spot.direction,
                    spot.half_cone,
                    spot.softness,
                    1.0,
                ]);
            }
        }
        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.color_mask(true, true, true, true);
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.uniform_3_f32(
            self.shade.as_ref(),
            lighting.shade[0],
            lighting.shade[1],
            lighting.shade[2],
        );
        gl.uniform_4_f32_slice(self.light_a.as_ref(), &a);
        gl.uniform_4_f32_slice(self.light_b.as_ref(), &b);
        gl.uniform_4_f32_slice(self.light_c.as_ref(), &c);
        gl.uniform_1_i32(self.count.as_ref(), count as i32);
        let [x, y] = paint.screen_to_world;
        gl.uniform_3_f32(self.world_x.as_ref(), x[0], x[1], x[2]);
        gl.uniform_3_f32(self.world_y.as_ref(), y[0], y[1], y[2]);
        gl.uniform_1_f32(self.ppp.as_ref(), pixels_per_point.max(f32::EPSILON));
        gl.uniform_1_f32(self.screen_h.as_ref(), screen_size_px[1] as f32);
        gl.uniform_1_f32(self.max_gain.as_ref(), MAX_GAIN);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        // Multiply: source * destination + destination * source.
        gl.blend_func_separate(glow::DST_COLOR, glow::SRC_COLOR, glow::ZERO, glow::ONE);
        gl.uniform_1_i32(self.pass.as_ref(), 0);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        if lighting
            .lights
            .iter()
            .any(|light| light.glow > 0.0 && light.color.iter().any(|channel| *channel > 0.0))
        {
            // Glows add their light, covering transparent places too.
            gl.blend_func_separate(glow::ONE, glow::ONE, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.uniform_1_i32(self.pass.as_ref(), 1);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
        gl.bind_vertex_array(None);
    }
}
