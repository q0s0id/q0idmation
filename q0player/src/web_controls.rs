//! Browser transport drawn by the player's own egui widgets and theme.
//! Browser fullscreen/audio are returned as actions to the page host.
#[allow(dead_code)]
#[path = "ui/atomic_file.rs"]
mod atomic_file;
#[allow(dead_code)]
#[path = "ui/chrome.rs"]
mod chrome;
#[allow(dead_code)]
#[path = "ui/theme.rs"]
mod theme;

use crate::Player;
use egui::{Context, Event, PointerButton, Pos2, Rect};
use wasm_bindgen::{JsCast, JsError};

// Browser-only palette, matching the site's public/assets/css/q0id.css tokens.
const Q0ID_THEME: q0theme::BuiltinTheme = q0theme::BuiltinTheme {
    id: "q0id",
    name: "q0id",
    accent: q0theme::Rgb::new(0xb3, 0x20, 0x1f),
    panel: q0theme::Rgb::new(0x13, 0x0c, 0x0c),
    window: q0theme::Rgb::new(0x0e, 0x09, 0x09),
    deep_bg: q0theme::Rgb::new(0x07, 0x05, 0x05),
    text: q0theme::Rgb::new(0xe8, 0xdd, 0xd8),
    text_dim: q0theme::Rgb::new(0xa8, 0x95, 0x8f),
    stroke_dark: q0theme::Rgb::new(0x3b, 0x1a, 0x1a),
    viewport_bg: q0theme::Rgb::new(0x0b, 0x08, 0x08),
};

pub(super) struct Transport {
    pub reveal: crate::transport::Reveal,
    ctx: Context,
    painter: egui_glow::Painter,
    events: Vec<Event>,
    scrub: Option<bool>,
    time: f64,
    pub height: f32,
}

impl Transport {
    pub fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsError> {
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"alpha".into(), &false.into())
            .map_err(|_| JsError::new("webgl options"))?;
        let webgl = canvas
            .get_context_with_context_options("webgl2", &options)
            .map_err(|_| JsError::new("could not create transport renderer"))?
            .ok_or_else(|| JsError::new("webgl2 is required"))?
            .dyn_into::<web_sys::WebGl2RenderingContext>()
            .map_err(|_| JsError::new("webgl2 context"))?;
        let gl = std::sync::Arc::new(glow::Context::from_webgl2_context(webgl));
        let painter =
            egui_glow::Painter::new(gl, "", None).map_err(|e| JsError::new(&e.to_string()))?;
        let ctx = Context::default();
        theme::Theme::from_builtin(&Q0ID_THEME).apply(&ctx);
        Ok(Self {
            reveal: Default::default(),
            ctx,
            painter,
            events: Vec::new(),
            scrub: None,
            time: 0.0,
            height: 73.0,
        })
    }

    pub fn pointer(&mut self, kind: u8, x: f32, y: f32) {
        let pos = egui::pos2(x, y);
        self.events.push(match kind {
            1 | 2 => Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: kind == 1,
                modifiers: Default::default(),
            },
            3 => Event::PointerGone,
            _ => Event::PointerMoved(pos),
        });
    }

    pub fn paint(
        &mut self,
        player: &mut Player,
        width: u32,
        height: u32,
        density: f32,
        touch: bool,
    ) -> u8 {
        if width == 0 || height == 0 {
            return 0;
        }
        let density = density.max(0.1);
        let size = egui::vec2(width as f32 / density, height as f32 / density);
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(density);
        let mut actions = 0;
        let output = self.ctx.run(raw, |ctx| {
            let total = player.total_frames().max(1);
            let current = player.current_frame();
            let fps = player.fps();
            let playing = player.is_playing();
            let mut markers: Vec<_> = player
                .project_v2()
                .into_iter()
                .flat_map(|project| {
                    project
                        .runtime
                        .frame_labels
                        .iter()
                        .filter(|(key, _)| key.q0rg_id == project.meta.entry_q0rg_id)
                        .map(|(key, name)| chrome::Marker {
                            frame: usize::from(key.frame),
                            name: name.clone(),
                        })
                })
                .collect();
            markers.sort_by_key(|marker| marker.frame);
            let frame = egui::Frame::side_top_panel(&ctx.style())
                .inner_margin(egui::Margin::symmetric(12.0, if touch { 8.0 } else { 6.0 }));
            let panel = egui::TopBottomPanel::bottom("transport")
                .frame(frame)
                .show(ctx, |ui| {
                    let (target, held) =
                        chrome::seek_bar(ui, current, total, fps, &markers, true, touch);
                    if held && self.scrub.is_none() {
                        self.scrub = Some(playing);
                        player.set_playing(false);
                        actions |= 1;
                    }
                    if let Some(target) = target {
                        seek(player, target);
                        actions |= 1;
                    }
                    if !held {
                        if let Some(was_playing) = self.scrub.take() {
                            player.set_playing(was_playing);
                            actions |= 1;
                        }
                    }
                    ui.horizontal(|ui| {
                        let size = if touch { 42.0 } else { 28.0 };
                        ui.set_min_height(size + 10.0);
                        let narrow = ui.available_width() < 520.0;
                        ui.spacing_mut().item_spacing.x = if touch { 8.0 } else { 4.0 };
                        if chrome::icon_button(ui, chrome::Icon::Restart, size, false, true)
                            .on_hover_text("Restart from the beginning (Ctrl+R)")
                            .clicked()
                        {
                            actions |= 4;
                        }
                        if chrome::icon_button(ui, chrome::Icon::StepBack, size, false, true)
                            .on_hover_text("One frame back (Left)")
                            .clicked()
                        {
                            seek(player, current.saturating_sub(1));
                            actions |= 1;
                        }
                        if chrome::play_button(ui, playing, size + 10.0, true)
                            .on_hover_text(if playing {
                                "Pause (Space)"
                            } else {
                                "Play (Space)"
                            })
                            .clicked()
                        {
                            player.toggle_play_pause();
                            actions |= 1;
                        }
                        if chrome::icon_button(ui, chrome::Icon::StepForward, size, false, true)
                            .on_hover_text("One frame forward (Right)")
                            .clicked()
                        {
                            seek(player, (current + 1).min(total - 1));
                            actions |= 1;
                        }
                        ui.add_space(if touch { 6.0 } else { 10.0 });
                        ui.label(
                            egui::RichText::new(chrome::format_time(current, fps))
                                .monospace()
                                .strong(),
                        );
                        if !narrow {
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "/ {}",
                                    chrome::format_time(total, fps)
                                ))
                                .monospace()
                                .weak(),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if chrome::icon_button(ui, chrome::Icon::Fullscreen, size, false, true)
                                .on_hover_text("Fullscreen (F9)")
                                .clicked()
                            {
                                actions |= 2;
                            }
                            if chrome::icon_button(
                                ui,
                                chrome::Icon::Loop,
                                size,
                                player.is_loop_enabled(),
                                true,
                            )
                            .on_hover_text("Loop playback")
                            .clicked()
                            {
                                player.set_loop_enabled(!player.is_loop_enabled());
                                actions |= 1;
                            }
                            if !touch && !narrow {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "frame {} / {total}  ·  {fps} fps",
                                        current + 1
                                    ))
                                    .weak()
                                    .small(),
                                );
                            }
                        });
                    });
                });
            self.height = panel.response.rect.height().ceil();
        });
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        self.painter.clear(
            [width, height],
            self.ctx
                .style()
                .visuals
                .panel_fill
                .to_array()
                .map(|channel| f32::from(channel) / 255.0),
        );
        self.painter.paint_and_update_textures(
            [width, height],
            output.pixels_per_point,
            &primitives,
            &output.textures_delta,
        );
        actions
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.painter.destroy();
    }
}

fn seek(player: &mut Player, frame: usize) {
    let playing = player.is_playing();
    player.seek_to_frame(frame);
    player.set_playing(playing);
}
