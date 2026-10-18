//! `PlayerApp` — eframe entry point. Owns the optional `Player`, the
//! frame buffer + GPU texture, the `Settings` (theme + recents), the
//! optional Settings dialog, and the dropped-file watcher.
//!
//! UI layout:
//!   ┌─────────────────────────────────────────────────────────┐
//!   │ menubar  (File / View / Tools / Help)                    │
//!   ├──────────┬──────────────────────────────┬───────────────┤
//!   │ recent   │ playback viewport             │ file info    │
//!   │ files    │   (.q0s frame bitmap)         │   (file info)│
//!   │ panel    │                               │              │
//!   ├──────────┴──────────────────────────────┴───────────────┤
//!   │ transport bar (play / pause / frame slider / fps / loop)│
//!   └─────────────────────────────────────────────────────────┘

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::App;
use egui::{Color32, ColorImage, Context, Key, Modifiers, TextureHandle, TextureOptions, Vec2};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use super::analytics::FileStats;
use super::credits;
use super::platform::PlayerEvent;
use super::recent;
use super::render::TextureCache;
use super::settings::Settings;
use super::theme::{ColorRgb, Theme};
use crate::{Player, PlayerFormat};

#[derive(Debug, Clone, Copy, Default)]
pub struct PreviewOptions {
    pub enabled: bool,
    pub all_scenes: bool,
    pub delete_after_load: bool,
}

pub struct PlayerApp {
    preview: PreviewOptions,
    settings: Settings,
    player: Option<Player>,
    file_path: Option<PathBuf>,
    stats: Option<FileStats>,
    /// Reusable RGBA frame buffer — only used by the v1 (bitmap-only)
    /// playback path; v2 movies render straight through the painter and
    /// never touch this.
    frame_buf: Vec<u8>,
    /// v1 texture handle — same caveat. None for v2 playback.
    frame_tex: Option<TextureHandle>,
    /// Per-bitmap GPU texture cache for v2 playback. Populated on first
    /// reference, dropped on file close / reload.
    v2_textures: TextureCache,
    last_tick: Instant,
    /// Status text shown bottom-left — "loaded …", "saved theme", etc.
    status: String,
    /// Shared Credits / Settings dialog open/closed.
    show_settings: bool,
    /// Credits is deliberately the left tab; Settings remains the right tab.
    dialog_tab: DialogTab,
    /// Restart the entrance cascade every time Credits is opened.
    credits_opened_at: Instant,
    /// One-shot file path passed via the binary's CLI args (file association
    /// double-click). Loaded on the first `update()` so we have a valid ctx.
    pending_open: Option<PathBuf>,
    timeline_audio: Option<TimelineAudioPlayback>,
    observed_scene_generation: u64,
    last_pointer_stage: Option<(f32, f32, bool)>,
    /// Touch screens: controls and system bars hidden, the movie alone.
    immersive: bool,
    /// Touch screens: whether the display was last asked to stay awake.
    screen_kept_on: bool,
    /// Whether a menu bar menu was open last frame.
    menu_open: bool,
    /// Back asked for an Escape press on the next frame.
    press_escape: bool,
    touch_pad: TouchPad,
    /// Where the movie was drawn last frame.
    last_canvas: egui::Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerShortcut {
    Open,
    TogglePlay,
    PreviousFrame,
    NextFrame,
    FirstFrame,
    LastFrame,
    ToggleFullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogTab {
    Credits,
    Settings,
}

const DIALOG_TABS: [DialogTab; 2] = [DialogTab::Credits, DialogTab::Settings];

#[derive(Clone)]
struct PlayerAudioSource {
    timeline: crate::PlayerAudioTimeline,
    sample_frame: u64,
    channel: u16,
}

impl PlayerAudioSource {
    fn new(timeline: crate::PlayerAudioTimeline) -> Self {
        let sample_frame = timeline.start_sample_frame;
        Self {
            timeline,
            sample_frame,
            channel: 0,
        }
    }
}

impl Iterator for PlayerAudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.sample_frame >= self.timeline.total_sample_frames {
            return None;
        }
        let sample = crate::sample_audio_timeline(&self.timeline, self.sample_frame, self.channel);
        self.channel += 1;
        if self.channel >= crate::PLAYER_AUDIO_CHANNELS {
            self.channel = 0;
            self.sample_frame += 1;
        }
        Some(sample)
    }
}

impl Source for PlayerAudioSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        crate::PLAYER_AUDIO_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        crate::PLAYER_AUDIO_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        let remaining = self
            .timeline
            .total_sample_frames
            .saturating_sub(self.sample_frame);
        Some(Duration::from_secs_f64(
            remaining as f64 / f64::from(crate::PLAYER_AUDIO_SAMPLE_RATE),
        ))
    }
}

struct TimelineAudioPlayback {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
}

impl TimelineAudioPlayback {
    fn new() -> Result<Self, String> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|error| format!("open default audio output: {error}"))?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
        })
    }

    fn sync(&mut self, player: &Player) -> Result<(), String> {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        let Some(audio) = player.audio_timeline() else {
            return Ok(());
        };
        let sink = Sink::try_new(&self.handle)
            .map_err(|error| format!("create timeline audio sink: {error}"))?;
        if audio.start_sample_frame < audio.total_sample_frames {
            sink.append(PlayerAudioSource::new(audio.clone()));
        }
        if player.is_loop_enabled() && audio.total_sample_frames > 0 {
            let mut loop_audio = audio;
            loop_audio.start_sample_frame = 0;
            sink.append(PlayerAudioSource::new(loop_audio).repeat_infinite());
        }
        if !player.is_playing() {
            sink.pause();
        }
        self.sink = Some(sink);
        Ok(())
    }

    fn set_playing(&self, playing: bool) {
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if playing {
            sink.play();
        } else {
            sink.pause();
        }
    }
}

impl PlayerApp {
    pub fn new(initial_file: Option<PathBuf>, ctx: &Context) -> Self {
        let mut settings = Settings::load();
        settings.prune_recent();
        settings.theme.apply(ctx);
        settings.apply_text_size(ctx);
        Self {
            settings,
            preview: PreviewOptions::default(),
            player: None,
            file_path: None,
            stats: None,
            frame_buf: Vec::new(),
            frame_tex: None,
            v2_textures: TextureCache::default(),
            last_tick: Instant::now(),
            status: "ready".to_string(),
            show_settings: false,
            dialog_tab: DialogTab::Settings,
            credits_opened_at: Instant::now(),
            pending_open: initial_file,
            timeline_audio: None,
            observed_scene_generation: 0,
            last_pointer_stage: None,
            immersive: false,
            screen_kept_on: false,
            menu_open: false,
            press_escape: false,
            touch_pad: TouchPad::default(),
            last_canvas: egui::Rect::NOTHING,
        }
    }

    pub fn with_preview(mut self, options: PreviewOptions) -> Self {
        self.preview = options;
        self
    }

    fn sync_timeline_audio(&mut self) -> Result<(), String> {
        let has_audio = self.player.as_ref().is_some_and(Player::has_audio_timeline);
        if !has_audio {
            self.timeline_audio = None;
            return Ok(());
        }
        if self.timeline_audio.is_none() {
            self.timeline_audio = Some(TimelineAudioPlayback::new()?);
        }
        let player = self.player.as_ref().expect("audio player still loaded");
        self.timeline_audio
            .as_mut()
            .expect("audio output initialized")
            .sync(player)
    }

    fn set_timeline_audio_playing(&self, playing: bool) {
        if let Some(audio) = self.timeline_audio.as_ref() {
            audio.set_playing(playing);
        }
    }

    fn open_path(&mut self, ctx: &Context, path: &Path) {
        match Player::from_file(path) {
            Ok(mut player) => {
                player.set_loop_enabled(self.settings.loop_playback);
                if self.preview.enabled {
                    player.set_loop_enabled(true);
                    if self.preview.all_scenes {
                        player.enable_document_scene_sequence();
                    }
                }
                let script_warning = player.initial_script_warning();
                let bytes_len = std::fs::metadata(path)
                    .map(|m| m.len() as usize)
                    .unwrap_or(0);
                // Build file info from whichever variant the player loaded.
                let stats = if let Some(project) = player.project_v2() {
                    Some(FileStats::from_project(project, bytes_len))
                } else if let Some(spec) = player.q0v_spec() {
                    Some(FileStats::from_q0v(spec, bytes_len))
                } else {
                    player
                        .movie_v1()
                        .map(|movie| FileStats::from_movie(movie, bytes_len))
                };
                // Match viewport size to the file's natural stage dim if
                // we got a hint — keeps zoom honest after every Open.
                if let Some((w, h)) = stats.as_ref().and_then(|s| s.viewport_hint()) {
                    self.settings.viewport_w = w.clamp(16, 4096);
                    self.settings.viewport_h = h.clamp(16, 4096);
                }
                self.stats = stats;
                let scene_generation = player.scene_generation();
                self.player = Some(player);
                self.observed_scene_generation = scene_generation;
                self.last_pointer_stage = None;
                let audio_warning = self.sync_timeline_audio().err();
                self.file_path = Some(path.to_path_buf());
                self.frame_tex = None;
                self.v2_textures.invalidate();
                self.last_tick = Instant::now();
                if !self.preview.enabled {
                    self.settings.push_recent(path);
                    self.settings.save();
                }
                self.status = match (script_warning, audio_warning) {
                    (Some(script), Some(audio)) => {
                        format!(
                            "loaded {}; {script}; audio warning: {audio}",
                            path.display()
                        )
                    }
                    (Some(script), None) => format!("loaded {}; {script}", path.display()),
                    (None, Some(audio)) => {
                        format!("loaded {}; audio warning: {audio}", path.display())
                    }
                    (None, None) => format!("loaded {}", path.display()),
                };
                let title = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("q0player");
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{title} - q0player")));
                if self.preview.enabled {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                        "q0e — fast preview".into(),
                    ));
                    if let Some((w, h)) =
                        self.stats.as_ref().and_then(|stats| stats.viewport_hint())
                    {
                        let scale = (1000.0 / w.max(1) as f32)
                            .min(700.0 / h.max(1) as f32)
                            .min(1.0);
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                            (w as f32 * scale).max(240.0),
                            (h as f32 * scale).max(180.0),
                        )));
                    }
                    if self.preview.delete_after_load {
                        remove_preview_snapshot(path);
                    }
                }
            }
            Err(err) => {
                self.status = format!("open failed: {err}");
                if self.preview.enabled && self.preview.delete_after_load {
                    remove_preview_snapshot(path);
                }
            }
        }
    }

    fn pick_open(&mut self, ctx: &Context) {
        if let Some(path) = super::platform::pick_movie() {
            self.open_path(ctx, &path);
        }
    }

    fn load_theme(&mut self, ctx: &Context, path: &Path) {
        match Theme::read_q7s(path) {
            Ok((name, theme)) => {
                self.settings.theme = theme;
                self.settings.theme.apply(ctx);
                self.settings.apply_text_size(ctx);
                self.settings.save();
                self.status = format!("theme: {name}");
            }
            Err(e) => self.status = format!("load theme failed: {e}"),
        }
    }

    fn apply_recent(&mut self, ctx: &Context, action: recent::RecentAction) {
        match action {
            recent::RecentAction::Open(p) => self.open_path(ctx, &p),
            recent::RecentAction::Forget(p) => {
                self.settings.recent.retain(|x| x != &p);
                self.settings.save();
            }
            recent::RecentAction::ClearAll => {
                self.settings.recent.clear();
                self.settings.save();
            }
        }
    }

    /// Movies picked or sent by other apps, platform messages and Back.
    fn poll_platform(&mut self, ctx: &Context) {
        for event in super::platform::take_events(ctx) {
            match event {
                PlayerEvent::Open { path, preview } => {
                    self.preview = preview.unwrap_or_default();
                    self.open_path(ctx, &path);
                }
                PlayerEvent::Theme(path) => self.load_theme(ctx, &path),
                PlayerEvent::Status(message) => self.status = message,
                PlayerEvent::Back => self.back(ctx),
            }
        }
    }

    /// The system Back button: closes what is open, and with nothing left
    /// sends the player to the background, back to q0editor after a preview.
    fn back(&mut self, ctx: &Context) {
        if self.show_settings {
            self.show_settings = false;
        } else if self.immersive {
            self.toggle_fullscreen(ctx);
        } else if self.menu_open || ctx.memory(|memory| memory.any_popup_open()) {
            self.press_escape = true;
        } else {
            super::platform::leave();
        }
        ctx.request_repaint();
    }

    fn close_file(&mut self, ctx: &Context) {
        self.player = None;
        self.timeline_audio = None;
        self.observed_scene_generation = 0;
        self.last_pointer_stage = None;
        self.file_path = None;
        self.stats = None;
        self.frame_tex = None;
        self.v2_textures.invalidate();
        self.status = "closed".to_string();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("q0player".to_string()));
    }

    fn sync_scene_resources_if_needed(&mut self) {
        let Some(player) = self.player.as_ref() else {
            self.observed_scene_generation = 0;
            return;
        };
        let generation = player.scene_generation();
        if generation == self.observed_scene_generation {
            return;
        }
        self.observed_scene_generation = generation;
        let alias = player.active_scene_alias().map(str::to_owned);

        // Asset ids are scoped to one ProjectV2. Keeping texture handles from
        // the previous scene would let an identically-numbered asset in the
        // new scene display old pixels. Audio is scene-local for the same
        // reason, so both resources are invalidated at the scene boundary.
        self.frame_tex = None;
        self.v2_textures.invalidate();
        match self.sync_timeline_audio() {
            Ok(()) => {
                if let Some(alias) = alias {
                    self.status = format!("scene: {alias}");
                }
            }
            Err(error) => {
                self.status = match alias {
                    Some(alias) => format!("scene: {alias}; audio warning: {error}"),
                    None => format!("scene audio warning: {error}"),
                };
            }
        }
    }

    fn maybe_advance(&mut self) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.25);
        self.last_tick = now;
        player.tick(dt);
    }

    fn toggle_playback(&mut self) {
        let playing = {
            let Some(player) = self.player.as_mut() else {
                return;
            };
            player.toggle_play_pause();
            player.is_playing()
        };
        self.set_timeline_audio_playing(playing);
        self.last_tick = Instant::now();
        self.status = if playing {
            "playing".to_string()
        } else {
            "paused".to_string()
        };
    }

    fn seek_shortcut(&mut self, shortcut: PlayerShortcut) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let current = player.current_frame();
        let total = player.total_frames();
        let Some(target) = shortcut_frame_target(shortcut, current, total) else {
            return;
        };
        seek_to(player, target);
        if let Err(error) = self.sync_timeline_audio() {
            self.status = format!("audio seek failed: {error}");
        }
        self.last_tick = Instant::now();
        self.status = format!("frame {} of {}", target + 1, total.max(1));
    }

    fn toggle_fullscreen(&mut self, ctx: &Context) {
        if super::platform::TOUCH {
            self.immersive = !self.immersive;
            super::platform::immersive(self.immersive);
            self.status = if self.immersive {
                "fullscreen; Back returns".to_string()
            } else {
                "windowed mode".to_string()
            };
            return;
        }
        let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        self.status = if fullscreen {
            "windowed mode".to_string()
        } else {
            "fullscreen".to_string()
        };
    }

    fn handle_shortcuts(&mut self, ctx: &Context, game_input_active: bool) {
        if self.preview.enabled
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        for shortcut in take_shortcuts(ctx, game_input_active) {
            if self.preview.enabled && shortcut == PlayerShortcut::Open {
                continue;
            }
            match shortcut {
                PlayerShortcut::Open => self.pick_open(ctx),
                PlayerShortcut::TogglePlay => self.toggle_playback(),
                PlayerShortcut::PreviousFrame
                | PlayerShortcut::NextFrame
                | PlayerShortcut::FirstFrame
                | PlayerShortcut::LastFrame => self.seek_shortcut(shortcut),
                PlayerShortcut::ToggleFullscreen => self.toggle_fullscreen(ctx),
            }
        }
    }

    /// Render the current frame into our CPU buffer, then upload it as an
    /// `egui::ColorImage` into the cached texture handle. Driven by the
    /// settings' viewport size (which `viewport()` retargets each frame
    /// when `auto_resolution` is on, so the rasteriser always matches
    /// the actual on-screen pixel count and the result stays crisp at
    /// any window size).
    fn refresh_texture(&mut self, ctx: &Context) {
        let Some(player) = self.player.as_ref() else {
            return;
        };
        let image = if player.format() == PlayerFormat::Q0v {
            let Some((spec, rgba)) = player.q0v_current_frame_rgba() else {
                return;
            };
            ColorImage::from_rgba_unmultiplied(
                [spec.width.max(1) as usize, spec.height.max(1) as usize],
                &rgba,
            )
        } else {
            let w = self.settings.viewport_w.max(1);
            let h = self.settings.viewport_h.max(1);
            let needed = (w as usize).saturating_mul(h as usize).saturating_mul(4);
            if self.frame_buf.len() != needed {
                self.frame_buf = vec![0; needed];
            } else {
                self.frame_buf.fill(0);
            }
            let ss = self.settings.supersample.clamp(1, 4);
            player.render_with_quality(&mut self.frame_buf, w, h, ss);
            ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &self.frame_buf)
        };
        let opts = TextureOptions::LINEAR;
        match self.frame_tex.as_mut() {
            Some(handle) => handle.set(image, opts),
            None => {
                let handle = ctx.load_texture("q0player_frame", image, opts);
                self.frame_tex = Some(handle);
            }
        }
    }
}

fn remove_preview_snapshot(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    // Only disposable files created by the editor qualify for automatic cleanup.
    if path.file_name().is_some_and(|name| name == "preview.q0s")
        && parent
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("q0e-preview-"))
        && parent.parent() == Some(std::env::temp_dir().as_path())
    {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(parent);
    }
}

fn aspect_fit_rect(container: egui::Rect, content_aspect: f32) -> egui::Rect {
    let available = container.size().max(Vec2::splat(1.0));
    let content_aspect = content_aspect.max(1.0e-6);
    let canvas_aspect = available.x / available.y;
    let drawn = if canvas_aspect > content_aspect {
        Vec2::new(available.y * content_aspect, available.y)
    } else {
        Vec2::new(available.x, available.x / content_aspect)
    };
    egui::Rect::from_center_size(container.center(), drawn)
}

impl App for PlayerApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drag-and-drop: open the first .q0s, or explain why the drop was
        // rejected instead of leaving the user with no feedback.
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let supported = dropped
                .iter()
                .filter_map(|file| file.path.as_ref())
                .find(|path| is_player_media_path(path))
                .cloned();
            if let Some(path) = supported {
                self.open_path(ctx, &path);
            } else {
                self.status =
                    "unsupported dropped file; q0player opens .q0s and .q0v media".to_string();
            }
        }

        // CLI-arg file: load once on first update so the texture upload has
        // a real `ctx`.
        if let Some(p) = self.pending_open.take() {
            self.open_path(ctx, &p);
        }
        self.poll_platform(ctx);

        let game_input_active = self
            .player
            .as_ref()
            .is_some_and(crate::Player::uses_game_input);
        if game_input_active {
            let (mut left, mut right, mut jump) = take_game_input(ctx, self.show_settings);
            if super::platform::TOUCH && !self.show_settings {
                let (touch_left, touch_right, touch_jump) =
                    self.touch_pad.held(ctx, self.last_canvas);
                left |= touch_left;
                right |= touch_right;
                jump |= touch_jump;
            }
            if let Some(player) = self.player.as_mut() {
                player.set_input_state(left, right, jump);
            }
        }
        self.handle_shortcuts(ctx, game_input_active);

        // Keep playing as long as the player wants to.
        let was_playing = self
            .player
            .as_ref()
            .map(|p| p.is_playing())
            .unwrap_or(false);
        self.maybe_advance();
        let keep_awake = was_playing || game_input_active;
        if keep_awake != self.screen_kept_on {
            self.screen_kept_on = keep_awake;
            super::platform::keep_screen_on(keep_awake);
        }
        if was_playing || game_input_active {
            // Interactive q0.input movies own a fixed-step runtime clock even
            // while authored timeline transport is paused.
            ctx.request_repaint();
        }

        if !self.preview.enabled && !self.immersive {
            self.menubar(ctx);
            self.transport_bar(ctx);
            self.side_panels(ctx);
        } else {
            self.menu_open = false;
        }
        self.sync_scene_resources_if_needed();
        // viewport() retargets the rasteriser size to actual canvas
        // pixels (when auto_resolution is on) and refreshes the texture
        // — done last so the central area's pixel size is already known.
        self.viewport(ctx);

        if self.preview.enabled && self.player.is_none() {
            egui::Window::new("Fast preview could not open")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(&self.status);
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
        }

        if self.show_settings {
            self.settings_dialog(ctx);
        }
        if self.immersive && !self.preview.enabled {
            self.leave_fullscreen_button(ctx);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &Context, raw_input: &mut egui::RawInput) {
        if std::mem::take(&mut self.press_escape) {
            for pressed in [true, false] {
                raw_input.events.push(egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.preview.enabled {
            self.settings.save();
        }
    }
}

// -------------------- UI sections --------------------

impl PlayerApp {
    fn menubar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                let file = ui.menu_button("File", |ui| {
                    if ui.button("Open...").clicked() {
                        ui.close_menu();
                        self.pick_open(ctx);
                    }
                    ui.menu_button("Open Recent", |ui| {
                        if self.settings.recent.is_empty() {
                            ui.label(egui::RichText::new("(no recent files)").weak());
                        } else {
                            let recent = self.settings.recent.clone();
                            let labels = recent_menu_labels(&recent);
                            for (path, label) in recent.into_iter().zip(labels) {
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    ui.close_menu();
                                    self.open_path(ctx, &path);
                                }
                            }
                            ui.separator();
                            if ui.button("Clear list").clicked() {
                                ui.close_menu();
                                self.settings.recent.clear();
                                self.settings.save();
                            }
                        }
                    });
                    if ui
                        .add_enabled(self.player.is_some(), egui::Button::new("Close"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.close_file(ctx);
                    }
                    // Android apps are left with Back or Home, not closed.
                    if !super::platform::TOUCH {
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            ui.close_menu();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
                let view = ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.settings.show_left_panel, "Recent panel");
                    ui.checkbox(&mut self.settings.show_right_panel, "File info panel");
                    ui.separator();
                    let fullscreen =
                        ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
                    let label = if fullscreen {
                        "Exit Fullscreen  F11"
                    } else {
                        "Fullscreen  F11"
                    };
                    if ui.button(label).clicked() {
                        ui.close_menu();
                        self.toggle_fullscreen(ctx);
                    }
                });
                let tools = ui.menu_button("Tools", |ui| {
                    if ui.button("Credits...").clicked() {
                        ui.close_menu();
                        self.dialog_tab = DialogTab::Credits;
                        self.credits_opened_at = Instant::now();
                        self.show_settings = true;
                    }
                    if ui.button("Settings...").clicked() {
                        ui.close_menu();
                        self.dialog_tab = DialogTab::Settings;
                        self.show_settings = true;
                    }
                    #[cfg(windows)]
                    {
                        use super::assoc::AssociationKind;
                        ui.separator();
                        for kind in [AssociationKind::Q0s, AssociationKind::Q0v] {
                            let extension = kind.extension();
                            if ui
                                .button(format!("Associate {extension} with this player"))
                                .clicked()
                            {
                                ui.close_menu();
                                match super::assoc::register_for_current_user(kind) {
                                    Ok(()) => {
                                        self.status =
                                            format!("{extension} files now open with q0player")
                                    }
                                    Err(error) => {
                                        self.status = format!("association failed: {error}")
                                    }
                                }
                            }
                            if ui
                                .button(format!("Remove {extension} association"))
                                .clicked()
                            {
                                ui.close_menu();
                                match super::assoc::unregister_for_current_user(kind) {
                                    Ok(()) => {
                                        self.status = format!("{extension} association removed")
                                    }
                                    Err(error) => self.status = format!("removal failed: {error}"),
                                }
                            }
                        }
                    }
                });
                let help = ui.menu_button("Help", |ui| {
                    ui.label(egui::RichText::new("q0player").strong());
                    ui.label(egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).weak());
                    ui.separator();
                    ui.label("Plays .q0s movies and .q0v embedded media.");
                    ui.label("Drop a file on the window or use File > Open...");
                    ui.separator();
                    ui.label("Ctrl+O: open | Space: play/pause | F11: fullscreen");
                    ui.label("Left/Right: one frame | Home/End: first/last frame");
                });
                self.menu_open = [file.inner, view.inner, tools.inner, help.inner]
                    .iter()
                    .any(Option::is_some);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let full_status = self.status.clone();
                    let mut text = egui::RichText::new(truncate_label(&full_status, 72)).small();
                    text = if status_needs_attention(&full_status) {
                        text.color(self.settings.theme.accent.to_color32()).strong()
                    } else {
                        text.weak()
                    };
                    let response = ui.label(text).on_hover_text(full_status.as_str());
                    response.context_menu(|ui| {
                        if ui.button("Copy status").clicked() {
                            ui.output_mut(|output| output.copied_text = full_status.clone());
                            ui.close_menu();
                        }
                    });
                });
            });
        });
    }

    fn transport_bar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if super::platform::TOUCH {
                    ui.spacing_mut().interact_size.y = 40.0;
                    ui.spacing_mut().button_padding = egui::vec2(14.0, 8.0);
                }
                let has_player = self.player.is_some();
                let interactive_single_frame = self
                    .player
                    .as_ref()
                    .is_some_and(|player| player.uses_game_input() && player.total_frames() == 1);
                let playing = self
                    .player
                    .as_ref()
                    .map(|p| p.is_playing())
                    .unwrap_or(false);
                let label = if interactive_single_frame {
                    "Game running"
                } else {
                    playback_button_label(playing)
                };
                if ui
                    .add_enabled(
                        has_player && !interactive_single_frame,
                        egui::Button::new(label),
                    )
                    .on_hover_text(if interactive_single_frame {
                        "q0.input runtime runs independently from one-frame timeline transport"
                    } else {
                        "Play or pause timeline (Space)"
                    })
                    .clicked()
                {
                    self.toggle_playback();
                }
                if ui
                    .add_enabled(has_player, egui::Button::new("Restart"))
                    .on_hover_text("Restart playback from the beginning")
                    .clicked()
                {
                    if let Some(path) = self.file_path.clone() {
                        self.open_path(ctx, &path);
                    } else {
                        self.status = "restart failed: source file is unavailable".to_string();
                    }
                }

                let total = self.player.as_ref().map(|p| p.total_frames()).unwrap_or(1);
                let mut current = self.player.as_ref().map(|p| p.current_frame()).unwrap_or(0);
                let max = total.saturating_sub(1);
                if super::platform::TOUCH {
                    // The slider takes what the fps label and Loop leave.
                    ui.spacing_mut().slider_width = (ui.available_width() - 190.0).max(80.0);
                }
                let resp = ui
                    .add_enabled(
                        has_player,
                        egui::Slider::new(&mut current, 0..=max).text("frame"),
                    )
                    .on_hover_text("Left/Right: one frame; Home/End: first/last frame");
                if resp.changed() {
                    if let Some(p) = self.player.as_mut() {
                        seek_to(p, current);
                    }
                    if let Err(error) = self.sync_timeline_audio() {
                        self.status = format!("audio seek failed: {error}");
                    }
                }

                ui.separator();
                let fps = self.player.as_ref().map(|p| p.fps()).unwrap_or(0);
                ui.label(egui::RichText::new(format!("{fps} fps")).weak());
                ui.separator();
                if ui
                    .checkbox(&mut self.settings.loop_playback, "Loop")
                    .changed()
                {
                    if let Some(p) = self.player.as_mut() {
                        p.set_loop_enabled(self.settings.loop_playback);
                    }
                    if let Err(error) = self.sync_timeline_audio() {
                        self.status = format!("audio loop update failed: {error}");
                    }
                    self.settings.save();
                }
            });
        });
    }

    fn side_panels(&mut self, ctx: &Context) {
        if self.settings.show_left_panel {
            egui::SidePanel::left("recent_panel")
                .resizable(true)
                .default_width(180.0)
                .show(ctx, |ui| {
                    let recent = self.settings.recent.clone();
                    if let Some(action) = recent::render(&recent, ui) {
                        self.apply_recent(ctx, action);
                    }
                });
        }
        if self.settings.show_right_panel {
            egui::SidePanel::right("analytics_panel")
                .resizable(true)
                .default_width(220.0)
                .show(ctx, |ui| match &self.stats {
                    Some(stats) => stats.render(ui),
                    None => {
                        ui.heading("File info");
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Open a .q0s file to see its stats here.")
                                .weak()
                                .small(),
                        );
                    }
                });
        }
    }

    fn dispatch_stage_pointer(&mut self, ctx: &Context, canvas_rect: egui::Rect) {
        let Some(player) = self.player.as_mut() else {
            self.last_pointer_stage = None;
            return;
        };
        if !player.uses_mouse_events() || player.format() != PlayerFormat::VectorV2 {
            self.last_pointer_stage = None;
            return;
        }
        let Some(project) = player.project_v2() else {
            return;
        };
        let stage_w = project.meta.stage_width.max(1) as f32;
        let stage_h = project.meta.stage_height.max(1) as f32;
        let scale = (canvas_rect.width() / stage_w)
            .min(canvas_rect.height() / stage_h)
            .max(0.001);
        let drawn = egui::vec2(stage_w * scale, stage_h * scale);
        let stage_rect = egui::Rect::from_center_size(canvas_rect.center(), drawn);
        let (pointer_pos, delta, pressed, released, clicked, double_clicked, scroll) =
            ctx.input(|input| {
                (
                    input.pointer.latest_pos(),
                    input.pointer.delta(),
                    input.pointer.primary_pressed(),
                    input.pointer.primary_released(),
                    input.pointer.primary_clicked(),
                    input
                        .pointer
                        .button_double_clicked(egui::PointerButton::Primary),
                    input.raw_scroll_delta,
                )
            });
        let (x, y, inside) = if let Some(pos) = pointer_pos {
            (
                ((pos.x - stage_rect.left()) / scale).clamp(0.0, stage_w),
                ((pos.y - stage_rect.top()) / scale).clamp(0.0, stage_h),
                stage_rect.contains(pos),
            )
        } else {
            let (x, y, _) = self.last_pointer_stage.unwrap_or((0.0, 0.0, false));
            (x, y, false)
        };
        let now = (x, y, inside);
        if self.last_pointer_stage != Some(now) || delta != egui::Vec2::ZERO {
            player.dispatch_pointer_event("mouse.move", x, y, inside, 0.0, 0.0);
        }
        if pressed && inside {
            player.dispatch_pointer_event("mouse.down", x, y, true, 0.0, 0.0);
        }
        if released {
            player.dispatch_pointer_event("mouse.up", x, y, inside, 0.0, 0.0);
        }
        if clicked && inside {
            player.dispatch_pointer_event("mouse.click", x, y, true, 0.0, 0.0);
        }
        if double_clicked && inside {
            player.dispatch_pointer_event("mouse.double", x, y, true, 0.0, 0.0);
        }
        if inside && scroll != egui::Vec2::ZERO {
            player.dispatch_pointer_event("mouse.wheel", x, y, true, scroll.x, scroll.y);
        }
        self.last_pointer_stage = Some(now);
    }

    fn viewport(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.settings.theme.viewport_bg.to_color32()))
            .show(ctx, |ui| {
                let canvas_rect = ui.max_rect();
                let painter = ui.painter_at(canvas_rect);
                self.dispatch_stage_pointer(ctx, canvas_rect);
                self.last_canvas = canvas_rect;

                let format = self.player.as_ref().map(|p| p.format());

                match format {
                    // ── v2 vector path: emit shapes straight into the
                    // canvas painter, no CPU rasterisation, no upscaled
                    // texture. egui's tesselator hits native pixels at
                    // any zoom so it stays SWF-crisp.
                    Some(PlayerFormat::VectorV2) => {
                        if let Some(player) = self.player.as_ref() {
                            if let Some(project) = player.project_v2() {
                                let q0rg_id = project.meta.entry_q0rg_id;
                                let frame = player.current_frame() as u16;
                                crate::ui::render::paint_v2_frame_with_runtime_overrides(
                                    &painter,
                                    ctx,
                                    project,
                                    q0rg_id,
                                    frame,
                                    canvas_rect,
                                    &mut self.v2_textures,
                                    self.settings.white_stage_bg,
                                    player.rig_runtime_overrides(),
                                    player.scene_runtime(),
                                );
                            }
                        }
                    }
                    // ── v1 bitmap path: keep the texture pipeline. Bitmap
                    // movies have no vector data to vectorise; texture
                    // upscaling is honest given the source IS pixels.
                    Some(PlayerFormat::BitmapV1 | PlayerFormat::Q0v) => {
                        let content_aspect = self
                            .stats
                            .as_ref()
                            .and_then(|s| s.viewport_hint())
                            .map(|(w, h)| (w as f32) / (h.max(1) as f32))
                            .unwrap_or(16.0 / 9.0);
                        let rect = aspect_fit_rect(canvas_rect, content_aspect);
                        let lw = rect.width();
                        let lh = rect.height();
                        if format == Some(PlayerFormat::BitmapV1) && self.settings.auto_resolution {
                            let pp = ctx.pixels_per_point();
                            let phys_w = ((lw * pp).round() as u32).clamp(16, 4096);
                            let phys_h = ((lh * pp).round() as u32).clamp(16, 4096);
                            if self.settings.viewport_w != phys_w
                                || self.settings.viewport_h != phys_h
                            {
                                self.settings.viewport_w = phys_w;
                                self.settings.viewport_h = phys_h;
                                self.frame_tex = None;
                            }
                        }
                        self.refresh_texture(ctx);
                        if let Some(tex) = self.frame_tex.as_ref() {
                            painter.image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                    }
                    None => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.heading("q0player");
                            if super::platform::TOUCH {
                                ui.add_space(12.0);
                                let open = egui::Button::new("Open a movie...")
                                    .min_size(egui::vec2(220.0, 44.0));
                                if ui.add(open).clicked() {
                                    self.pick_open(ctx);
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new(
                                        "Drop a .q0s or .q0v file here, or use File > Open...",
                                    )
                                    .weak(),
                                );
                            }
                        });
                        // The recent panel starts hidden on touch screens.
                        if super::platform::TOUCH && !self.settings.show_left_panel {
                            ui.add_space(24.0);
                            let recent = self.settings.recent.clone();
                            if let Some(action) = recent::render(&recent, ui) {
                                self.apply_recent(ctx, action);
                            }
                        }
                    }
                }
                let game_input = self
                    .player
                    .as_ref()
                    .is_some_and(crate::Player::uses_game_input);
                if super::platform::TOUCH && game_input && !self.show_settings {
                    self.touch_pad.paint(&painter, canvas_rect);
                }
            });
    }

    /// Touch screens have no Escape or F11 to get out of fullscreen.
    fn leave_fullscreen_button(&mut self, ctx: &Context) {
        egui::Area::new(egui::Id::new("q0player_leave_fullscreen"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let button = egui::Button::new(egui::RichText::new("Exit fullscreen").small())
                    .fill(Color32::from_black_alpha(120));
                if ui.add(button).clicked() {
                    self.toggle_fullscreen(ctx);
                }
            });
    }

    fn settings_dialog(&mut self, ctx: &Context) {
        let mut open = self.show_settings;
        let mut close_requested = false;
        let mut viewport_changed = false;
        let screen = ctx.screen_rect().size() - egui::vec2(16.0, 16.0);
        egui::Window::new("q0player")
            .open(&mut open)
            .default_size([720.0_f32.min(screen.x), 680.0_f32.min(screen.y)])
            .min_size([500.0_f32.min(screen.x), 500.0_f32.min(screen.y)])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for tab in DIALOG_TABS {
                        let selected = self.dialog_tab == tab;
                        let label = match tab {
                            DialogTab::Credits => "credits",
                            DialogTab::Settings => "settings",
                        };
                        let accent = self.settings.theme.accent.to_color32();
                        let panel = self.settings.theme.panel.to_color32();
                        let text = if selected {
                            self.settings.theme.text.to_color32()
                        } else {
                            self.settings.theme.text_dim.to_color32()
                        };
                        let button = egui::Button::new(
                            egui::RichText::new(label).size(16.0).strong().color(text),
                        )
                        .fill(if selected { accent } else { panel })
                        .stroke(egui::Stroke::new(
                            if selected { 1.5_f32 } else { 1.0_f32 },
                            if selected { accent } else { self.settings.theme.stroke_dark.to_color32() },
                        ));
                        if ui.add_sized([132.0, 36.0], button).clicked() {
                            if tab == DialogTab::Credits && self.dialog_tab != DialogTab::Credits {
                                self.credits_opened_at = Instant::now();
                            }
                            self.dialog_tab = tab;
                        }
                    }
                });
                ui.add_space(8.0);

                match self.dialog_tab {
                    DialogTab::Credits => {
                        egui::ScrollArea::vertical()
                            .id_source("credits_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                credits::show(
                                    ui,
                                    &self.settings.theme,
                                    self.credits_opened_at.elapsed().as_secs_f32(),
                                );
                            });
                    }
                    DialogTab::Settings => {
                        egui::ScrollArea::vertical()
                            .id_source("settings_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                ui.heading("Theme presets");
                ui.horizontal_wrapped(|ui| {
                    for preset in Theme::PRESETS {
                        if ui.button(preset.name).clicked() {
                            self.settings.theme = Theme::from_builtin(preset);
                            self.settings.theme.apply(ctx);
                            self.settings.apply_text_size(ctx);
                            self.settings.save();
                            self.status = format!("theme: {}", preset.name);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Save theme...")
                        .on_hover_text("Save the current colours as a reusable .q7s theme")
                        .clicked()
                    {
                        if let Some(path) = super::platform::theme_save_path() {
                            let name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Custom")
                                .to_string();
                            match self.settings.theme.write_q7s(&path, &name) {
                                Ok(()) => {
                                    super::platform::file_written(&path);
                                    self.status = format!("saved theme: {}", path.display())
                                }
                                Err(e) => self.status = format!("save theme failed: {e}"),
                            }
                        }
                    }
                    if ui
                        .button("Load theme...")
                        .on_hover_text("Apply a previously saved .q7s theme")
                        .clicked()
                    {
                        if let Some(path) = super::platform::pick_theme() {
                            self.load_theme(ctx, &path);
                        }
                    }
                });
                ui.separator();

                ui.heading("Custom colours");
                let mut t = self.settings.theme.clone();
                egui::Grid::new("theme_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        color_row(ui, "Accent", &mut t.accent);
                        color_row(ui, "Panel", &mut t.panel);
                        color_row(ui, "Window", &mut t.window);
                        color_row(ui, "Inputs background", &mut t.deep_bg);
                        color_row(ui, "Text", &mut t.text);
                        color_row(ui, "Text dim", &mut t.text_dim);
                        color_row(ui, "Window stroke", &mut t.stroke_dark);
                        color_row(ui, "Viewport background", &mut t.viewport_bg);
                    });
                if t != self.settings.theme {
                    self.settings.theme = t;
                    self.settings.theme.apply(ctx);
                    self.settings.apply_text_size(ctx);
                    self.settings.save();
                }
                ui.separator();

                ui.heading("Interface");
                let mut text_size = self.settings.text_size_points.clamp(11, 24);
                if ui
                    .add(egui::Slider::new(&mut text_size, 11..=24).text("Text size"))
                    .on_hover_text("Changes the size of menus, panels, buttons and labels")
                    .changed()
                {
                    self.settings.text_size_points = text_size;
                    self.settings.apply_text_size(ctx);
                    self.settings.save();
                    self.status = format!("text size: {} pt", self.settings.text_size_points);
                }
                ui.separator();

                ui.heading("Stage");
                ui.checkbox(
                    &mut self.settings.white_stage_bg,
                    "White paper under transparent areas",
                )
                .on_hover_text(
                    "Paints the stage white before drawing the movie. Without this, \
                     transparent / unfilled regions blend onto the player's panel \
                     colour and may become invisible on dark themes.",
                );
                ui.separator();

                ui.heading("Bitmap (legacy v1) playback");
                ui.label(
                    egui::RichText::new(
                        "Vector .q0s v2 files render straight to GPU and stay crisp at any \
                         window size; no settings are needed. These options only affect old \
                         bitmap-only v1 files.",
                    )
                    .small()
                    .weak(),
                );
                ui.checkbox(
                    &mut self.settings.auto_resolution,
                    "Match canvas pixels (v1)",
                )
                .on_hover_text(
                    "For v1 files: resample the source bitmap to the canvas's actual pixel \
                     size every frame. Turn off to render at a fixed `viewport_w/h`.",
                );
                ui.add_enabled_ui(!self.settings.auto_resolution, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Width");
                        let r = ui.add(
                            egui::DragValue::new(&mut self.settings.viewport_w)
                                .clamp_range(16..=4096),
                        );
                        if r.changed() {
                            viewport_changed = true;
                        }
                        ui.label("Height");
                        let r = ui.add(
                            egui::DragValue::new(&mut self.settings.viewport_h)
                                .clamp_range(16..=4096),
                        );
                        if r.changed() {
                            viewport_changed = true;
                        }
                    });
                });
                ui.separator();

                #[cfg(windows)]
                {
                    use super::assoc::AssociationKind;
                    ui.heading("File associations");
                    ui.label(
                        egui::RichText::new(
                            "Choose which q0player media types open here when double-clicked. \
                             This affects the current Windows user only; no admin access is needed.",
                        )
                        .small()
                        .weak(),
                    );
                    for kind in [AssociationKind::Q0s, AssociationKind::Q0v] {
                        let extension = kind.extension();
                        let state = match super::assoc::is_registered_for_current_user(kind) {
                            Ok(true) => "active for this q0player",
                            Ok(false) => "not active",
                            Err(_) => "status unavailable",
                        };
                        ui.horizontal(|ui| {
                            ui.label(format!("{extension}: {state}"));
                            if ui.button("Register").clicked() {
                                match super::assoc::register_for_current_user(kind) {
                                    Ok(()) => {
                                        self.status =
                                            format!("{extension} files now open with q0player")
                                    }
                                    Err(error) => {
                                        self.status = format!("association failed: {error}")
                                    }
                                }
                            }
                            if ui.button("Remove").clicked() {
                                match super::assoc::unregister_for_current_user(kind) {
                                    Ok(()) => {
                                        self.status = format!("{extension} association removed")
                                    }
                                    Err(error) => self.status = format!("removal failed: {error}"),
                                }
                            }
                        });
                    }
                    ui.separator();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close_requested = true;
                    }
                });

                            });
                    }
                }
            });
        if close_requested {
            open = false;
        }
        self.show_settings = open;
        if viewport_changed {
            self.frame_tex = None;
            self.settings.save();
        }
    }
}

fn take_shortcuts(ctx: &Context, game_input_active: bool) -> Vec<PlayerShortcut> {
    if ctx.wants_keyboard_input() {
        return Vec::new();
    }

    ctx.input_mut(|input| {
        let mut shortcuts = Vec::new();
        if input.consume_key(Modifiers::COMMAND, Key::O) {
            shortcuts.push(PlayerShortcut::Open);
        }
        if !game_input_active && input.consume_key(Modifiers::NONE, Key::Space) {
            shortcuts.push(PlayerShortcut::TogglePlay);
        }
        if !game_input_active && input.consume_key(Modifiers::NONE, Key::ArrowLeft) {
            shortcuts.push(PlayerShortcut::PreviousFrame);
        }
        if !game_input_active && input.consume_key(Modifiers::NONE, Key::ArrowRight) {
            shortcuts.push(PlayerShortcut::NextFrame);
        }
        if input.consume_key(Modifiers::NONE, Key::Home) {
            shortcuts.push(PlayerShortcut::FirstFrame);
        }
        if input.consume_key(Modifiers::NONE, Key::End) {
            shortcuts.push(PlayerShortcut::LastFrame);
        }
        if input.consume_key(Modifiers::NONE, Key::F11) {
            shortcuts.push(PlayerShortcut::ToggleFullscreen);
        }
        shortcuts
    })
}

/// On-screen buttons for movies that read q0.input, on screens without keys.
/// Every finger counts, not just the one egui follows as the pointer, so
/// running and jumping can be held at once.
#[derive(Default)]
struct TouchPad {
    /// Where each finger down is, by touch id.
    fingers: std::collections::HashMap<u64, egui::Pos2>,
}

impl TouchPad {
    const SIZE: f32 = 76.0;
    const MARGIN: f32 = 18.0;

    /// Left, right and jump, from the bottom corners of `canvas`.
    fn buttons(canvas: egui::Rect) -> [egui::Rect; 3] {
        let size = egui::vec2(Self::SIZE, Self::SIZE);
        let top = canvas.bottom() - Self::MARGIN - Self::SIZE;
        let left = egui::Rect::from_min_size(egui::pos2(canvas.left() + Self::MARGIN, top), size);
        let right = left.translate(egui::vec2(Self::SIZE + 12.0, 0.0));
        let jump = egui::Rect::from_min_size(
            egui::pos2(canvas.right() - Self::MARGIN - Self::SIZE, top),
            size,
        );
        [left, right, jump]
    }

    fn held(&mut self, ctx: &Context, canvas: egui::Rect) -> (bool, bool, bool) {
        ctx.input(|input| {
            for event in &input.events {
                if let egui::Event::Touch { id, phase, pos, .. } = event {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.fingers.insert(id.0, *pos);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.fingers.remove(&id.0);
                        }
                    }
                }
            }
        });
        let [left, right, jump] =
            Self::buttons(canvas).map(|rect| self.fingers.values().any(|pos| rect.contains(*pos)));
        (left, right, jump)
    }

    fn paint(&self, painter: &egui::Painter, canvas: egui::Rect) {
        let [left, right, jump] = Self::buttons(canvas);
        for (rect, direction) in [
            (left, egui::vec2(-1.0, 0.0)),
            (right, egui::vec2(1.0, 0.0)),
            (jump, egui::vec2(0.0, -1.0)),
        ] {
            let down = self.fingers.values().any(|pos| rect.contains(*pos));
            painter.rect_filled(
                rect,
                14.0,
                Color32::from_white_alpha(if down { 90 } else { 36 }),
            );
            // A triangle pointing in `direction`.
            let center = rect.center();
            let side = egui::vec2(-direction.y, direction.x);
            let tip = center + direction * 16.0;
            let back = center - direction * 12.0;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, back + side * 16.0, back - side * 16.0],
                Color32::from_white_alpha(200),
                egui::Stroke::NONE,
            ));
        }
    }
}

fn take_game_input(ctx: &Context, blocked_by_dialog: bool) -> (bool, bool, bool) {
    if blocked_by_dialog {
        return (false, false, false);
    }
    ctx.input(|input| {
        let left = input.key_down(Key::A) || input.key_down(Key::ArrowLeft);
        let right = input.key_down(Key::D) || input.key_down(Key::ArrowRight);
        let jump =
            input.key_down(Key::W) || input.key_down(Key::ArrowUp) || input.key_down(Key::Space);
        (left, right, jump)
    })
}

fn shortcut_frame_target(shortcut: PlayerShortcut, current: usize, total: usize) -> Option<usize> {
    let last = total.saturating_sub(1);
    match shortcut {
        PlayerShortcut::PreviousFrame => Some(current.saturating_sub(1)),
        PlayerShortcut::NextFrame => Some(current.saturating_add(1).min(last)),
        PlayerShortcut::FirstFrame => Some(0),
        PlayerShortcut::LastFrame => Some(last),
        PlayerShortcut::Open | PlayerShortcut::TogglePlay | PlayerShortcut::ToggleFullscreen => {
            None
        }
    }
}

fn playback_button_label(playing: bool) -> &'static str {
    if playing {
        "Pause"
    } else {
        "Play"
    }
}

fn is_player_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("q0s") || extension.eq_ignore_ascii_case("q0v")
        })
}

fn recent_menu_labels(recent: &[PathBuf]) -> Vec<String> {
    let names: Vec<String> = recent
        .iter()
        .map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        })
        .collect();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for name in &names {
        *counts.entry(name.to_lowercase()).or_default() += 1;
    }

    recent
        .iter()
        .zip(names)
        .map(|(path, name)| {
            let duplicate = counts.get(&name.to_lowercase()).copied().unwrap_or(0) > 1;
            let label = if duplicate {
                let parent = path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|part| part.to_string_lossy().into_owned())
                    .filter(|part| !part.is_empty())
                    .unwrap_or_else(|| "root".to_string());
                format!("{name} ({parent})")
            } else {
                name
            };
            truncate_label(&label, 64)
        })
        .collect()
}

fn color_row(ui: &mut egui::Ui, label: &str, target: &mut ColorRgb) {
    ui.label(label);
    let mut c = [target.r, target.g, target.b];
    if ui.color_edit_button_srgb(&mut c).changed() {
        target.r = c[0];
        target.g = c[1];
        target.b = c[2];
    }
    ui.end_row();
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("...");
    shortened
}

fn status_needs_attention(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    [
        "failed",
        "error",
        "unsupported",
        "too large",
        "warning",
        "diagnostic",
    ]
    .iter()
    .any(|marker| status.contains(marker))
}

/// Seek the player to `target_frame` while preserving play/pause state.
fn seek_to(player: &mut Player, target_frame: usize) {
    let was_playing = player.is_playing();
    player.seek_to_frame(target_frame);
    player.set_playing(was_playing);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_with_key(key: Key, modifiers: Modifiers) -> Context {
        let ctx = Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        ctx.begin_frame(input);
        ctx
    }

    #[test]
    fn q0v_aspect_fit_is_centered_and_never_crops_the_source() {
        let container = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 500.0));
        let square = aspect_fit_rect(container, 1.0);
        assert_eq!(square.size(), Vec2::new(500.0, 500.0));
        assert_eq!(square.center(), container.center());
        assert!(container.contains(square.min));
        assert!(container.contains(square.max));

        let portrait = aspect_fit_rect(container, 9.0 / 16.0);
        assert_eq!(portrait.height(), 500.0);
        assert!(portrait.width() < container.width());
        assert_eq!(portrait.center(), container.center());
    }

    #[test]
    fn credits_tab_is_immediately_left_of_settings() {
        assert_eq!(DIALOG_TABS, [DialogTab::Credits, DialogTab::Settings]);
    }

    #[test]
    fn shortcut_keys_map_to_player_actions() {
        let ctx = context_with_key(Key::Space, Modifiers::NONE);
        assert_eq!(
            take_shortcuts(&ctx, false),
            vec![PlayerShortcut::TogglePlay]
        );
        let _ = ctx.end_frame();

        let ctx = context_with_key(Key::O, Modifiers::COMMAND);
        assert_eq!(take_shortcuts(&ctx, false), vec![PlayerShortcut::Open]);
        let _ = ctx.end_frame();
    }

    #[test]
    fn game_input_is_blocked_when_player_dialog_is_open() {
        let ctx = context_with_key(Key::D, Modifiers::NONE);
        assert_eq!(take_game_input(&ctx, false), (false, true, false));
        assert_eq!(take_game_input(&ctx, true), (false, false, false));
        let _ = ctx.end_frame();
    }

    #[test]
    fn game_input_reserves_space_and_arrows_from_transport() {
        let ctx = context_with_key(Key::Space, Modifiers::NONE);
        assert!(take_shortcuts(&ctx, true).is_empty());
        assert_eq!(take_game_input(&ctx, false), (false, false, true));
        let _ = ctx.end_frame();

        let ctx = context_with_key(Key::ArrowRight, Modifiers::NONE);
        assert!(take_shortcuts(&ctx, true).is_empty());
        assert_eq!(take_game_input(&ctx, false), (false, true, false));
        let _ = ctx.end_frame();
    }

    #[test]
    fn shortcuts_are_suppressed_while_a_widget_has_keyboard_focus() {
        let ctx = context_with_key(Key::Space, Modifiers::NONE);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("typing")));
        assert!(ctx.wants_keyboard_input());
        assert!(take_shortcuts(&ctx, false).is_empty());
        let _ = ctx.end_frame();
    }

    #[test]
    fn playback_button_uses_pause_for_active_playback() {
        assert_eq!(playback_button_label(false), "Play");
        assert_eq!(playback_button_label(true), "Pause");
    }

    #[test]
    fn shortcut_frame_targets_are_bounded() {
        assert_eq!(
            shortcut_frame_target(PlayerShortcut::PreviousFrame, 0, 12),
            Some(0)
        );
        assert_eq!(
            shortcut_frame_target(PlayerShortcut::NextFrame, 11, 12),
            Some(11)
        );
        assert_eq!(
            shortcut_frame_target(PlayerShortcut::FirstFrame, 7, 12),
            Some(0)
        );
        assert_eq!(
            shortcut_frame_target(PlayerShortcut::LastFrame, 7, 12),
            Some(11)
        );
    }

    #[test]
    fn media_drop_check_is_case_insensitive() {
        assert!(is_player_media_path(Path::new("movie.q0s")));
        assert!(is_player_media_path(Path::new("movie.Q0S")));
        assert!(is_player_media_path(Path::new("clip.q0v")));
        assert!(is_player_media_path(Path::new("clip.Q0V")));
        assert!(!is_player_media_path(Path::new("project.q1s")));
        assert!(!is_player_media_path(Path::new("no-extension")));
    }

    #[test]
    fn duplicate_recent_names_include_parent_folder() {
        let paths = vec![
            PathBuf::from("draft/movie.q0s"),
            PathBuf::from("release/movie.q0s"),
            PathBuf::from("release/other.q0s"),
        ];
        assert_eq!(
            recent_menu_labels(&paths),
            vec!["movie.q0s (draft)", "movie.q0s (release)", "other.q0s"]
        );
    }

    #[test]
    fn warning_and_error_statuses_receive_attention_styling() {
        assert!(status_needs_attention("open failed: permission denied"));
        assert!(status_needs_attention(
            "loaded movie.q0s; script warning: line 2"
        ));
        assert!(status_needs_attention(
            "unsupported dropped file: notes.txt"
        ));
        assert!(status_needs_attention("q0s file is too large"));
        assert!(!status_needs_attention("loaded movie.q0s"));
        assert!(!status_needs_attention("paused"));
    }

    #[test]
    fn touch_pad_holds_every_finger_down_until_it_lifts() {
        let ctx = Context::default();
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let [left, _, jump] = TouchPad::buttons(canvas);
        let touch = |id, phase, pos| egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id),
            phase,
            pos,
            force: None,
        };
        let mut pad = TouchPad::default();
        let mut frame = |events| {
            let mut held = (false, false, false);
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| held = pad.held(ctx, canvas),
            );
            held
        };

        let both = vec![
            touch(1, egui::TouchPhase::Start, left.center()),
            touch(2, egui::TouchPhase::Start, jump.center()),
        ];
        assert_eq!(frame(both), (true, false, true), "run and jump together");
        assert_eq!(
            frame(vec![]),
            (true, false, true),
            "held without new events"
        );
        let slide_off = vec![touch(1, egui::TouchPhase::Move, canvas.center())];
        assert_eq!(frame(slide_off), (false, false, true));
        let lift = vec![touch(2, egui::TouchPhase::End, jump.center())];
        assert_eq!(frame(lift), (false, false, false));
    }
}
