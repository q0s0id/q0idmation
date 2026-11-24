//! A GPU render target for one faded layer. The ordinary painter continues
//! submitting meshes and callbacks into it in their existing order.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui::{PaintCallback, Painter, Shape};
use glow::HasContext as _;
use q0s_format::v2::{BlendMode, PlacementFx};

use crate::{callback_region, CallbackRegion, GlResources, GL_RESOURCES};

pub(crate) struct LayerTarget {
    pub(crate) size: [i32; 2],
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) texture: glow::Texture,
    msaa: Option<(glow::Framebuffer, glow::Renderbuffer)>,
}

impl LayerTarget {
    pub(crate) unsafe fn new(
        gl: &glow::Context,
        size: [i32; 2],
        multisample: bool,
    ) -> Result<Self, String> {
        let [w, h] = size;
        let framebuffer = gl.create_framebuffer()?;
        let texture = match gl.create_texture() {
            Ok(texture) => texture,
            Err(error) => {
                gl.delete_framebuffer(framebuffer);
                return Err(error);
            }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
        for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::NEAREST as i32);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_texture(texture);
            gl.delete_framebuffer(framebuffer);
            return Err("layer opacity framebuffer is incomplete".into());
        }
        let samples = gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(0, 4);
        let msaa = if multisample && samples > 1 {
            create_msaa(gl, w, h, samples)
        } else {
            None
        };
        Ok(Self {
            size,
            framebuffer,
            texture,
            msaa,
        })
    }

    fn draw_framebuffer(&self) -> glow::Framebuffer {
        self.msaa
            .map_or(self.framebuffer, |(framebuffer, _)| framebuffer)
    }

    unsafe fn resolve(&self, gl: &glow::Context, clip: CallbackRegion) {
        if let Some((framebuffer, _)) = self.msaa {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.framebuffer));
            let (right, top) = (clip.left + clip.width, clip.bottom + clip.height);
            gl.blit_framebuffer(
                clip.left,
                clip.bottom,
                right,
                top,
                clip.left,
                clip.bottom,
                right,
                top,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
    }

    pub(crate) unsafe fn delete(self, gl: &glow::Context) {
        gl.delete_framebuffer(self.framebuffer);
        gl.delete_texture(self.texture);
        if let Some((framebuffer, color)) = self.msaa {
            gl.delete_framebuffer(framebuffer);
            gl.delete_renderbuffer(color);
        }
    }
}

unsafe fn create_msaa(
    gl: &glow::Context,
    w: i32,
    h: i32,
    samples: i32,
) -> Option<(glow::Framebuffer, glow::Renderbuffer)> {
    let framebuffer = gl.create_framebuffer().ok()?;
    let color = match gl.create_renderbuffer() {
        Ok(color) => color,
        Err(_) => {
            gl.delete_framebuffer(framebuffer);
            return None;
        }
    };
    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
    gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::RGBA8, w, h);
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
    gl.framebuffer_renderbuffer(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::RENDERBUFFER,
        Some(color),
    );
    if gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
        Some((framebuffer, color))
    } else {
        gl.delete_renderbuffer(color);
        gl.delete_framebuffer(framebuffer);
        None
    }
}

struct Pass {
    target: LayerTarget,
    parent: Option<glow::Framebuffer>,
}

#[derive(Default)]
struct Layers {
    active: Vec<Option<Pass>>,
    spare: Vec<LayerTarget>,
    masks: Vec<Option<LayerTarget>>,
    effects: Option<crate::group_effects::GroupEffects>,
}

thread_local! {
    static LAYERS: RefCell<HashMap<usize, Layers>> = RefCell::new(HashMap::new());
    static ACTIVE_GROUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn draw_framebuffer(painter: &egui_glow::Painter) -> Option<glow::Framebuffer> {
    if ACTIVE_GROUPS.with(|active| active.get() != 0) {
        let key = Arc::as_ptr(painter.gl()) as usize;
        if let Some(target) = LAYERS.with(|layers| {
            layers.borrow().get(&key).and_then(|stack| {
                stack
                    .active
                    .iter()
                    .rev()
                    .flatten()
                    .next()
                    .map(|pass| pass.target.draw_framebuffer())
            })
        }) {
            return Some(target);
        }
    }
    painter.intermediate_fbo()
}

/// Ends an opacity group on the same painter, including on early returns.
#[must_use]
pub struct LayerOpacity<'a> {
    painter: &'a Painter,
    output: GroupOutput,
}

#[derive(Clone, Copy)]
enum GroupOutput {
    Paint {
        fx: PlacementFx,
        scale: f32,
        masked: bool,
    },
    Mask,
}

/// Keeps the intersection of the layer's masks on the GPU until its art is drawn.
#[must_use]
pub struct LayerMasks<'a> {
    painter: &'a Painter,
}

fn add_mask_callback(painter: &Painter, callback: egui_glow::CallbackFn) {
    let rect = painter.clip_rect();
    painter.add(Shape::Callback(PaintCallback {
        rect,
        callback: Arc::new(callback),
    }));
}

pub fn begin_layer_masks(painter: &Painter) -> LayerMasks<'_> {
    add_mask_callback(
        painter,
        egui_glow::CallbackFn::new(|_, painter| {
            let key = Arc::as_ptr(painter.gl()) as usize;
            LAYERS.with(|layers| layers.borrow_mut().entry(key).or_default().masks.push(None));
        }),
    );
    LayerMasks { painter }
}

impl LayerMasks<'_> {
    pub fn capture(&self) -> Option<LayerOpacity<'_>> {
        begin_group(self.painter, GroupOutput::Mask)
    }

    pub fn placement(&self, fx: PlacementFx, scale: f32) -> Option<LayerOpacity<'_>> {
        begin_group(
            self.painter,
            GroupOutput::Paint {
                fx,
                scale,
                masked: true,
            },
        )
    }
}

impl Drop for LayerMasks<'_> {
    fn drop(&mut self) {
        add_mask_callback(
            self.painter,
            egui_glow::CallbackFn::new(|_, painter| {
                let key = Arc::as_ptr(painter.gl()) as usize;
                LAYERS.with(|layers| {
                    if let Some(stack) = layers.borrow_mut().get_mut(&key) {
                        if let Some(Some(mask)) = stack.masks.pop() {
                            stack.spare.push(mask);
                        }
                    }
                });
            }),
        );
    }
}

unsafe fn store_mask(gl: &glow::Context, key: usize, target: LayerTarget, clip: CallbackRegion) {
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        let stack = layers.get_mut(&key).expect("active GPU layer");
        let mask = stack.masks.last_mut().expect("active GPU mask");
        let Some(combined) = mask.as_ref() else {
            *mask = Some(target);
            return;
        };
        GL_RESOURCES.with(|resources| {
            let mut resources = resources.borrow_mut();
            let resources = resources.get_mut(&key).expect("active GPU compositor");
            resources.ensure_filter_targets(gl, target.size[0], target.size[1]);
            resources.multiply_mask_textures(gl, combined.texture, target.texture, 0, clip);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(resources.filter_fbos[0]));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(combined.framebuffer));
            let (right, top) = (clip.left + clip.width, clip.bottom + clip.height);
            gl.blit_framebuffer(
                clip.left,
                clip.bottom,
                right,
                top,
                clip.left,
                clip.bottom,
                right,
                top,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        });
        stack.spare.push(target);
    });
}

pub fn begin_layer_opacity(painter: &Painter, opacity: f32) -> Option<LayerOpacity<'_>> {
    begin_group_effects(
        painter,
        PlacementFx {
            opacity,
            ..PlacementFx::default()
        },
        1.0,
    )
}

pub fn begin_group_effects(
    painter: &Painter,
    fx: PlacementFx,
    scale: f32,
) -> Option<LayerOpacity<'_>> {
    if fx.is_identity() {
        return None;
    }
    begin_group(
        painter,
        GroupOutput::Paint {
            fx,
            scale,
            masked: false,
        },
    )
}

fn begin_group(painter: &Painter, output: GroupOutput) -> Option<LayerOpacity<'_>> {
    let rect = painter.clip_rect();
    if !rect.is_finite() || !rect.is_positive() {
        return None;
    }
    crate::PAINT_DIAGNOSTICS.with(|diagnostics| {
        let mut diagnostics = diagnostics.borrow_mut();
        diagnostics.layer_opacity_groups = diagnostics.layer_opacity_groups.saturating_add(1);
    });
    let callback = egui_glow::CallbackFn::new(move |info, painter| {
        let gl = painter.gl();
        let key = Arc::as_ptr(gl) as usize;
        let clip = callback_region(
            info.viewport,
            info.clip_rect,
            info.pixels_per_point,
            info.screen_size_px,
        );
        let pass = clip.and_then(|clip| unsafe {
            let parent =
                NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                    .map(glow::NativeFramebuffer);
            let available = GL_RESOURCES.with(|resources| {
                let mut resources = resources.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) = resources.entry(key) {
                    if crate::gl_unavailable(key) {
                        return false;
                    }
                    match GlResources::new(gl) {
                        Ok(created) => {
                            entry.insert(created);
                        }
                        Err(error) => {
                            crate::mark_gl_unavailable(key, &error);
                            return false;
                        }
                    }
                }
                true
            });
            if !available {
                gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                return None;
            }
            let size = info.screen_size_px.map(|size| size as i32);
            let target = LAYERS.with(|layers| {
                let mut layers = layers.borrow_mut();
                let stack = layers.entry(key).or_default();
                match stack.spare.pop() {
                    Some(target) if target.size == size => Ok(target),
                    old => {
                        if let Some(old) = old {
                            old.delete(gl);
                        }
                        LayerTarget::new(gl, size, true)
                    }
                }
            });
            let target = match target {
                Ok(target) => target,
                Err(error) => {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                    eprintln!("q0glblend: {error}");
                    return None;
                }
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.draw_framebuffer()));
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip.left, clip.bottom, clip.width, clip.height);
            gl.color_mask(true, true, true, true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            Some(Pass { target, parent })
        });
        LAYERS.with(|layers| {
            layers
                .borrow_mut()
                .entry(key)
                .or_default()
                .active
                .push(pass)
        });
        ACTIVE_GROUPS.with(|active| active.set(active.get() + 1));
    });
    if let GroupOutput::Paint { fx, .. } = output {
        let blur_passes = |quality: u8| 2 * u64::from(quality.clamp(1, 15));
        let blur = fx
            .blur
            .filter(|v| v.radius_x.max(v.radius_y) > 0.0)
            .map_or(0, |v| blur_passes(v.quality));
        let glow = fx
            .glow
            .filter(|v| v.strength > 0.0 && v.color.a != 0)
            .map_or(0, |v| blur_passes(v.quality));
        let shadow = fx
            .shadow
            .filter(|v| v.strength > 0.0 && v.color.a != 0)
            .map_or(0, |v| {
                1 + if v.blur_radius > 0.0 {
                    blur_passes(v.quality)
                } else {
                    0
                }
            });
        let outline = fx
            .outline
            .filter(|v| v.width > 0.0 && v.color.a != 0)
            .map_or(0, |v| {
                let jump = (v.width * painter.ctx().pixels_per_point()).ceil() as u32;
                3 + u64::from(jump.max(1).next_power_of_two().trailing_zeros())
            });
        crate::record_filter_work(painter, rect, blur, glow, blur + glow + shadow + outline);
    }
    painter.add(Shape::Callback(PaintCallback {
        rect,
        callback: Arc::new(callback),
    }));
    Some(LayerOpacity { painter, output })
}

impl Drop for LayerOpacity<'_> {
    fn drop(&mut self) {
        let rect = self.painter.clip_rect();
        let output = self.output;
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let gl = painter.gl();
            let key = Arc::as_ptr(gl) as usize;
            let pass = LAYERS.with(|layers| {
                layers
                    .borrow_mut()
                    .get_mut(&key)
                    .and_then(|stack| stack.active.pop())
            });
            let Some(pass) = pass else { return };
            ACTIVE_GROUPS.with(|active| active.set(active.get().saturating_sub(1)));
            let Some(Pass { target, parent }) = pass else {
                return;
            };
            unsafe {
                if let Some(mut clip) = callback_region(
                    info.viewport,
                    info.clip_rect,
                    info.pixels_per_point,
                    info.screen_size_px,
                ) {
                    gl.disable(glow::SCISSOR_TEST);
                    target.resolve(gl, clip);
                    let GroupOutput::Paint { fx, scale, masked } = output else {
                        store_mask(gl, key, target, clip);
                        gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                        return;
                    };
                    gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                    let mask = if masked {
                        LAYERS.with(|layers| {
                            layers.borrow().get(&key).and_then(|stack| {
                                stack.masks.last()?.as_ref().map(|mask| mask.texture)
                            })
                        })
                    } else {
                        None
                    };
                    let [w, h] = target.size.map(|size| size as f32);
                    // The compositor's source UVs go from screen top to bottom,
                    // while the captured framebuffer uses OpenGL coordinates.
                    clip.source_uv_min =
                        [clip.left as f32 / w, (clip.bottom + clip.height) as f32 / h];
                    clip.source_uv_max =
                        [(clip.left + clip.width) as f32 / w, clip.bottom as f32 / h];
                    GL_RESOURCES.with(|resources| {
                        if let Some(resources) = resources.borrow_mut().get_mut(&key) {
                            if masked && mask.is_none() {
                                return;
                            }
                            let mut texture = target.texture;
                            let mut body_only = fx;
                            body_only.opacity = 1.0;
                            body_only.blend_mode = BlendMode::Normal;
                            if !body_only.is_identity() {
                                resources.gpu_filter_timer.poll_one(gl);
                                let query = resources.gpu_filter_timer.begin_sample(gl, true);
                                let filtered = LAYERS.with(|layers| {
                                    let mut layers = layers.borrow_mut();
                                    let stack = layers.get_mut(&key).expect("active GPU layer");
                                    if stack.effects.is_none() {
                                        stack.effects =
                                            Some(crate::group_effects::GroupEffects::new(gl)?);
                                    }
                                    stack.effects.as_mut().unwrap().filter(
                                        gl,
                                        resources,
                                        &target,
                                        clip,
                                        fx,
                                        scale,
                                        info.pixels_per_point,
                                    )
                                });
                                if let Some(index) = query {
                                    resources.gpu_filter_timer.end_sample(gl, index);
                                }
                                match filtered {
                                    Ok((filtered_texture, filtered_clip)) => {
                                        texture = filtered_texture;
                                        clip = filtered_clip;
                                    }
                                    Err(error) => eprintln!("q0glblend: {error}"),
                                }
                            }
                            gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                            resources.target_framebuffer = parent;
                            let mode = crate::blend_mode_tag(fx.blend_mode);
                            if fx.blend_mode == BlendMode::Normal {
                                resources.paint_opaque_fixed(
                                    gl, texture, mode, clip, false, fx.opacity, mask,
                                );
                            } else {
                                resources.paint(gl, texture, mode, clip, false, fx.opacity, mask);
                            }
                        }
                    });
                } else {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, parent);
                }
            }
            LAYERS.with(|layers| {
                layers
                    .borrow_mut()
                    .entry(key)
                    .or_default()
                    .spare
                    .push(target)
            });
        });
        crate::record_paint_callback(self.painter, rect, None);
        self.painter.add(Shape::Callback(PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }
}
