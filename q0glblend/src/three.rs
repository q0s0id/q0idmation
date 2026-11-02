//! 3D on the GPU: meshes and planes drawn with a depth buffer into an
//! offscreen, multisampled target, then laid over what egui has painted.
//! The shader lights like `q0s_format::three::draw::shade`, so the editor,
//! q0player and software exports agree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Weak};

use egui::{PaintCallback, Painter, Rect, Shape};
use glow::HasContext as _;
use q0s_format::three::draw::{draw_passes, DrawItem, Frame3d, View3d};
use q0s_format::three::math::{Mat4, V3};
use q0s_format::three::model::{RenderMesh, Shading};
use q0s_format::three::scene::Light3dKind;
use q0s_format::three::MAX_LIGHTS_3D;
use q0s_format::transform::Affine;

thread_local! {
    static RENDERERS: RefCell<HashMap<usize, Option<Renderer>>> = RefCell::new(HashMap::new());
}

/// Draws `frame` over everything painted so far inside `rect`. The frame's
/// view maps stage space to screen points.
pub fn paint_frame3d(painter: &Painter, rect: Rect, frame: Arc<Frame3d>) {
    let rect = rect.intersect(painter.clip_rect());
    if !rect.is_positive() || frame.items.is_empty() {
        return;
    }
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
        RENDERERS.with(|renderers| {
            let mut renderers = renderers.borrow_mut();
            let renderer = renderers.entry(key).or_insert_with(|| {
                // SAFETY: called on the GL thread inside egui's paint
                // callback, with its context current.
                unsafe { Renderer::new(gl) }
                    .map_err(|error| eprintln!("3D shader: {error}"))
                    .ok()
            });
            if let Some(renderer) = renderer {
                // SAFETY: as above.
                unsafe {
                    renderer.draw(
                        gl,
                        &frame,
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

const MESH_VERTEX: &str = r#"#version 140
in vec3 a_pos;
in vec3 a_normal;
in vec2 a_uv;
in vec4 a_color;
in vec3 a_smooth;
uniform mat4 u_model;
uniform mat4 u_normal_mat;
uniform mat4 u_view;
uniform mat4 u_clip;
uniform float u_outline;
uniform vec2 u_outline_k;
out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_color;
void main() {
    vec4 world = u_model * vec4(a_pos, 1.0);
    vec3 n = normalize((u_normal_mat * vec4(a_normal, 0.0)).xyz);
    if (u_outline > 0.0) {
        vec3 s = normalize((u_normal_mat * vec4(a_smooth, 0.0)).xyz);
        float z = (u_view * world).z;
        world.xyz += s * (u_outline_k.x * max(z, 0.0) + u_outline_k.y);
    }
    v_world = world.xyz;
    v_normal = n;
    v_uv = a_uv;
    v_color = a_color;
    gl_Position = u_clip * (u_view * world);
}
"#;

const MESH_FRAGMENT: &str = r#"#version 140
in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_color;
uniform vec4 u_base;
uniform int u_has_tex;
uniform int u_tex_premul;
uniform sampler2D u_tex;
uniform int u_vertex_colors;
uniform int u_lit;
uniform int u_shading;
uniform float u_bands;
uniform float u_rough;
uniform float u_metal;
uniform vec3 u_emissive;
uniform float u_cutoff;
uniform float u_opacity;
uniform int u_depth_only;
uniform vec3 u_ambient;
uniform int u_light_count;
uniform vec4 u_la[16];
uniform vec4 u_lb[16];
uniform vec4 u_lc[16];
uniform vec4 u_ld[16];
uniform vec3 u_eye;
uniform vec4 u_fog;
uniform vec2 u_fog_range;
uniform int u_ortho;
uniform vec3 u_forward;
uniform mat4 u_shadow_mat0;
uniform mat4 u_shadow_mat1;
uniform sampler2D u_shadow_tex0;
uniform sampler2D u_shadow_tex1;
uniform int u_shadow_light0;
uniform int u_shadow_light1;
uniform float u_shadow_texel0;
uniform float u_shadow_texel1;
out vec4 out_color;

// How much of a light reaches past its shadow map, with a 3x3 filter.
float shadow_seen(sampler2D map, mat4 m, vec3 world, vec3 n, float texel) {
    vec4 c = m * vec4(world + n * texel * 1.5, 1.0);
    if (c.w <= 1e-6) return 1.0;
    vec3 p = c.xyz / c.w;
    if (abs(p.x) > 1.0 || abs(p.y) > 1.0 || p.z > 1.0) return 1.0;
    vec2 uv = p.xy * 0.5 + 0.5;
    float z = p.z * 0.5 + 0.5;
    float size = float(textureSize(map, 0).x);
    float lit = 0.0;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            float d = texture(map, uv + vec2(float(x), float(y)) / size).r;
            lit += (z - 0.0005 <= d) ? 1.0 : 0.0;
        }
    }
    return lit / 9.0;
}

float step_light(float l, float bands) {
    float x = clamp(l, 0.0, 1.0);
    return min(floor(x * bands) / (bands - 1.0), 1.0);
}

float edge(float e0, float e1, float x) {
    if (e1 <= e0) return x < e0 ? 0.0 : 1.0;
    float t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

void main() {
    vec4 rgba = u_base;
    if (u_has_tex != 0) {
        vec4 t = texture(u_tex, v_uv);
        if (u_tex_premul == 0) t.rgb *= t.a;
        rgba *= t;
    }
    if (u_vertex_colors != 0) {
        rgba *= vec4(v_color.rgb * v_color.a, v_color.a);
    }
    if (u_cutoff >= 0.0) {
        if (rgba.a / max(u_opacity, 1e-6) < u_cutoff) discard;
        rgba *= u_opacity / max(rgba.a, 1e-6);
    }
    if (rgba.a <= 1.0 / 512.0) discard;
    if (u_depth_only != 0) {
        if (rgba.a < 0.5) discard;
        out_color = vec4(0.0);
        return;
    }
    if (u_lit != 0 && u_shading == 3) {
        // Glass: a little of its own colour, its surroundings mirrored more
        // toward its edges, sharp highlights.
        vec3 to_eye = u_ortho != 0 ? -u_forward : normalize(u_eye - v_world);
        vec3 n = normalize(v_normal);
        if (dot(n, to_eye) < 0.0) n = -n;
        float body = clamp(rgba.a, 0.0, 1.0);
        vec3 tint = body > 1e-4 ? rgba.rgb / body : vec3(1.0);
        float cosv = clamp(dot(n, to_eye), 0.0, 1.0);
        float fres = 0.04 + 0.96 * pow(1.0 - cosv, 5.0);
        vec3 r = reflect(-to_eye, n);
        float up = clamp(-r.y * 0.5 + 0.5, 0.0, 1.0);
        vec3 around = mix(vec3(0.10, 0.11, 0.13), vec3(0.92, 0.95, 1.0), up * up);
        vec3 light = u_ambient;
        vec3 shine = vec3(0.0);
        for (int i = 0; i < 16; ++i) {
            if (i >= u_light_count) break;
            vec4 a = u_la[i];
            vec4 b = u_lb[i];
            vec4 c = u_lc[i];
            vec3 l;
            float reach = 1.0;
            if (a.w > 1.5) {
                l = -b.xyz;
            } else {
                vec3 off = a.xyz - v_world;
                float dist = length(off);
                l = dist > 1e-6 ? off / dist : vec3(0.0, 0.0, -1.0);
                if (b.w > 0.0) {
                    float f = min(dist / b.w, 1.0);
                    float w = max(1.0 - f * f * f * f, 0.0);
                    reach = w * w;
                }
                if (a.w > 0.5) {
                    reach *= edge(c.w, u_ld[i].x, dot(-l, b.xyz));
                }
            }
            if (reach <= 0.0) continue;
            vec3 h = normalize(l + to_eye);
            light += c.rgb * reach * max(dot(n, l), 0.0) * 0.25;
            shine += c.rgb * reach * pow(max(dot(n, h), 0.0), 180.0) * 1.6;
        }
        float glint = max(max(shine.r, shine.g), shine.b);
        float alpha = clamp(body + fres * (1.0 - body) + glint, 0.0, 1.0);
        rgba = vec4(min(tint * body * light + around * fres + shine, vec3(alpha)), alpha);
    } else if (u_lit != 0 && u_shading != 2) {
        vec3 to_eye = u_ortho != 0 ? -u_forward : normalize(u_eye - v_world);
        vec3 n = normalize(v_normal);
        if (dot(n, to_eye) < 0.0) n = -n;
        vec3 diffuse = u_ambient;
        vec3 shine = vec3(0.0);
        float rough = clamp(u_rough, 0.02, 1.0);
        float expo = pow(2.0, 1.0 + 10.0 * (1.0 - rough));
        float strength = (0.04 + 0.96 * u_metal) * (1.0 - rough) * (1.0 - rough) * 2.0;
        bool toon = u_shading == 1;
        for (int i = 0; i < 16; ++i) {
            if (i >= u_light_count) break;
            vec4 a = u_la[i];
            vec4 b = u_lb[i];
            vec4 c = u_lc[i];
            vec3 l;
            float reach = 1.0;
            if (a.w > 1.5) {
                l = -b.xyz;
            } else {
                vec3 off = a.xyz - v_world;
                float dist = length(off);
                l = dist > 1e-6 ? off / dist : vec3(0.0, 0.0, -1.0);
                if (b.w > 0.0) {
                    float f = min(dist / b.w, 1.0);
                    float w = max(1.0 - f * f * f * f, 0.0);
                    reach = w * w;
                }
                if (a.w > 0.5) {
                    reach *= edge(c.w, u_ld[i].x, dot(-l, b.xyz));
                }
            }
            if (reach <= 0.0) continue;
            float ndl = dot(n, l);
            if (ndl > 0.0) {
                if (i == u_shadow_light0) reach *= shadow_seen(u_shadow_tex0, u_shadow_mat0, v_world, n, u_shadow_texel0);
                if (i == u_shadow_light1) reach *= shadow_seen(u_shadow_tex1, u_shadow_mat1, v_world, n, u_shadow_texel1);
                if (reach <= 0.0) continue;
            }
            float d = toon ? step_light(ndl, u_bands) : max(ndl, 0.0);
            float s = 0.0;
            if (strength > 0.0 && ndl > 0.0) {
                vec3 h = normalize(l + to_eye);
                s = pow(max(dot(n, h), 0.0), expo) * strength;
                if (toon) s = s > 0.5 ? 0.6 : 0.0;
            }
            diffuse += c.rgb * reach * d;
            shine += c.rgb * reach * s;
        }
        float keep = 1.0 - u_metal * 0.9;
        diffuse = diffuse * keep + u_emissive;
        rgba.rgb = min(rgba.rgb * diffuse + shine * rgba.a, vec3(rgba.a));
    }
    if (u_fog.a > 0.5) {
        float distance = u_ortho != 0 ? dot(v_world - u_eye, u_forward) : length(v_world - u_eye);
        float amount = clamp((distance - u_fog_range.x) / max(u_fog_range.y - u_fog_range.x, 1.0), 0.0, 1.0);
        rgba.rgb = mix(rgba.rgb, u_fog.rgb * rgba.a, amount);
    }
    out_color = rgba;
}
"#;

const OVERLAY_VERTEX: &str = r#"#version 140
in vec3 a_pos;
in vec4 a_color;
uniform mat4 u_view;
uniform mat4 u_clip;
uniform float u_point_size;
out vec4 v_color;
void main() {
    v_color = vec4(a_color.rgb * a_color.a, a_color.a);
    gl_Position = u_clip * (u_view * vec4(a_pos, 1.0));
    gl_PointSize = u_point_size;
}
"#;

const OVERLAY_FRAGMENT: &str = r#"#version 140
in vec4 v_color;
out vec4 out_color;
void main() {
    out_color = v_color;
}
"#;

const SHADOW_VERTEX: &str = r#"#version 140
in vec3 a_pos;
in vec2 a_uv;
uniform mat4 u_light;
uniform mat4 u_model;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = u_light * (u_model * vec4(a_pos, 1.0));
}
"#;

const SHADOW_FRAGMENT: &str = r#"#version 140
in vec2 v_uv;
uniform int u_cut;
uniform sampler2D u_tex;
out vec4 out_color;
void main() {
    if (u_cut != 0 && texture(u_tex, v_uv).a < 0.5) discard;
    out_color = vec4(0.0);
}
"#;

const COMPOSITE_VERTEX: &str = r#"#version 140
out vec2 v_uv;
void main() {
    vec2 p;
    if (gl_VertexID == 0) p = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) p = vec2(1.0, -1.0);
    else if (gl_VertexID == 2) p = vec2(-1.0, 1.0);
    else p = vec2(1.0, 1.0);
    v_uv = p * 0.5 + 0.5;
    gl_Position = vec4(p, 0.0, 1.0);
}
"#;

const COMPOSITE_FRAGMENT: &str = r#"#version 140
in vec2 v_uv;
uniform sampler2D u_tex;
out vec4 out_color;
void main() {
    out_color = texture(u_tex, v_uv);
}
"#;

/// Floats per vertex in a static buffer: position, normal, uv, colour,
/// smoothed normal.
const STATIC_FLOATS: usize = 3 + 3 + 2 + 4 + 3;
/// Floats per vertex in a posed buffer: position, normal, smoothed normal.
const POSED_FLOATS: usize = 9;

struct MeshBuffers {
    owner: Weak<RenderMesh>,
    vertices: glow::Buffer,
    indices: glow::Buffer,
}

struct TextureEntry {
    owner: Weak<Vec<u8>>,
    texture: glow::Texture,
}

struct Target {
    width: i32,
    height: i32,

    msaa: Option<glow::Framebuffer>,
    msaa_color: Option<glow::Renderbuffer>,
    msaa_depth: Option<glow::Renderbuffer>,
    resolve: glow::Framebuffer,
    resolve_texture: glow::Texture,
    resolve_depth: glow::Renderbuffer,
}

struct Uniforms {
    model: Option<glow::UniformLocation>,
    normal_mat: Option<glow::UniformLocation>,
    view: Option<glow::UniformLocation>,
    clip: Option<glow::UniformLocation>,
    outline: Option<glow::UniformLocation>,
    outline_k: Option<glow::UniformLocation>,
    base: Option<glow::UniformLocation>,
    has_tex: Option<glow::UniformLocation>,
    tex_premul: Option<glow::UniformLocation>,
    tex: Option<glow::UniformLocation>,
    vertex_colors: Option<glow::UniformLocation>,
    lit: Option<glow::UniformLocation>,
    shading: Option<glow::UniformLocation>,
    bands: Option<glow::UniformLocation>,
    rough: Option<glow::UniformLocation>,
    metal: Option<glow::UniformLocation>,
    emissive: Option<glow::UniformLocation>,
    cutoff: Option<glow::UniformLocation>,
    opacity: Option<glow::UniformLocation>,
    depth_only: Option<glow::UniformLocation>,
    ambient: Option<glow::UniformLocation>,
    light_count: Option<glow::UniformLocation>,
    la: Option<glow::UniformLocation>,
    lb: Option<glow::UniformLocation>,
    lc: Option<glow::UniformLocation>,
    ld: Option<glow::UniformLocation>,
    eye: Option<glow::UniformLocation>,
    ortho: Option<glow::UniformLocation>,
    fog: Option<glow::UniformLocation>,
    fog_range: Option<glow::UniformLocation>,
    forward: Option<glow::UniformLocation>,
    shadow_mat: [Option<glow::UniformLocation>; 2],
    shadow_tex: [Option<glow::UniformLocation>; 2],
    shadow_light: [Option<glow::UniformLocation>; 2],
    shadow_texel: [Option<glow::UniformLocation>; 2],
}

/// Draws depth as seen from a light.
struct ShadowPass {
    program: glow::Program,
    light: Option<glow::UniformLocation>,
    model: Option<glow::UniformLocation>,
    cut: Option<glow::UniformLocation>,
    tex: Option<glow::UniformLocation>,
    pos: u32,
    uv: u32,
    /// A depth texture and its framebuffer per shadowed light.
    maps: Vec<(glow::Framebuffer, glow::Texture)>,
}

struct Renderer {
    program: glow::Program,
    composite: glow::Program,
    composite_tex: Option<glow::UniformLocation>,
    overlay: glow::Program,
    overlay_view: Option<glow::UniformLocation>,
    overlay_clip: Option<glow::UniformLocation>,
    overlay_point: Option<glow::UniformLocation>,
    overlay_attributes: [u32; 2],
    overlay_buffer: glow::Buffer,
    shadow: Option<ShadowPass>,
    u: Uniforms,
    attributes: [u32; 5],
    vao: glow::VertexArray,
    quad: glow::Buffer,
    quad_indices: glow::Buffer,
    posed: glow::Buffer,
    white: glow::Texture,
    meshes: HashMap<usize, MeshBuffers>,
    textures: HashMap<(usize, bool), TextureEntry>,
    target: Option<Target>,
    /// Targets of other sizes (the stage, library thumbnails), kept so
    /// they are not rebuilt every time the size changes back.
    spare: Vec<Target>,
    max_samples: i32,
}

unsafe fn compile_program(
    gl: &glow::Context,
    vertex: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
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
    let vs = compile(glow::VERTEX_SHADER, vertex)?;
    let fs = match compile(glow::FRAGMENT_SHADER, fragment) {
        Ok(fs) => fs,
        Err(error) => {
            gl.delete_shader(vs);
            return Err(error);
        }
    };
    let program = gl.create_program()?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte pattern of the slice is valid
    // to read as bytes.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

fn u32_bytes(values: &[u32]) -> &[u8] {
    // SAFETY: as above.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

fn native_framebuffer(raw: i32) -> Option<glow::Framebuffer> {
    NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
}

/// Every render vertex's smoothed normal: the average over render vertices
/// that share its source vertex, so outlines do not split at hard edges.
fn smoothed(render: &RenderMesh, normals: &[V3]) -> Vec<V3> {
    let count = render
        .source
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m as usize + 1);
    let mut sums = vec![V3::ZERO; count];
    for (i, source) in render.source.iter().enumerate() {
        sums[*source as usize] += normals[i];
    }
    render
        .source
        .iter()
        .map(|source| sums[*source as usize].normalized())
        .collect()
}

impl Renderer {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = compile_program(gl, MESH_VERTEX, MESH_FRAGMENT)?;
        let composite = compile_program(gl, COMPOSITE_VERTEX, COMPOSITE_FRAGMENT)?;
        let overlay = compile_program(gl, OVERLAY_VERTEX, OVERLAY_FRAGMENT)?;
        let overlay_buffer = gl.create_buffer()?;
        // Shadows are a bonus: a driver without depth textures still draws.
        let shadow = compile_program(gl, SHADOW_VERTEX, SHADOW_FRAGMENT)
            .map_err(|error| eprintln!("3D shadow shader: {error}"))
            .ok()
            .map(|program| ShadowPass {
                light: gl.get_uniform_location(program, "u_light"),
                model: gl.get_uniform_location(program, "u_model"),
                cut: gl.get_uniform_location(program, "u_cut"),
                tex: gl.get_uniform_location(program, "u_tex"),
                pos: gl.get_attrib_location(program, "a_pos").unwrap_or(u32::MAX),
                uv: gl.get_attrib_location(program, "a_uv").unwrap_or(u32::MAX),
                program,
                maps: Vec::new(),
            });
        let at = |name: &str| gl.get_uniform_location(program, name);
        let array = |name: &str| at(&format!("{name}[0]")).or_else(|| at(name));
        let u = Uniforms {
            model: at("u_model"),
            normal_mat: at("u_normal_mat"),
            view: at("u_view"),
            clip: at("u_clip"),
            outline: at("u_outline"),
            outline_k: at("u_outline_k"),
            base: at("u_base"),
            has_tex: at("u_has_tex"),
            tex_premul: at("u_tex_premul"),
            tex: at("u_tex"),
            vertex_colors: at("u_vertex_colors"),
            lit: at("u_lit"),
            shading: at("u_shading"),
            bands: at("u_bands"),
            rough: at("u_rough"),
            metal: at("u_metal"),
            emissive: at("u_emissive"),
            cutoff: at("u_cutoff"),
            opacity: at("u_opacity"),
            depth_only: at("u_depth_only"),
            ambient: at("u_ambient"),
            light_count: at("u_light_count"),
            la: array("u_la"),
            lb: array("u_lb"),
            lc: array("u_lc"),
            ld: array("u_ld"),
            eye: at("u_eye"),
            ortho: at("u_ortho"),
            fog: at("u_fog"),
            fog_range: at("u_fog_range"),
            forward: at("u_forward"),
            shadow_mat: [at("u_shadow_mat0"), at("u_shadow_mat1")],
            shadow_tex: [at("u_shadow_tex0"), at("u_shadow_tex1")],
            shadow_light: [at("u_shadow_light0"), at("u_shadow_light1")],
            shadow_texel: [at("u_shadow_texel0"), at("u_shadow_texel1")],
        };
        let attribute = |name: &str| gl.get_attrib_location(program, name).unwrap_or(u32::MAX);
        let attributes = [
            attribute("a_pos"),
            attribute("a_normal"),
            attribute("a_uv"),
            attribute("a_color"),
            attribute("a_smooth"),
        ];
        let vao = gl.create_vertex_array()?;
        let quad = gl.create_buffer()?;
        // A unit square in its own plane, facing the viewer.
        #[rustfmt::skip]
        let corners: [f32; 60] = [
            0.0, 0.0, 0.0,  0.0, 0.0, -1.0,  0.0, 0.0,  1.0, 1.0, 1.0, 1.0,  0.0, 0.0, -1.0,
            1.0, 0.0, 0.0,  0.0, 0.0, -1.0,  1.0, 0.0,  1.0, 1.0, 1.0, 1.0,  0.0, 0.0, -1.0,
            1.0, 1.0, 0.0,  0.0, 0.0, -1.0,  1.0, 1.0,  1.0, 1.0, 1.0, 1.0,  0.0, 0.0, -1.0,
            0.0, 1.0, 0.0,  0.0, 0.0, -1.0,  0.0, 1.0,  1.0, 1.0, 1.0, 1.0,  0.0, 0.0, -1.0,
        ];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, f32_bytes(&corners), glow::STATIC_DRAW);
        let quad_indices = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(quad_indices));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            u32_bytes(&[0, 1, 2, 0, 2, 3]),
            glow::STATIC_DRAW,
        );
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        let posed = gl.create_buffer()?;
        let white = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(white));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            Some(&[255, 255, 255, 255]),
        );
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
        gl.bind_texture(glow::TEXTURE_2D, None);
        let max_samples = gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(0, 4);
        Ok(Self {
            program,
            composite_tex: gl.get_uniform_location(composite, "u_tex"),
            overlay_view: gl.get_uniform_location(overlay, "u_view"),
            overlay_clip: gl.get_uniform_location(overlay, "u_clip"),
            overlay_point: gl.get_uniform_location(overlay, "u_point_size"),
            overlay_attributes: [
                gl.get_attrib_location(overlay, "a_pos").unwrap_or(u32::MAX),
                gl.get_attrib_location(overlay, "a_color")
                    .unwrap_or(u32::MAX),
            ],
            overlay,
            overlay_buffer,
            shadow,
            composite,
            u,
            attributes,
            vao,
            quad,
            quad_indices,
            posed,
            white,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            target: None,
            spare: Vec::new(),
            max_samples,
        })
    }

    unsafe fn ensure_target(&mut self, gl: &glow::Context, width: i32, height: i32) -> Option<()> {
        if let Some(target) = &self.target {
            if target.width == width && target.height == height {
                return Some(());
            }
        }
        if let Some(old) = self.target.take() {
            self.spare.push(old);
        }
        if let Some(found) = self
            .spare
            .iter()
            .position(|t| t.width == width && t.height == height)
        {
            self.target = Some(self.spare.remove(found));
            return Some(());
        }
        while self.spare.len() > 4 {
            let old = self.spare.remove(0);
            if let Some(fb) = old.msaa {
                gl.delete_framebuffer(fb);
            }
            if let Some(rb) = old.msaa_color {
                gl.delete_renderbuffer(rb);
            }
            if let Some(rb) = old.msaa_depth {
                gl.delete_renderbuffer(rb);
            }
            gl.delete_framebuffer(old.resolve);
            gl.delete_texture(old.resolve_texture);
            gl.delete_renderbuffer(old.resolve_depth);
        }
        let resolve_texture = gl.create_texture().ok()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(resolve_texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
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
        gl.bind_texture(glow::TEXTURE_2D, None);
        let resolve_depth = gl.create_renderbuffer().ok()?;
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(resolve_depth));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, width, height);
        let resolve = gl.create_framebuffer().ok()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(resolve_texture),
            0,
        );
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::DEPTH_ATTACHMENT,
            glow::RENDERBUFFER,
            Some(resolve_depth),
        );
        let samples = self.max_samples;
        let (mut msaa, mut msaa_color, mut msaa_depth) = (None, None, None);
        if samples > 1 {
            let color = gl.create_renderbuffer().ok()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::RGBA8,
                width,
                height,
            );
            let depth = gl.create_renderbuffer().ok()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::DEPTH_COMPONENT24,
                width,
                height,
            );
            let fb = gl.create_framebuffer().ok()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(color),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(depth),
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
                msaa = Some(fb);
                msaa_color = Some(color);
                msaa_depth = Some(depth);
            } else {
                gl.delete_framebuffer(fb);
                gl.delete_renderbuffer(color);
                gl.delete_renderbuffer(depth);
            }
        }
        gl.bind_renderbuffer(glow::RENDERBUFFER, None);
        self.target = Some(Target {
            width,
            height,

            msaa,
            msaa_color,
            msaa_depth,
            resolve,
            resolve_texture,
            resolve_depth,
        });
        Some(())
    }

    unsafe fn mesh_buffers(
        &mut self,
        gl: &glow::Context,
        render: &Arc<RenderMesh>,
    ) -> Option<(glow::Buffer, glow::Buffer)> {
        let key = Arc::as_ptr(render) as usize;
        if let Some(entry) = self.meshes.get(&key) {
            if entry
                .owner
                .upgrade()
                .is_some_and(|alive| Arc::ptr_eq(&alive, render))
            {
                return Some((entry.vertices, entry.indices));
            }
            let stale = self.meshes.remove(&key)?;
            gl.delete_buffer(stale.vertices);
            gl.delete_buffer(stale.indices);
        }
        let smooth = smoothed(render, &render.normals);
        let mut data = Vec::with_capacity(render.positions.len() * STATIC_FLOATS);
        for (i, s) in smooth.iter().enumerate() {
            let (p, n, uv, c) = (
                render.positions[i],
                render.normals[i],
                render.uvs[i],
                render.colors[i],
            );
            data.extend_from_slice(&[p.x, p.y, p.z, n.x, n.y, n.z, uv[0], uv[1]]);
            data.extend(c.iter().map(|v| f32::from(*v) / 255.0));
            data.extend_from_slice(&[s.x, s.y, s.z]);
        }
        let vertices = gl.create_buffer().ok()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, f32_bytes(&data), glow::STATIC_DRAW);
        let indices = gl.create_buffer().ok()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            u32_bytes(&render.indices),
            glow::STATIC_DRAW,
        );
        self.meshes.insert(
            key,
            MeshBuffers {
                owner: Arc::downgrade(render),
                vertices,
                indices,
            },
        );
        Some((vertices, indices))
    }

    unsafe fn texture(
        &mut self,
        gl: &glow::Context,
        rgba: &Arc<Vec<u8>>,
        width: u32,
        height: u32,
        repeat: bool,
        nearest: bool,
    ) -> Option<glow::Texture> {
        let key = (Arc::as_ptr(rgba) as usize, repeat);
        if let Some(entry) = self.textures.get(&key) {
            if entry
                .owner
                .upgrade()
                .is_some_and(|alive| Arc::ptr_eq(&alive, rgba))
            {
                return Some(entry.texture);
            }
            let stale = self.textures.remove(&key)?;
            gl.delete_texture(stale.texture);
        }
        if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
            return None;
        }
        let texture = gl.create_texture().ok()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            Some(&rgba[..(width * height * 4) as usize]),
        );
        let filter = if nearest { glow::NEAREST } else { glow::LINEAR };
        let min_filter = if nearest {
            glow::NEAREST
        } else {
            glow::LINEAR_MIPMAP_LINEAR
        };
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            min_filter as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        let wrap = if repeat {
            glow::REPEAT
        } else {
            glow::CLAMP_TO_EDGE
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap as i32);
        self.textures.insert(
            key,
            TextureEntry {
                owner: Arc::downgrade(rgba),
                texture,
            },
        );
        Some(texture)
    }

    /// Drops buffers and textures whose owners are gone.
    unsafe fn sweep(&mut self, gl: &glow::Context) {
        self.meshes.retain(|_, entry| {
            let alive = entry.owner.strong_count() > 0;
            if !alive {
                gl.delete_buffer(entry.vertices);
                gl.delete_buffer(entry.indices);
            }
            alive
        });
        self.textures.retain(|_, entry| {
            let alive = entry.owner.strong_count() > 0;
            if !alive {
                gl.delete_texture(entry.texture);
            }
            alive
        });
    }

    unsafe fn bind_attributes(
        &self,
        gl: &glow::Context,
        buffer: glow::Buffer,
        posed: Option<glow::Buffer>,
    ) {
        let stride = (STATIC_FLOATS * 4) as i32;
        let [pos, normal, uv, color, smooth] = self.attributes;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        let attrib = |location: u32, size: i32, stride: i32, offset: i32| {
            if location != u32::MAX {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
            }
        };
        attrib(uv, 2, stride, 6 * 4);
        attrib(color, 4, stride, 8 * 4);
        match posed {
            Some(posed) => {
                let posed_stride = (POSED_FLOATS * 4) as i32;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(posed));
                attrib(pos, 3, posed_stride, 0);
                attrib(normal, 3, posed_stride, 3 * 4);
                attrib(smooth, 3, posed_stride, 6 * 4);
            }
            None => {
                attrib(pos, 3, stride, 0);
                attrib(normal, 3, stride, 3 * 4);
                attrib(smooth, 3, stride, 12 * 4);
            }
        }
    }

    unsafe fn set_matrix(gl: &glow::Context, location: Option<&glow::UniformLocation>, m: &Mat4) {
        gl.uniform_matrix_4_f32_slice(location, false, &m.m);
    }

    unsafe fn draw(
        &mut self,
        gl: &glow::Context,
        frame: &Frame3d,
        region: crate::CallbackRegion,
        pixels_per_point: f32,
        screen_size_px: [u32; 2],
    ) {
        let (width, height) = (region.width, region.height);
        let read_framebuffer =
            native_framebuffer(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING));
        let draw_framebuffer =
            native_framebuffer(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING));
        self.sweep(gl);
        if self.ensure_target(gl, width, height).is_none() {
            return;
        }
        let target = self.target.as_ref().expect("target");
        let draw_fb = target.msaa.unwrap_or(target.resolve);
        let (resolve_fb, resolve_texture) = (target.resolve, target.resolve_texture);
        let has_msaa = target.msaa.is_some();
        // Stage to this target's pixels: screen points, less the region's
        // corner, times the pixel density.
        let ppp = pixels_per_point.max(f32::EPSILON);
        let top_px = screen_size_px[1] as i32 - region.bottom - region.height;
        let to_target = Affine::compose(
            Affine {
                a11: ppp,
                a12: 0.0,
                a21: 0.0,
                a22: ppp,
                tx: -(region.left as f32),
                ty: -(top_px as f32),
            },
            frame.view.to_output,
        );
        let view = View3d {
            to_output: to_target,
            output: [width as u32, height as u32],
            ..frame.view
        };
        let (near, far) = frame.depth_range();
        let clip = view.clip_matrix(near, far);
        let flip = to_target.a11 * to_target.a22 - to_target.a12 * to_target.a21 < 0.0;

        gl.disable(glow::SCISSOR_TEST);
        let shadow_views = q0s_format::three::draw::shadow_views(frame);
        let shadows = self.render_shadows(gl, frame, &shadow_views);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(draw_fb));
        gl.viewport(0, 0, width, height);
        gl.color_mask(true, true, true, true);
        gl.depth_mask(true);
        match frame.clear {
            Some([r, g, b, a]) => gl.clear_color(
                f32::from(r) / 255.0 * f32::from(a) / 255.0,
                f32::from(g) / 255.0 * f32::from(a) / 255.0,
                f32::from(b) / 255.0 * f32::from(a) / 255.0,
                f32::from(a) / 255.0,
            ),
            None => gl.clear_color(0.0, 0.0, 0.0, 0.0),
        }
        gl.clear_depth_f32(1.0);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.enable(glow::BLEND);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.uniform_1_i32(self.u.tex.as_ref(), 0);
        Self::set_matrix(gl, self.u.view.as_ref(), &frame.view.view);
        Self::set_matrix(gl, self.u.clip.as_ref(), &clip);
        self.set_lights(gl, frame);
        for slot in 0..2 {
            match (shadow_views.get(slot), shadows.get(slot)) {
                (Some(view), Some(texture)) => {
                    gl.active_texture(glow::TEXTURE1 + slot as u32);
                    gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
                    gl.uniform_1_i32(self.u.shadow_tex[slot].as_ref(), 1 + slot as i32);
                    Self::set_matrix(gl, self.u.shadow_mat[slot].as_ref(), &view.matrix);
                    gl.uniform_1_i32(self.u.shadow_light[slot].as_ref(), view.light as i32);
                    gl.uniform_1_f32(self.u.shadow_texel[slot].as_ref(), view.texel);
                }
                _ => {
                    gl.uniform_1_i32(self.u.shadow_tex[slot].as_ref(), 1 + slot as i32);
                    gl.uniform_1_i32(self.u.shadow_light[slot].as_ref(), -1);
                }
            }
        }
        gl.active_texture(glow::TEXTURE0);

        // Outline reach per unit of view depth (perspective) or constant.
        let outline_per_depth = |width: f32| {
            if view.orthographic {
                [0.0, width / view.stage_scale_at(1.0).max(1.0e-6)]
            } else {
                [width / view.camera.zoom.max(1.0e-6), 0.0]
            }
        };

        if frame.overlay.is_some() {
            // Surfaces sit a hair behind the lines drawn on them.
            gl.enable(glow::POLYGON_OFFSET_FILL);
            gl.polygon_offset(1.0, 1.0);
        }
        let passes = draw_passes(&frame.items, &frame.view.view);
        // (pass, item, part or plane, depth-only)
        let mut draws: Vec<(u32, usize, Option<usize>, bool)> = Vec::new();
        for (index, item) in frame.items.iter().enumerate() {
            let (opaque, blended) = passes[index];
            let item_blends = item.opacity() < 0.999;
            match item {
                DrawItem::Mesh(mesh) => {
                    for (part, (material, _, _)) in mesh.render.parts.iter().enumerate() {
                        let blends = item_blends
                            || mesh
                                .model
                                .materials
                                .get(usize::from(*material))
                                .is_some_and(|m| m.blends(&mesh.model.textures));
                        draws.push((
                            if blends { blended } else { opaque },
                            index,
                            Some(part),
                            false,
                        ));
                    }
                    if mesh.outline && mesh.model.outline.is_some() {
                        draws.push((
                            if item_blends { blended } else { opaque },
                            index,
                            None,
                            false,
                        ));
                    }
                }
                DrawItem::Plane(plane) => {
                    draws.push((blended, index, None, false));
                    if !plane.layered {
                        draws.push((blended, index, None, true));
                    }
                }
            }
        }
        draws.sort_by_key(|draw| draw.0);
        let mut posed_for: Option<usize> = None;
        for (_, index, part, depth_only) in draws {
            match &frame.items[index] {
                DrawItem::Mesh(mesh) => {
                    let Some((vertices, indices)) = self.mesh_buffers(gl, &mesh.render) else {
                        continue;
                    };
                    let posed = match &mesh.posed {
                        Some(posed) => {
                            if posed_for != Some(index) {
                                let smooth = smoothed(&mesh.render, &posed.normals);
                                let mut data =
                                    Vec::with_capacity(posed.positions.len() * POSED_FLOATS);
                                for ((p, n), s) in
                                    posed.positions.iter().zip(&posed.normals).zip(&smooth)
                                {
                                    data.extend_from_slice(&[
                                        p.x, p.y, p.z, n.x, n.y, n.z, s.x, s.y, s.z,
                                    ]);
                                }
                                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.posed));
                                gl.buffer_data_u8_slice(
                                    glow::ARRAY_BUFFER,
                                    f32_bytes(&data),
                                    glow::STREAM_DRAW,
                                );
                                posed_for = Some(index);
                            }
                            Some(self.posed)
                        }
                        None => None,
                    };
                    self.bind_attributes(gl, vertices, posed);
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
                    let normal_mat = mesh
                        .matrix
                        .inverse()
                        .map(|m| m.transpose())
                        .unwrap_or(mesh.matrix);
                    Self::set_matrix(gl, self.u.model.as_ref(), &mesh.matrix);
                    Self::set_matrix(gl, self.u.normal_mat.as_ref(), &normal_mat);
                    let mirrored = (mesh.matrix.determinant3() < 0.0) ^ flip;
                    gl.front_face(if mirrored { glow::CW } else { glow::CCW });
                    gl.uniform_1_i32(self.u.depth_only.as_ref(), 0);
                    match part {
                        Some(part) => {
                            let (material_index, first, count) = mesh.render.parts[part];
                            let Some(material) =
                                mesh.model.materials.get(usize::from(material_index))
                            else {
                                continue;
                            };
                            let blends =
                                mesh.opacity < 0.999 || material.blends(&mesh.model.textures);
                            if material.double_sided {
                                gl.disable(glow::CULL_FACE);
                            } else {
                                gl.enable(glow::CULL_FACE);
                                gl.cull_face(glow::BACK);
                            }
                            gl.depth_mask(!blends);
                            let a = f32::from(material.color.a) / 255.0 * mesh.opacity;
                            gl.uniform_4_f32(
                                self.u.base.as_ref(),
                                f32::from(material.color.r) / 255.0 * a,
                                f32::from(material.color.g) / 255.0 * a,
                                f32::from(material.color.b) / 255.0 * a,
                                a,
                            );
                            let texture = material
                                .texture
                                .and_then(|t| mesh.model.textures.get(usize::from(t)))
                                .and_then(|t| {
                                    self.texture(gl, &t.rgba, t.width, t.height, true, t.pixelated)
                                });
                            gl.bind_texture(glow::TEXTURE_2D, Some(texture.unwrap_or(self.white)));
                            gl.uniform_1_i32(self.u.has_tex.as_ref(), i32::from(texture.is_some()));
                            gl.uniform_1_i32(self.u.tex_premul.as_ref(), 0);
                            gl.uniform_1_i32(
                                self.u.vertex_colors.as_ref(),
                                i32::from(mesh.render.has_colors),
                            );
                            gl.uniform_1_i32(
                                self.u.lit.as_ref(),
                                i32::from(mesh.lit && material.shading != Shading::Unlit),
                            );
                            gl.uniform_1_i32(
                                self.u.shading.as_ref(),
                                match material.shading {
                                    Shading::Lit => 0,
                                    Shading::Toon => 1,
                                    Shading::Unlit => 2,
                                    Shading::Glass => 3,
                                },
                            );
                            gl.uniform_1_f32(
                                self.u.bands.as_ref(),
                                f32::from(material.toon_bands.max(2)),
                            );
                            gl.uniform_1_f32(self.u.rough.as_ref(), material.roughness);
                            gl.uniform_1_f32(self.u.metal.as_ref(), material.metallic);
                            let e = material.emissive;
                            gl.uniform_3_f32(
                                self.u.emissive.as_ref(),
                                f32::from(e.r) / 255.0,
                                f32::from(e.g) / 255.0,
                                f32::from(e.b) / 255.0,
                            );
                            gl.uniform_1_f32(
                                self.u.cutoff.as_ref(),
                                material.alpha_cutoff.unwrap_or(-1.0),
                            );
                            gl.uniform_1_f32(self.u.opacity.as_ref(), mesh.opacity);
                            gl.uniform_1_f32(self.u.outline.as_ref(), 0.0);
                            gl.draw_elements(
                                glow::TRIANGLES,
                                count as i32,
                                glow::UNSIGNED_INT,
                                (first * 4) as i32,
                            );
                        }
                        None => {
                            let Some(outline) = mesh.model.outline else {
                                continue;
                            };
                            gl.enable(glow::CULL_FACE);
                            gl.cull_face(glow::FRONT);
                            gl.depth_mask(mesh.opacity >= 0.999);
                            let a = f32::from(outline.color.a) / 255.0 * mesh.opacity;
                            gl.uniform_4_f32(
                                self.u.base.as_ref(),
                                f32::from(outline.color.r) / 255.0 * a,
                                f32::from(outline.color.g) / 255.0 * a,
                                f32::from(outline.color.b) / 255.0 * a,
                                a,
                            );
                            gl.bind_texture(glow::TEXTURE_2D, Some(self.white));
                            gl.uniform_1_i32(self.u.has_tex.as_ref(), 0);
                            gl.uniform_1_i32(self.u.vertex_colors.as_ref(), 0);
                            gl.uniform_1_i32(self.u.lit.as_ref(), 0);
                            gl.uniform_1_f32(self.u.cutoff.as_ref(), -1.0);
                            gl.uniform_1_f32(self.u.opacity.as_ref(), mesh.opacity);
                            gl.uniform_1_f32(self.u.outline.as_ref(), 1.0);
                            let [k0, k1] = outline_per_depth(outline.width);
                            gl.uniform_2_f32(self.u.outline_k.as_ref(), k0, k1);
                            gl.draw_elements(
                                glow::TRIANGLES,
                                mesh.render.indices.len() as i32,
                                glow::UNSIGNED_INT,
                                0,
                            );
                        }
                    }
                }
                DrawItem::Plane(plane) => {
                    let Some(texture) = self.texture(
                        gl,
                        &plane.image.rgba,
                        plane.image.width,
                        plane.image.height,
                        false,
                        false,
                    ) else {
                        continue;
                    };
                    self.bind_attributes(gl, self.quad, None);
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.quad_indices));
                    let [x0, y0, x1, y1] = plane.rect;
                    let model = plane.matrix
                        * Mat4::translation(V3::new(x0, y0, 0.0))
                        * Mat4::scale(V3::new(x1 - x0, y1 - y0, 1.0));
                    let normal_mat = plane
                        .matrix
                        .inverse()
                        .map(|m| m.transpose())
                        .unwrap_or(plane.matrix);
                    Self::set_matrix(gl, self.u.model.as_ref(), &model);
                    Self::set_matrix(gl, self.u.normal_mat.as_ref(), &normal_mat);
                    gl.disable(glow::CULL_FACE);
                    if plane.layered {
                        gl.disable(glow::DEPTH_TEST);
                    }
                    gl.depth_mask(depth_only);
                    gl.color_mask(!depth_only, !depth_only, !depth_only, !depth_only);
                    gl.uniform_1_i32(self.u.depth_only.as_ref(), i32::from(depth_only));
                    gl.uniform_4_f32(
                        self.u.base.as_ref(),
                        plane.opacity,
                        plane.opacity,
                        plane.opacity,
                        plane.opacity,
                    );
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.uniform_1_i32(self.u.has_tex.as_ref(), 1);
                    gl.uniform_1_i32(self.u.tex_premul.as_ref(), 1);
                    gl.uniform_1_i32(self.u.vertex_colors.as_ref(), 0);
                    gl.uniform_1_i32(
                        self.u.lit.as_ref(),
                        i32::from(plane.lit && frame.rig.authored),
                    );
                    gl.uniform_1_i32(self.u.shading.as_ref(), 0);
                    gl.uniform_1_f32(self.u.rough.as_ref(), 1.0);
                    gl.uniform_1_f32(self.u.metal.as_ref(), 0.0);
                    gl.uniform_3_f32(self.u.emissive.as_ref(), 0.0, 0.0, 0.0);
                    gl.uniform_1_f32(self.u.cutoff.as_ref(), -1.0);
                    gl.uniform_1_f32(self.u.opacity.as_ref(), plane.opacity);
                    gl.uniform_1_f32(self.u.outline.as_ref(), 0.0);
                    gl.draw_elements(glow::TRIANGLES, 6, glow::UNSIGNED_INT, 0);
                    gl.color_mask(true, true, true, true);
                    gl.enable(glow::DEPTH_TEST);
                }
            }
        }
        for location in self.attributes {
            if location != u32::MAX {
                gl.disable_vertex_attrib_array(location);
            }
        }
        gl.depth_mask(true);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.front_face(glow::CCW);
        gl.disable(glow::POLYGON_OFFSET_FILL);
        for slot in 0..2 {
            gl.active_texture(glow::TEXTURE1 + slot);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
        if let Some(overlay) = &frame.overlay {
            self.draw_overlay(gl, overlay, &frame.view.view, &clip);
        }
        if has_msaa {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(draw_fb));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(resolve_fb));
            gl.blit_framebuffer(
                0,
                0,
                width,
                height,
                0,
                0,
                width,
                height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        // Lay the picture over what egui painted.
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw_framebuffer);
        gl.viewport(region.left, region.bottom, region.width, region.height);
        gl.use_program(Some(self.composite));
        gl.bind_texture(glow::TEXTURE_2D, Some(resolve_texture));
        gl.uniform_1_i32(self.composite_tex.as_ref(), 0);
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE_MINUS_DST_ALPHA,
            glow::ONE,
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
    }

    /// Draws each shadowed light's depth map. Returns the textures, in the
    /// order of `views`; empty when shadows cannot be drawn here.
    unsafe fn render_shadows(
        &mut self,
        gl: &glow::Context,
        frame: &Frame3d,
        views: &[q0s_format::three::draw::ShadowView],
    ) -> Vec<glow::Texture> {
        if views.is_empty() || self.shadow.is_none() {
            return Vec::new();
        }
        let size = q0s_format::three::draw::SHADOW_SIZE as i32;
        while self
            .shadow
            .as_ref()
            .is_some_and(|s| s.maps.len() < views.len())
        {
            let Ok(texture) = gl.create_texture() else {
                return Vec::new();
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::DEPTH_COMPONENT24 as i32,
                size,
                size,
                0,
                glow::DEPTH_COMPONENT,
                glow::UNSIGNED_INT,
                None,
            );
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
            gl.bind_texture(glow::TEXTURE_2D, None);
            let Ok(fb) = gl.create_framebuffer() else {
                gl.delete_texture(texture);
                return Vec::new();
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.draw_buffers(&[glow::NONE]);
            gl.read_buffer(glow::NONE);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(fb);
                gl.delete_texture(texture);
                eprintln!("3D shadows: depth target unsupported");
                self.shadow = None;
                return Vec::new();
            }
            if let Some(pass) = self.shadow.as_mut() {
                pass.maps.push((fb, texture));
            }
        }
        let Some(pass) = self.shadow.as_ref() else {
            return Vec::new();
        };
        let (program, light_u, model_u, cut_u, tex_u, pos, uv) = (
            pass.program,
            pass.light,
            pass.model,
            pass.cut,
            pass.tex,
            pass.pos,
            pass.uv,
        );
        let maps: Vec<(glow::Framebuffer, glow::Texture)> =
            pass.maps.iter().take(views.len()).copied().collect();
        gl.use_program(Some(program));
        gl.bind_vertex_array(Some(self.vao));
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LESS);
        gl.depth_mask(true);
        gl.color_mask(false, false, false, false);
        gl.viewport(0, 0, size, size);
        gl.active_texture(glow::TEXTURE0);
        gl.uniform_1_i32(tex_u.as_ref(), 0);
        let attrib = |location: u32, buffer: glow::Buffer, size: i32, stride: i32, offset: i32| {
            if location != u32::MAX {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
            }
        };
        let stride = (STATIC_FLOATS * 4) as i32;
        for (view, (fb, _)) in views.iter().zip(&maps) {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(*fb));
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            Self::set_matrix(gl, light_u.as_ref(), &view.matrix);
            for item in &frame.items {
                match item {
                    DrawItem::Mesh(mesh) if mesh.opacity >= 0.5 => {
                        let Some((vertices, indices)) = self.mesh_buffers(gl, &mesh.render) else {
                            continue;
                        };
                        match &mesh.posed {
                            Some(posed) => {
                                let data: Vec<f32> = posed
                                    .positions
                                    .iter()
                                    .flat_map(|p| [p.x, p.y, p.z])
                                    .collect();
                                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.posed));
                                gl.buffer_data_u8_slice(
                                    glow::ARRAY_BUFFER,
                                    f32_bytes(&data),
                                    glow::STREAM_DRAW,
                                );
                                attrib(pos, self.posed, 3, 12, 0);
                            }
                            None => attrib(pos, vertices, 3, stride, 0),
                        }
                        attrib(uv, vertices, 2, stride, 6 * 4);
                        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
                        Self::set_matrix(gl, model_u.as_ref(), &mesh.matrix);
                        gl.uniform_1_i32(cut_u.as_ref(), 0);
                        gl.draw_elements(
                            glow::TRIANGLES,
                            mesh.render.indices.len() as i32,
                            glow::UNSIGNED_INT,
                            0,
                        );
                    }
                    DrawItem::Plane(plane) if !plane.layered && plane.opacity >= 0.5 => {
                        let Some(texture) = self.texture(
                            gl,
                            &plane.image.rgba,
                            plane.image.width,
                            plane.image.height,
                            false,
                            false,
                        ) else {
                            continue;
                        };
                        attrib(pos, self.quad, 3, stride, 0);
                        attrib(uv, self.quad, 2, stride, 6 * 4);
                        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.quad_indices));
                        let [x0, y0, x1, y1] = plane.rect;
                        let model = plane.matrix
                            * Mat4::translation(V3::new(x0, y0, 0.0))
                            * Mat4::scale(V3::new(x1 - x0, y1 - y0, 1.0));
                        Self::set_matrix(gl, model_u.as_ref(), &model);
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        gl.uniform_1_i32(cut_u.as_ref(), 1);
                        gl.draw_elements(glow::TRIANGLES, 6, glow::UNSIGNED_INT, 0);
                    }
                    _ => {}
                }
            }
        }
        for location in [pos, uv] {
            if location != u32::MAX {
                gl.disable_vertex_attrib_array(location);
            }
        }
        gl.color_mask(true, true, true, true);
        maps.iter().map(|(_, texture)| *texture).collect()
    }

    unsafe fn draw_overlay(
        &self,
        gl: &glow::Context,
        overlay: &q0s_format::three::draw::Overlay,
        view: &Mat4,
        clip: &Mat4,
    ) {
        gl.use_program(Some(self.overlay));
        Self::set_matrix(gl, self.overlay_view.as_ref(), view);
        Self::set_matrix(gl, self.overlay_clip.as_ref(), clip);
        gl.uniform_1_f32(self.overlay_point.as_ref(), overlay.point_size.max(1.0));
        gl.bind_vertex_array(Some(self.vao));
        let [pos, color] = self.overlay_attributes;
        gl.disable(glow::CULL_FACE);
        gl.depth_mask(false);
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        #[cfg(not(target_os = "android"))]
        if !gl.version().is_embedded {
            gl.enable(glow::PROGRAM_POINT_SIZE);
        }
        let draw = |mode: u32, vertices: &[(V3, [u8; 4])], on_top: bool| {
            if vertices.is_empty() || pos == u32::MAX {
                return;
            }
            let mut data = Vec::with_capacity(vertices.len() * 7);
            for (p, c) in vertices {
                data.extend_from_slice(&[
                    p.x,
                    p.y,
                    p.z,
                    f32::from(c[0]) / 255.0,
                    f32::from(c[1]) / 255.0,
                    f32::from(c[2]) / 255.0,
                    f32::from(c[3]) / 255.0,
                ]);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.overlay_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, f32_bytes(&data), glow::STREAM_DRAW);
            gl.enable_vertex_attrib_array(pos);
            gl.vertex_attrib_pointer_f32(pos, 3, glow::FLOAT, false, 28, 0);
            if color != u32::MAX {
                gl.enable_vertex_attrib_array(color);
                gl.vertex_attrib_pointer_f32(color, 4, glow::FLOAT, false, 28, 12);
            }
            if on_top {
                gl.disable(glow::DEPTH_TEST);
            } else {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
            }
            gl.draw_arrays(mode, 0, vertices.len() as i32);
            gl.disable_vertex_attrib_array(pos);
            if color != u32::MAX {
                gl.disable_vertex_attrib_array(color);
            }
        };
        let tris: Vec<(V3, [u8; 4])> = overlay
            .triangles
            .iter()
            .flat_map(|(t, c)| t.iter().map(move |p| (*p, *c)))
            .collect();
        draw(glow::TRIANGLES, &tris, overlay.on_top);
        let lines: Vec<(V3, [u8; 4])> = overlay
            .lines
            .iter()
            .flat_map(|(l, c)| l.iter().map(move |p| (*p, *c)))
            .collect();
        draw(glow::LINES, &lines, overlay.on_top);
        draw(glow::POINTS, &overlay.points, overlay.on_top);
        let top: Vec<(V3, [u8; 4])> = overlay
            .top_lines
            .iter()
            .flat_map(|(l, c)| l.iter().map(move |p| (*p, *c)))
            .collect();
        draw(glow::LINES, &top, true);
        draw(glow::POINTS, &overlay.top_points, true);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_mask(true);
    }

    unsafe fn set_lights(&self, gl: &glow::Context, frame: &Frame3d) {
        let rig = &frame.rig;
        let count = rig.lights.len().min(MAX_LIGHTS_3D);
        let mut a = vec![0.0f32; MAX_LIGHTS_3D * 4];
        let mut b = vec![0.0f32; MAX_LIGHTS_3D * 4];
        let mut c = vec![0.0f32; MAX_LIGHTS_3D * 4];
        let mut d = vec![0.0f32; MAX_LIGHTS_3D * 4];
        for (i, light) in rig.lights.iter().take(count).enumerate() {
            let kind = match light.kind {
                Light3dKind::Point => 0.0,
                Light3dKind::Spot => 1.0,
                Light3dKind::Sun => 2.0,
            };
            a[i * 4..i * 4 + 4].copy_from_slice(&[
                light.position.x,
                light.position.y,
                light.position.z,
                kind,
            ]);
            b[i * 4..i * 4 + 4].copy_from_slice(&[
                light.direction.x,
                light.direction.y,
                light.direction.z,
                light.range,
            ]);
            c[i * 4..i * 4 + 4].copy_from_slice(&[
                light.color[0],
                light.color[1],
                light.color[2],
                light.cos_outer,
            ]);
            d[i * 4] = light.cos_inner;
        }
        gl.uniform_4_f32_slice(self.u.la.as_ref(), &a);
        gl.uniform_4_f32_slice(self.u.lb.as_ref(), &b);
        gl.uniform_4_f32_slice(self.u.lc.as_ref(), &c);
        gl.uniform_4_f32_slice(self.u.ld.as_ref(), &d);
        gl.uniform_1_i32(self.u.light_count.as_ref(), count as i32);
        gl.uniform_3_f32(
            self.u.ambient.as_ref(),
            rig.ambient[0],
            rig.ambient[1],
            rig.ambient[2],
        );
        let eye = frame.view.camera.position;
        gl.uniform_3_f32(self.u.eye.as_ref(), eye.x, eye.y, eye.z);
        let forward = (frame.view.camera.target - frame.view.camera.position).normalized();
        gl.uniform_3_f32(self.u.forward.as_ref(), forward.x, forward.y, forward.z);
        gl.uniform_1_i32(self.u.ortho.as_ref(), i32::from(frame.view.orthographic));
        match rig.fog {
            Some((color, near, far)) => {
                gl.uniform_4_f32(self.u.fog.as_ref(), color[0], color[1], color[2], 1.0);
                gl.uniform_2_f32(self.u.fog_range.as_ref(), near, far);
            }
            None => gl.uniform_4_f32(self.u.fog.as_ref(), 0.0, 0.0, 0.0, 0.0),
        }
    }
}
