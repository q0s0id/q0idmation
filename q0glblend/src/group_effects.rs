//! Filters for the isolated GPU layer or symbol target.

use glow::HasContext as _;
use q0s_format::v2::{PlacementFx, Rgba};

use crate::layer_opacity::LayerTarget;
use crate::{CallbackRegion, GlResources};

const VERTEX: &str = r#"#version 140
void main() {
    vec2 p = vec2((gl_VertexID & 1) == 0 ? -1.0 : 1.0,
                  gl_VertexID < 2 ? -1.0 : 1.0);
    gl_Position = vec4(p, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"#version 140
uniform sampler2D u_source;
uniform sampler2D u_body;
uniform vec4 u_resample;
uniform ivec2 u_size;
uniform ivec2 u_offset;
uniform int u_mode;
uniform int u_jump;
uniform vec4 u_color;
uniform float u_amount;
out vec4 out_color;

bool in_bounds(ivec2 p) {
    return all(greaterThanEqual(p, ivec2(0))) && all(lessThan(p, u_size));
}

vec4 sample_at(sampler2D image, ivec2 p) {
    if (!in_bounds(p)) return vec4(0.0);
    return texelFetch(image, p, 0);
}

vec4 sample_color(ivec2 p) {
    if (!in_bounds(p)) return vec4(0.0);
    if (u_resample.x == 0.0) return texelFetch(u_source, p, 0);
    vec2 half_texel = u_resample.zw * 0.5;
    vec2 uv = (vec2(p) + 0.5) * u_resample.xy;
    return texture(u_source, clamp(uv, half_texel, vec2(u_size) * u_resample.xy - half_texel));
}

vec4 pack_seed(ivec2 p) {
    ivec2 value = p + 1;
    return vec4(float(value.x / 256), float(value.x % 256),
                float(value.y / 256), float(value.y % 256)) / 255.0;
}

ivec2 unpack_seed(vec4 pixel) {
    ivec4 c = ivec4(round(pixel * 255.0));
    return ivec2(c.x * 256 + c.y, c.z * 256 + c.w) - 1;
}

float outline_under_edge(ivec2 p) {
    float body = sample_at(u_body, p).a;
    if (body <= 0.0 || body >= 1.0) return 0.0;

    // Overlap the antialiased edge so the backdrop cannot show between
    // the body and its outline. Keep the interior of translucent art intact.
    float low = body;
    float high = body;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            float neighbor = sample_at(u_body, p + ivec2(x, y)).a;
            low = min(low, neighbor);
            high = max(high, neighbor);
        }
    }
    return low == 0.0 ? min(high - body, u_amount) / (1.0 - body) : 0.0;
}

void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    if (u_mode == 0) {
        vec4 source = sample_color(p);
        out_color = vec4(mix(source.rgb, u_color.rgb * source.a, u_amount), source.a);
    } else if (u_mode == 1) {
        float alpha = clamp(sample_color(p - u_offset).a * u_amount, 0.0, 1.0) * u_color.a;
        out_color = vec4(u_color.rgb * alpha, alpha);
    } else if (u_mode == 2) {
        ivec2 seed = unpack_seed(sample_at(u_source, p));
        float distance_px = length(vec2(seed - p));
        float coverage = 0.0;
        if (seed.x >= 0) {
            coverage = seed == p ? outline_under_edge(p) : clamp(u_amount + 1.0 - distance_px, 0.0, 1.0);
        }
        float alpha = coverage * u_color.a;
        out_color = vec4(u_color.rgb * alpha, alpha);
    } else if (u_mode == 3) {
        out_color = sample_at(u_source, p).a > 0.0 ? pack_seed(p) : vec4(0.0);
    } else {
        ivec2 nearest = ivec2(-1);
        float best = 1.0e20;
        for (int y = -1; y <= 1; y++) {
            for (int x = -1; x <= 1; x++) {
                ivec2 seed = unpack_seed(sample_at(u_source, p + ivec2(x, y) * u_jump));
                if (seed.x < 0) continue;
                vec2 delta = vec2(seed - p);
                float squared = dot(delta, delta);
                if (squared < best) {
                    best = squared;
                    nearest = seed;
                }
            }
        }
        out_color = pack_seed(nearest);
    }
}
"#;

pub(crate) struct GroupEffects {
    shader: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 9],
    source: Option<LayerTarget>,
    output: Option<LayerTarget>,
}

struct EffectSource {
    texture: glow::Texture,
    resample: [f32; 4],
}

impl EffectSource {
    fn unscaled(texture: glow::Texture) -> Self {
        Self {
            texture,
            resample: [0.0; 4],
        }
    }
}

impl GroupEffects {
    pub(crate) unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let shader = crate::create_fullscreen_program(gl, VERTEX, FRAGMENT)?;
        let uniforms = [
            "u_source",
            "u_body",
            "u_resample",
            "u_size",
            "u_offset",
            "u_mode",
            "u_jump",
            "u_color",
            "u_amount",
        ]
        .map(|name| gl.get_uniform_location(shader, name));
        Ok(Self {
            shader,
            uniforms,
            source: None,
            output: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn pass(
        &self,
        gl: &glow::Context,
        vao: glow::VertexArray,
        region: CallbackRegion,
        source: EffectSource,
        output: glow::Framebuffer,
        operation: i32,
        color: Rgba,
        amount: f32,
        offset: [i32; 2],
        jump: i32,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(output));
        gl.viewport(0, 0, region.width, region.height);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        if operation < 3 {
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        } else {
            gl.disable(glow::BLEND);
        }
        gl.use_program(Some(self.shader));
        gl.bind_vertex_array(Some(vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(source.texture));
        let [sampler, body, resample, size, offset_uniform, mode, jump_uniform, color_uniform, amount_uniform] =
            &self.uniforms;
        gl.uniform_1_i32(sampler.as_ref(), 0);
        gl.uniform_1_i32(body.as_ref(), i32::from(operation == 2));
        gl.uniform_4_f32_slice(resample.as_ref(), &source.resample);
        gl.uniform_2_i32(size.as_ref(), region.width, region.height);
        gl.uniform_2_i32(offset_uniform.as_ref(), offset[0], offset[1]);
        gl.uniform_1_i32(mode.as_ref(), operation);
        gl.uniform_1_i32(jump_uniform.as_ref(), jump);
        gl.uniform_4_f32(
            color_uniform.as_ref(),
            f32::from(color.r) / 255.0,
            f32::from(color.g) / 255.0,
            f32::from(color.b) / 255.0,
            f32::from(color.a) / 255.0,
        );
        gl.uniform_1_f32(amount_uniform.as_ref(), amount);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn filter(
        &mut self,
        gl: &glow::Context,
        resources: &mut GlResources,
        capture: &LayerTarget,
        clip: CallbackRegion,
        fx: PlacementFx,
        scale: f32,
        ppp: f32,
    ) -> Result<(glow::Texture, CallbackRegion), String> {
        let [w, h] = [clip.width, clip.height];
        for slot in [&mut self.source, &mut self.output] {
            if slot.as_ref().is_some_and(|target| target.size == [w, h]) {
                continue;
            }
            if let Some(old) = slot.take() {
                old.delete(gl);
            }
            *slot = Some(LayerTarget::new(gl, [w, h], false)?);
        }
        let source = self.source.as_ref().unwrap();
        let output = self.output.as_ref().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(source.texture));
        for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::LINEAR as i32);
        }
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(capture.framebuffer));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(source.framebuffer));
        gl.disable(glow::SCISSOR_TEST);
        gl.blit_framebuffer(
            clip.left,
            clip.bottom,
            clip.left + w,
            clip.bottom + h,
            0,
            0,
            w,
            h,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(output.framebuffer));
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        let local = CallbackRegion {
            left: 0,
            bottom: 0,
            ..clip
        };
        let neutral = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        if let Some(shadow) = fx
            .shadow
            .filter(|shadow| shadow.color.a != 0 && shadow.strength > 0.0)
        {
            let blur = shadow.blur_radius * scale;
            let texture =
                blur_texture(gl, resources, source, local, [blur; 2], shadow.quality, ppp);
            let offset = [
                (shadow.offset_x * scale * ppp).round() as i32,
                (-shadow.offset_y * scale * ppp).round() as i32,
            ];
            self.pass(
                gl,
                resources.vao,
                local,
                texture,
                output.framebuffer,
                1,
                shadow.color,
                shadow.strength,
                offset,
                0,
            );
        }
        if let Some(glow) = fx
            .glow
            .filter(|glow| glow.color.a != 0 && glow.strength > 0.0)
        {
            let texture = blur_texture(
                gl,
                resources,
                source,
                local,
                [glow.radius_x * scale, glow.radius_y * scale],
                glow.quality,
                ppp,
            );
            self.pass(
                gl,
                resources.vao,
                local,
                texture,
                output.framebuffer,
                1,
                glow.color,
                glow.strength,
                [0; 2],
                0,
            );
        }
        if let Some(outline) = fx
            .outline
            .filter(|outline| outline.width > 0.0 && outline.color.a != 0)
        {
            resources.ensure_filter_targets(gl, w, h);
            self.pass(
                gl,
                resources.vao,
                local,
                EffectSource::unscaled(source.texture),
                resources.filter_fbos[0],
                3,
                neutral,
                0.0,
                [0; 2],
                0,
            );
            let radius = outline.width * ppp;
            let mut jump = (radius.ceil() as u32).max(1).next_power_of_two() as i32;
            let mut slot = 0;
            while jump >= 1 {
                self.pass(
                    gl,
                    resources.vao,
                    local,
                    EffectSource::unscaled(resources.filter_textures[slot]),
                    resources.filter_fbos[1 - slot],
                    4,
                    neutral,
                    0.0,
                    [0; 2],
                    jump,
                );
                slot = 1 - slot;
                jump /= 2;
            }
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(source.texture));
            self.pass(
                gl,
                resources.vao,
                local,
                EffectSource::unscaled(resources.filter_textures[slot]),
                output.framebuffer,
                2,
                outline.color,
                radius,
                [0; 2],
                0,
            );
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
        }
        let body = if let Some(blur) = fx.blur {
            blur_texture(
                gl,
                resources,
                source,
                local,
                [blur.radius_x * scale, blur.radius_y * scale],
                blur.quality,
                ppp,
            )
        } else {
            EffectSource::unscaled(source.texture)
        };
        let (tint, amount) = fx.tint.map_or((neutral, 0.0), |tint| {
            (tint.color, tint.amount.clamp(0.0, 1.0))
        });
        self.pass(
            gl,
            resources.vao,
            local,
            body,
            output.framebuffer,
            0,
            tint,
            amount,
            [0; 2],
            0,
        );
        Ok((
            output.texture,
            CallbackRegion {
                source_uv_min: [0.0, 1.0],
                source_uv_max: [1.0, 0.0],
                ..clip
            },
        ))
    }
}

unsafe fn blur_texture(
    gl: &glow::Context,
    resources: &mut GlResources,
    source: &LayerTarget,
    region: CallbackRegion,
    radius: [f32; 2],
    quality: u8,
    ppp: f32,
) -> EffectSource {
    if radius[0].max(radius[1]) * ppp <= 0.5 {
        return EffectSource::unscaled(source.texture);
    }
    let [width, height] = [region.width, region.height];
    let passes = usize::from(quality.clamp(1, 15));
    let (mut horizontal, _) = crate::gaussian_box_radii(radius[0] * ppp, passes);
    let (mut vertical, _) = crate::gaussian_box_radii(radius[1] * ppp, passes);
    let max_radius_x = *horizontal[..passes].iter().max().unwrap();
    let max_radius_y = *vertical[..passes].iter().max().unwrap();
    resources.ensure_filter_targets(gl, width, height);

    // Wide screen-space kernels do the same work at a lower resolution.
    // Reduce each axis independently so an anisotropic glow keeps its sharp axis.
    const MAX_TAPS_RADIUS: i32 = 16;
    let (mut w, mut h) = (width, height);
    let mut input_fbo = source.framebuffer;
    let mut input_texture = source.texture;
    let mut input_size = (width, height);
    let mut slot = 1;
    loop {
        let next_w = if w > 1 && max_radius_x * w > MAX_TAPS_RADIUS * width {
            (w + 1) / 2
        } else {
            w
        };
        let next_h = if h > 1 && max_radius_y * h > MAX_TAPS_RADIUS * height {
            (h + 1) / 2
        } else {
            h
        };
        if (next_w, next_h) == (w, h) {
            break;
        }
        // Slot 0 is the destination of the first horizontal box pass.
        slot = 3 - slot;
        let destination = resources.filter_fbos[slot];
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(input_fbo));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(destination));
        gl.blit_framebuffer(
            0,
            0,
            w,
            h,
            0,
            0,
            next_w,
            next_h,
            glow::COLOR_BUFFER_BIT,
            glow::LINEAR,
        );
        (w, h) = (next_w, next_h);
        input_fbo = destination;
        input_texture = resources.filter_textures[slot];
        input_size = resources.filter_size;
    }

    let resized = (w, h) != (width, height);
    if resized {
        for (axis, factor) in [
            (&mut horizontal, w as f32 / width as f32),
            (&mut vertical, h as f32 / height as f32),
        ] {
            for radius in &mut axis[..passes] {
                *radius = (((*radius as f32 + 0.5) * factor - 0.5).round() as i32).max(0);
            }
        }
    }
    let texture = resources.box_blur_group_texture(
        gl,
        input_texture,
        input_size,
        CallbackRegion {
            width: w,
            height: h,
            ..region
        },
        &horizontal[..passes],
        &vertical[..passes],
    );
    if !resized {
        return EffectSource::unscaled(texture);
    }
    let texel_x = 1.0 / resources.filter_size.0 as f32;
    let texel_y = 1.0 / resources.filter_size.1 as f32;
    EffectSource {
        texture,
        resample: [
            w as f32 / width as f32 * texel_x,
            h as f32 / height as f32 * texel_y,
            texel_x,
            texel_y,
        ],
    }
}
