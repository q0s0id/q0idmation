//! Browser bindings for the same Player, qvm, renderer and simulations as desktop.
//! Page chrome and user-gesture/audio policy belong to the embedding web page.

use q0s_format::particles::ParticleLook;
use q0s_format::v2::{Layer, Placement, ProjectV2, Q0rg, Transform2D, Tween};
use wasm_bindgen::prelude::*;

use crate::{sample_audio_timeline, Inner, Player, MAX_Q0S_FILE_BYTES, PLAYER_AUDIO_SAMPLE_RATE};

#[cfg(feature = "browser-gpu")]
#[path = "web_controls.rs"]
mod controls;

#[wasm_bindgen]
pub struct BrowserPlayer {
    player: Player,
    source: Vec<u8>,
    item_project: Option<(u64, u16, ProjectV2)>,
    #[cfg(feature = "browser-gpu")]
    gpu: Option<BrowserGpu>,
    #[cfg(feature = "browser-gpu")]
    controls: Option<controls::Transport>,
}

#[wasm_bindgen]
impl BrowserPlayer {
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<BrowserPlayer, JsError> {
        if bytes.len() as u64 > MAX_Q0S_FILE_BYTES {
            return Err(JsError::new("this movie is too large"));
        }
        let player = if bytes.starts_with(&q0video::q0v::MAGIC) {
            Player::from_q0v_bytes(bytes.to_vec()).map_err(|e| JsError::new(&e.to_string()))?
        } else {
            Player::from_bytes(bytes).map_err(|e| JsError::new(&e.to_string()))?
        };
        Ok(Self {
            player,
            source: bytes.to_vec(),
            item_project: None,
            #[cfg(feature = "browser-gpu")]
            gpu: None,
            #[cfg(feature = "browser-gpu")]
            controls: None,
        })
    }

    pub fn info(&self) -> String {
        let (width, height) = self.stage_size();
        serde_json::json!({
            "width": width, "height": height, "fps": self.player.fps(),
            "frames": self.player.total_frames(), "game": self.player.is_game(),
            "audio": self.player.has_audio_timeline(), "sampleRate": PLAYER_AUDIO_SAMPLE_RATE,
            "channels": crate::PLAYER_AUDIO_CHANNELS,
            "mouse": self.player.uses_mouse_events(),
        })
        .to_string()
    }

    pub fn frame(&self) -> u32 {
        self.player.current_frame() as u32
    }
    pub fn playing(&self) -> bool {
        self.player.is_playing()
    }
    pub fn scene_generation(&self) -> u32 {
        self.player.scene_generation() as u32
    }
    pub fn loop_enabled(&self) -> bool {
        self.player.is_loop_enabled()
    }
    pub fn set_loop(&mut self, value: bool) {
        self.player.set_loop_enabled(value);
    }
    pub fn set_playing(&mut self, value: bool) {
        self.player.set_playing(value);
    }
    pub fn seek(&mut self, frame: u32) {
        let playing = self.player.is_playing();
        self.player.seek_to_frame(frame as usize);
        self.player.set_playing(playing);
    }
    pub fn restart(&mut self) -> Result<(), JsError> {
        let looping = self.player.is_loop_enabled();
        self.player = if self.source.starts_with(&q0video::q0v::MAGIC) {
            Player::from_q0v_bytes(self.source.clone()).map_err(|e| JsError::new(&e.to_string()))?
        } else {
            Player::from_bytes(&self.source).map_err(|e| JsError::new(&e.to_string()))?
        };
        self.player.set_loop_enabled(looping);
        self.item_project = None;
        #[cfg(feature = "browser-gpu")]
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.cache.invalidate();
            gpu.bitmaps.clear();
            gpu.video = None;
            gpu.generation = self.player.scene_generation();
        }
        Ok(())
    }
    pub fn tick(&mut self, seconds: f32) -> u32 {
        self.player.tick(if seconds.is_finite() {
            seconds.clamp(0.0, 0.25)
        } else {
            0.0
        }) as u32
    }
    pub fn input(&mut self, left: bool, right: bool, jump: bool) {
        self.player.set_input_state(left, right, jump);
    }
    pub fn pointer(
        &mut self,
        event: &str,
        x: f32,
        y: f32,
        inside: bool,
        wheel_x: f32,
        wheel_y: f32,
    ) {
        self.player
            .dispatch_pointer_event(event, x, y, inside, wheel_x, wheel_y);
    }

    #[cfg(feature = "browser-gpu")]
    pub fn attach_controls(&mut self, canvas: web_sys::HtmlCanvasElement) -> Result<(), JsError> {
        self.controls = Some(controls::Transport::new(canvas)?);
        Ok(())
    }

    #[cfg(feature = "browser-gpu")]
    pub fn controls_pointer(&mut self, kind: u8, x: f32, y: f32) {
        if let Some(controls) = self.controls.as_mut() {
            controls.pointer(kind, x, y);
        }
    }

    #[cfg(feature = "browser-gpu")]
    pub fn paint_controls(&mut self, width: u32, height: u32, density: f32, touch: bool) -> u8 {
        self.controls.as_mut().map_or(0, |controls| {
            controls.paint(&mut self.player, width, height, density, touch)
        })
    }

    #[cfg(feature = "browser-gpu")]
    pub fn controls_height(&self) -> f32 {
        self.controls
            .as_ref()
            .map_or(73.0, |controls| controls.height)
    }

    #[cfg(feature = "browser-gpu")]
    pub fn controls_visible(&mut self, now: f64, inside: bool, active: bool, held: bool) -> bool {
        let playing = self.player.is_playing();
        !self.player.is_game()
            && self.controls.as_mut().is_some_and(|controls| {
                controls
                    .reveal
                    .visible(now, playing, inside, active, held, false)
            })
    }

    #[cfg(feature = "browser-gpu")]
    pub fn attach_canvas(&mut self, canvas: web_sys::HtmlCanvasElement) -> Result<u32, JsError> {
        use glow::HasContext;
        use wasm_bindgen::JsCast;
        let options = js_sys::Object::new();
        for (name, value) in [
            ("antialias", true),
            ("alpha", true),
            ("depth", false),
            ("stencil", false),
            ("preserveDrawingBuffer", false),
        ] {
            js_sys::Reflect::set(&options, &name.into(), &value.into())
                .map_err(|_| JsError::new("could not configure webgl"))?;
        }
        let webgl = canvas
            .get_context_with_context_options("webgl2", &options)
            .map_err(|_| JsError::new("could not create webgl2 context"))?
            .ok_or_else(|| JsError::new("webgl2 is required for vector playback"))?
            .dyn_into::<web_sys::WebGl2RenderingContext>()
            .map_err(|_| JsError::new("could not access webgl2"))?;
        let gl = std::sync::Arc::new(glow::Context::from_webgl2_context(webgl));
        let max_side = unsafe { gl.get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE) }.max(1) as u32;
        let painter = egui_glow::Painter::new(gl, "", None)
            .map_err(|error| JsError::new(&error.to_string()))?;
        self.gpu = Some(BrowserGpu {
            ctx: egui::Context::default(),
            painter,
            cache: Default::default(),
            generation: self.player.scene_generation(),
            bitmaps: Default::default(),
            video: None,
            time: 0.0,
        });
        Ok(max_side)
    }

    /// Tessellate cached authored vectors and draw them directly with WebGL.
    #[cfg(feature = "browser-gpu")]
    pub fn paint(&mut self, width: u32, height: u32) -> Result<(), JsError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let stage_size = self.stage_size();
        let gpu = self
            .gpu
            .as_mut()
            .ok_or_else(|| JsError::new("vector renderer is not attached"))?;
        if gpu.generation != self.player.scene_generation() {
            gpu.cache.invalidate();
            gpu.bitmaps.clear();
            gpu.video = None;
            gpu.generation = self.player.scene_generation();
        }
        let target =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32));
        gpu.time += 1.0 / f64::from(self.player.fps().max(1));
        let raw = egui::RawInput {
            screen_rect: Some(target),
            time: Some(gpu.time),
            ..Default::default()
        };
        let player = &self.player;
        let output = gpu.ctx.run(raw, |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("movie"),
            ));
            match &player.inner {
                Inner::V2 {
                    project,
                    entry_q0rg_id,
                    frame_index,
                    ..
                } => {
                    crate::browser_render::paint_v2_frame_with_runtime_overrides(
                        &painter,
                        ctx,
                        project,
                        *entry_q0rg_id,
                        *frame_index,
                        target,
                        &mut gpu.cache,
                        true,
                        &player.rig_runtime_overrides,
                        &player.scene_runtime,
                    );
                    if player.has_live_effects() {
                        crate::browser_render::paint_live_draws(
                            &painter,
                            ctx,
                            project,
                            target,
                            &mut gpu.cache,
                            &player.live_draws(),
                            &player.rig_runtime_overrides,
                            &player.scene_runtime,
                        );
                    }
                }
                Inner::V1 { movie, frame_index } => {
                    let bg = &movie.background;
                    painter.rect_filled(
                        target,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a),
                    );
                    let scale = egui::vec2(
                        width as f32 / stage_size.0 as f32,
                        height as f32 / stage_size.1 as f32,
                    );
                    for placement in movie
                        .placements_by_frame
                        .get(*frame_index)
                        .into_iter()
                        .flatten()
                    {
                        let Some(bitmap) = movie.bitmaps.get(&placement.bitmap_id) else {
                            continue;
                        };
                        let texture = gpu.bitmaps.entry(bitmap.id).or_insert_with(|| {
                            ctx.load_texture(
                                format!("bitmap-{}", bitmap.id),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [bitmap.width as usize, bitmap.height as usize],
                                    &bitmap.rgba,
                                ),
                                egui::TextureOptions::LINEAR,
                            )
                        });
                        let min = egui::pos2(
                            f32::from(placement.x) * scale.x,
                            f32::from(placement.y) * scale.y,
                        );
                        let size = egui::vec2(
                            f32::from(bitmap.width) * placement.scale_x * scale.x,
                            f32::from(bitmap.height) * placement.scale_y * scale.y,
                        );
                        painter.image(
                            texture.id(),
                            egui::Rect::from_min_size(min, size),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
                Inner::Q0v { media, frame_index } => {
                    if let Ok(rgba) = media.decode_frame_rgba(*frame_index as usize) {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [media.spec.width as usize, media.spec.height as usize],
                            &rgba,
                        );
                        if let Some(texture) = gpu.video.as_mut() {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            gpu.video = Some(ctx.load_texture(
                                "video",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        painter.image(
                            gpu.video.as_ref().unwrap().id(),
                            target,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
            }
        });
        let primitives = gpu.ctx.tessellate(output.shapes, output.pixels_per_point);
        gpu.painter.clear([width, height], [0.0, 0.0, 0.0, 1.0]);
        gpu.painter.paint_and_update_textures(
            [width, height],
            output.pixels_per_point,
            &primitives,
            &output.textures_delta,
        );
        if let Some(error) = q0glblend::take_webgl_error() {
            return Err(JsError::new(&error));
        }
        Ok(())
    }

    pub fn render(&self, width: u32, height: u32) -> Result<Vec<u8>, JsError> {
        checked_size(width, height)?;
        if let Inner::V2 {
            project,
            entry_q0rg_id,
            frame_index,
            ..
        } = &self.player.inner
        {
            return Ok(q0s_format::raster::rasterize_q0rg_frame_with_runtime_state(
                project,
                *entry_q0rg_id,
                *frame_index,
                width,
                height,
                1,
                [
                    project.meta.background_color.r,
                    project.meta.background_color.g,
                    project.meta.background_color.b,
                    project.meta.background_color.a,
                ],
                &self.player.rig_runtime_overrides,
                &self.player.scene_runtime,
            ));
        }
        let (native_width, native_height) = self.stage_size();
        checked_size(native_width, native_height)?;
        let mut native = vec![0; native_width as usize * native_height as usize * 4];
        self.player.render(&mut native, native_width, native_height);
        if (width, height) == (native_width, native_height) {
            return Ok(native);
        }
        let mut output = vec![0; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let source = ((y * native_height / height * native_width
                    + x * native_width / width)
                    * 4) as usize;
                let destination = ((y * width + x) * 4) as usize;
                output[destination..destination + 4].copy_from_slice(&native[source..source + 4]);
            }
        }
        Ok(output)
    }

    /// Geometry for browser Canvas2D overlays, from the native live simulations.
    pub fn live_draws(&self) -> String {
        let live = self.player.live_draws();
        let color = |c: q0s_format::v2::Rgba| [c.r, c.g, c.b, c.a];
        serde_json::json!({
            "liquids": live.liquids.iter().map(|(paths, c)| serde_json::json!([paths, color(*c)])).collect::<Vec<_>>(),
            "links": live.links.iter().flat_map(|(strokes, c)| strokes.iter().map(move |s| serde_json::json!({
                "points": s.points, "closed": s.closed, "width": s.width, "color": color(*c)
            }))).collect::<Vec<_>>(),
            "particles": live.particles.iter().filter(|p| !matches!(p.look, ParticleLook::Item(_))).map(|p| serde_json::json!({
                "look": format!("{:?}", p.look).to_ascii_lowercase(), "position": p.position,
                "size": p.style.size, "rotation": p.style.rotation, "stretch": p.style.stretch,
                "additive": p.style.additive, "color": color(p.style.color)
            })).collect::<Vec<_>>(),
        }).to_string()
    }

    /// Library-item particles use the shared vector renderer, including nested symbols.
    pub fn render_live_items(&mut self, width: u32, height: u32) -> Result<Vec<u8>, JsError> {
        checked_size(width, height)?;
        let live = self.player.live_draws();
        let placements = live
            .particles
            .iter()
            .filter_map(|particle| {
                let ParticleLook::Item(target) = particle.look else {
                    return None;
                };
                Some(Placement {
                    instance_id: 0,
                    pivot: None,
                    frame: 0,
                    target,
                    transform: Transform2D {
                        tx: particle.position[0],
                        ty: particle.position[1],
                        sx: (particle.style.size * particle.style.stretch).max(0.001),
                        sy: particle.style.size.max(0.001),
                        rotation: particle.style.rotation,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: Default::default(),
                })
            })
            .collect::<Vec<_>>();
        if placements.is_empty() {
            return Ok(Vec::new());
        }
        let Some(project) = self.player.project_v2() else {
            return Ok(Vec::new());
        };
        let generation = self.player.scene_generation();
        if self
            .item_project
            .as_ref()
            .is_none_or(|(old, _, _)| *old != generation)
        {
            let id = (1..=u16::MAX)
                .rev()
                .find(|id| project.q0rgs.iter().all(|q| q.q0rg_id != *id))
                .ok_or_else(|| JsError::new("no free particle render timeline"))?;
            let mut copy = project.clone();
            copy.q0rgs.push(Q0rg {
                q0rg_id: id,
                name: String::new(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: String::new(),
                    explicit_keyframes: vec![0],
                    placements: Vec::new(),
                }],
            });
            self.item_project = Some((generation, id, copy));
        }
        let (_, id, project) = self.item_project.as_mut().unwrap();
        project.q0rgs.last_mut().unwrap().layers[0].placements = placements;
        Ok(q0s_format::raster::rasterize_q0rg_frame_with_runtime_state(
            project,
            *id,
            0,
            width,
            height,
            1,
            [0, 0, 0, 0],
            &self.player.rig_runtime_overrides,
            &self.player.scene_runtime,
        ))
    }

    /// Stereo PCM chunks mixed by the exact desktop audio mixer at 48 kHz.
    pub fn audio(&self, start: u32, frames: u32) -> Vec<f32> {
        let Some(timeline) = self.player.audio_timeline() else {
            return Vec::new();
        };
        let frames = frames.min(16_384);
        let mut samples = Vec::with_capacity(frames as usize * 2);
        for offset in 0..frames {
            let mut position = u64::from(start) + u64::from(offset);
            if self.player.is_loop_enabled() {
                position %= timeline.total_sample_frames.max(1);
            }
            for channel in 0..2 {
                samples.push(sample_audio_timeline(&timeline, position, channel));
            }
        }
        samples
    }

    /// Useful to embedders and integration checks, never rendered as player chrome.
    pub fn instance_metrics(&self, name: &str) -> Vec<f32> {
        self.player
            .runtime_instance_metrics(name)
            .map(|m| vec![m.x, m.y, m.left, m.right, m.top, m.bottom])
            .unwrap_or_default()
    }
}

impl BrowserPlayer {
    fn stage_size(&self) -> (u32, u32) {
        match &self.player.inner {
            Inner::V2 { project, .. } => (
                u32::from(project.meta.stage_width.max(1)),
                u32::from(project.meta.stage_height.max(1)),
            ),
            Inner::Q0v { media, .. } => (media.spec.width.max(1), media.spec.height.max(1)),
            Inner::V1 { movie, .. } => {
                let mut width = 1.0_f32;
                let mut height = 1.0_f32;
                for placement in movie.placements_by_frame.iter().flatten() {
                    if let Some(bitmap) = movie.bitmaps.get(&placement.bitmap_id) {
                        width = width.max(
                            f32::from(placement.x) + f32::from(bitmap.width) * placement.scale_x,
                        );
                        height = height.max(
                            f32::from(placement.y) + f32::from(bitmap.height) * placement.scale_y,
                        );
                    }
                }
                (
                    width.ceil().clamp(1.0, 4096.0) as u32,
                    height.ceil().clamp(1.0, 4096.0) as u32,
                )
            }
        }
    }
}

fn checked_size(width: u32, height: u32) -> Result<(), JsError> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 4_194_304 {
        return Err(JsError::new("movie render dimensions are too large"));
    }
    Ok(())
}

#[cfg(feature = "browser-gpu")]
struct BrowserGpu {
    ctx: egui::Context,
    painter: egui_glow::Painter,
    cache: crate::browser_render::TextureCache,
    generation: u64,
    bitmaps: std::collections::HashMap<u16, egui::TextureHandle>,
    video: Option<egui::TextureHandle>,
    time: f64,
}

#[cfg(feature = "browser-gpu")]
impl Drop for BrowserGpu {
    fn drop(&mut self) {
        self.painter.destroy();
    }
}
