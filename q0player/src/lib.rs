pub mod ui;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use q0s_format::q0lang::runtime::{Runtime, RuntimeAction, RuntimeDiagnostic, TimelineTarget};
use q0s_format::rig::{RigControlOverride, RigRuntimeOverrides};
use q0s_format::v2::ProjectV2;
use q0s_format::{is_q0s_v2, parse_q0s, parse_q0s_v2, Bitmap, Error, Movie, Placement};
use q0video::q0v::{Q0vFile, Q0vSpec, MAGIC as Q0V_MAGIC};

#[derive(Debug)]
pub enum PlayerLoadError {
    Io(std::io::Error),
    Parse(Error),
    Media(String),
    TooLarge { size: u64, max: u64 },
}

impl std::fmt::Display for PlayerLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "failed to read file: {}", err),
            Self::Parse(err) => write!(f, "failed to parse q0s: {}", err),
            Self::Media(err) => write!(f, "failed to parse q0v: {err}"),
            Self::TooLarge { size, max } => {
                write!(f, "q0s file is too large ({size} bytes; limit is {max})")
            }
        }
    }
}

impl std::error::Error for PlayerLoadError {}

impl From<std::io::Error> for PlayerLoadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<Error> for PlayerLoadError {
    fn from(value: Error) -> Self {
        Self::Parse(value)
    }
}

/// Internal state kept per-format. v1 walks pre-baked bitmap placements
/// per frame; v2 holds the full vector project and rasterises each frame
/// on demand using the shared software rasteriser.
enum Inner {
    V1 {
        movie: Movie,
        frame_index: usize,
    },
    V2 {
        project: ProjectV2,
        entry_q0rg_id: u16,
        frame_count: u16,
        fps: u16,
        frame_index: u16,
    },
    Q0v {
        media: Q0vFile,
        frame_index: u32,
    },
}

/// Refuse unexpectedly large movies before allocating a matching buffer.
pub const MAX_Q0S_FILE_BYTES: u64 = 256 * 1024 * 1024;

pub struct Player {
    inner: Inner,
    q0lang: Option<Runtime>,
    initial_script_diagnostics: Vec<RuntimeDiagnostic>,
    rig_runtime_overrides: RigRuntimeOverrides,
    playing: bool,
    loop_enabled: bool,
    accumulator_sec: f32,
}

/// Which player-format kind a `Player` was loaded from. Public so the UI
/// can show "Vector v2" vs. "Bitmap v1" badges in the file-info panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerFormat {
    BitmapV1,
    VectorV2,
    Q0v,
}

#[derive(Debug, Clone)]
pub struct Q0vAudioData {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
    pub start_interleaved: usize,
}

impl Player {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, PlayerLoadError> {
        let mut file = File::open(path.as_ref())?;
        let size = file.metadata()?.len();
        if size > MAX_Q0S_FILE_BYTES {
            return Err(PlayerLoadError::TooLarge {
                size,
                max: MAX_Q0S_FILE_BYTES,
            });
        }

        let mut bytes = Vec::with_capacity(size as usize);
        file.by_ref()
            .take(MAX_Q0S_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_Q0S_FILE_BYTES {
            return Err(PlayerLoadError::TooLarge {
                size: bytes.len() as u64,
                max: MAX_Q0S_FILE_BYTES,
            });
        }
        if bytes.starts_with(&Q0V_MAGIC) {
            Self::from_q0v_bytes(bytes)
        } else {
            Self::from_bytes(&bytes).map_err(Into::into)
        }
    }

    pub fn from_q0v_bytes(bytes: Vec<u8>) -> Result<Self, PlayerLoadError> {
        let media = Q0vFile::parse(bytes).map_err(PlayerLoadError::Media)?;
        Ok(Self {
            inner: Inner::Q0v {
                media,
                frame_index: 0,
            },
            q0lang: None,
            initial_script_diagnostics: Vec::new(),
            rig_runtime_overrides: RigRuntimeOverrides::new(),
            playing: true,
            loop_enabled: true,
            accumulator_sec: 0.0,
        })
    }

    /// Auto-detect format: vector v2 (Q0S magic + version 2) takes
    /// precedence; if the magic byte still says Q0S but the version is 1
    /// we fall through to the legacy bitmap loader.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if is_q0s_v2(bytes) {
            let project = parse_q0s_v2(bytes)?;
            let entry_q0rg_id = project.meta.entry_q0rg_id;
            let q = project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == entry_q0rg_id)
                .ok_or(Error::Validation("entry q0rg missing"))?;
            let frame_count = q.frame_count.max(1);
            let entry_script = q.script.clone();
            let fps = project.meta.fps.max(1);
            let inner = Inner::V2 {
                project,
                entry_q0rg_id,
                frame_count,
                fps,
                frame_index: 0,
            };
            let mut player = Self {
                inner,
                q0lang: Some(Runtime::new()),
                initial_script_diagnostics: Vec::new(),
                rig_runtime_overrides: RigRuntimeOverrides::new(),
                playing: true,
                loop_enabled: true,
                accumulator_sec: 0.0,
            };
            player.run_q0lang_source(&entry_script);
            Ok(player)
        } else {
            let movie = parse_q0s(bytes)?;
            Ok(Self {
                inner: Inner::V1 {
                    movie,
                    frame_index: 0,
                },
                q0lang: None,
                initial_script_diagnostics: Vec::new(),
                rig_runtime_overrides: RigRuntimeOverrides::new(),
                playing: true,
                loop_enabled: true,
                accumulator_sec: 0.0,
            })
        }
    }

    pub fn format(&self) -> PlayerFormat {
        match self.inner {
            Inner::V1 { .. } => PlayerFormat::BitmapV1,
            Inner::V2 { .. } => PlayerFormat::VectorV2,
            Inner::Q0v { .. } => PlayerFormat::Q0v,
        }
    }

    /// Vector-format access — returns `Some(&ProjectV2)` if this is a v2
    /// player, `None` for v1. UI uses this for vector-aware file info.
    pub fn project_v2(&self) -> Option<&ProjectV2> {
        match &self.inner {
            Inner::V2 { project, .. } => Some(project),
            _ => None,
        }
    }

    pub fn rig_runtime_overrides(&self) -> &RigRuntimeOverrides {
        &self.rig_runtime_overrides
    }

    /// Diagnostics produced while executing the entry q0lang script.
    /// They are warnings: valid movie content still opens and plays.
    pub fn initial_script_diagnostics(&self) -> &[RuntimeDiagnostic] {
        &self.initial_script_diagnostics
    }

    /// A compact status-line summary suitable for the GUI and bug reports.
    pub fn initial_script_warning(&self) -> Option<String> {
        let first = self.initial_script_diagnostics.first()?;
        let message: String = first.message.chars().take(120).collect();
        let location = if first.line > 0 {
            format!("line {}: ", first.line)
        } else {
            String::new()
        };
        let remaining = self.initial_script_diagnostics.len().saturating_sub(1);
        let suffix = if remaining == 0 {
            String::new()
        } else {
            format!(" (+{remaining} more)")
        };
        Some(format!("script warning: {location}{message}{suffix}"))
    }

    /// Bitmap-format access - `None` for v2. Kept for file-info parity.
    pub fn movie_v1(&self) -> Option<&Movie> {
        match &self.inner {
            Inner::V1 { movie, .. } => Some(movie),
            _ => None,
        }
    }

    pub fn q0v_spec(&self) -> Option<Q0vSpec> {
        match &self.inner {
            Inner::Q0v { media, .. } => Some(media.spec),
            _ => None,
        }
    }

    /// Decode the current q0v frame at its native dimensions. The GUI uploads
    /// this directly and lets the GPU do one linear-filtered aspect-fit; it
    /// must not pre-scale the frame on the CPU and then scale it a second time.
    pub fn q0v_current_frame_rgba(&self) -> Option<(Q0vSpec, Vec<u8>)> {
        let Inner::Q0v { media, frame_index } = &self.inner else {
            return None;
        };
        if !media.spec.video {
            return None;
        }
        let rgba = media.decode_frame_rgba(*frame_index as usize).ok()?;
        Some((media.spec, rgba))
    }

    pub fn q0v_audio_data(&self) -> Option<Q0vAudioData> {
        let Inner::Q0v { media, frame_index } = &self.inner else {
            return None;
        };
        if !media.spec.audio {
            return None;
        }
        let channels = media.spec.audio_channels;
        let sample_rate = media.spec.audio_sample_rate;
        let start_per_channel = u64::from(*frame_index).saturating_mul(u64::from(sample_rate))
            / u64::from(media.spec.fps.max(1));
        let start_interleaved = start_per_channel
            .saturating_mul(u64::from(channels))
            .min(media.audio_pcm_le_bytes().len() as u64 / 2)
            as usize;
        let samples = media
            .audio_pcm_le_bytes()
            .chunks_exact(2)
            .map(|raw| i16::from_le_bytes([raw[0], raw[1]]) as f32 / 32768.0)
            .collect();
        Some(Q0vAudioData {
            channels,
            sample_rate,
            samples,
            start_interleaved,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn set_playing(&mut self, value: bool) {
        self.playing = value;
    }

    pub fn toggle_play_pause(&mut self) {
        self.playing = !self.playing;
    }

    pub fn set_loop_enabled(&mut self, value: bool) {
        self.loop_enabled = value;
    }

    pub fn is_loop_enabled(&self) -> bool {
        self.loop_enabled
    }

    pub fn current_frame(&self) -> usize {
        match self.inner {
            Inner::V1 { frame_index, .. } => frame_index,
            Inner::V2 { frame_index, .. } => usize::from(frame_index),
            Inner::Q0v { frame_index, .. } => frame_index as usize,
        }
    }

    pub fn seek_to_frame(&mut self, target_frame: usize) {
        let target = target_frame.min(self.total_frames().saturating_sub(1));
        match &mut self.inner {
            Inner::V1 { frame_index, .. } => {
                *frame_index = target;
            }
            Inner::V2 { frame_index, .. } => {
                *frame_index = target as u16;
            }
            Inner::Q0v { frame_index, .. } => {
                *frame_index = target as u32;
            }
        }
        self.accumulator_sec = 0.0;
    }

    pub fn total_frames(&self) -> usize {
        match &self.inner {
            Inner::V1 { movie, .. } => usize::from(movie.header.frame_count.max(1)),
            Inner::V2 { frame_count, .. } => usize::from(*frame_count),
            Inner::Q0v { media, .. } => media.spec.timeline_frames.max(1) as usize,
        }
    }

    pub fn fps(&self) -> u16 {
        match &self.inner {
            Inner::V1 { movie, .. } => movie.header.fps.max(1),
            Inner::V2 { fps, .. } => *fps,
            Inner::Q0v { media, .. } => media.spec.fps.clamp(1, u32::from(u16::MAX)) as u16,
        }
    }

    pub fn tick(&mut self, delta_sec: f32) {
        if !self.playing {
            return;
        }
        self.accumulator_sec += delta_sec.max(0.0);
        let frame_time = 1.0_f32 / self.fps() as f32;
        while self.accumulator_sec >= frame_time {
            self.accumulator_sec -= frame_time;
            let total = self.total_frames();
            let mut hit_end = false;
            match &mut self.inner {
                Inner::V1 { frame_index, .. } => {
                    *frame_index += 1;
                    if *frame_index >= total {
                        if self.loop_enabled {
                            *frame_index = 0;
                        } else {
                            *frame_index = total.saturating_sub(1);
                            hit_end = true;
                        }
                    }
                }
                Inner::V2 { frame_index, .. } => {
                    let next = frame_index.saturating_add(1);
                    if usize::from(next) >= total {
                        if self.loop_enabled {
                            *frame_index = 0;
                        } else {
                            *frame_index = total.saturating_sub(1) as u16;
                            hit_end = true;
                        }
                    } else {
                        *frame_index = next;
                    }
                }
                Inner::Q0v { frame_index, .. } => {
                    let next = frame_index.saturating_add(1);
                    if next as usize >= total {
                        if self.loop_enabled {
                            *frame_index = 0;
                        } else {
                            *frame_index = total.saturating_sub(1) as u32;
                            hit_end = true;
                        }
                    } else {
                        *frame_index = next;
                    }
                }
            }
            if hit_end {
                self.playing = false;
                self.accumulator_sec = 0.0;
                break;
            }
        }
    }

    fn run_q0lang_source(&mut self, source: &str) {
        let report = match self.q0lang.as_mut() {
            Some(runtime) => runtime.execute_source(source),
            None => return,
        };
        self.initial_script_diagnostics = report.diagnostics;
        self.apply_q0lang_actions(&report.actions);
    }

    fn apply_q0lang_actions(&mut self, actions: &[RuntimeAction]) {
        let owner_q0rg_id = match &self.inner {
            Inner::V2 { entry_q0rg_id, .. } => Some(*entry_q0rg_id),
            _ => None,
        };
        for action in actions {
            match action {
                RuntimeAction::GoRun(target) => {
                    if let Some(frame) = self.resolve_timeline_target(target) {
                        self.seek_to_frame(frame);
                    }
                    self.playing = true;
                }
                RuntimeAction::GoStop(target) => {
                    if let Some(frame) = self.resolve_timeline_target(target) {
                        self.seek_to_frame(frame);
                    }
                    self.playing = false;
                }
                RuntimeAction::ShellCommand { .. } => {}
                RuntimeAction::RigSetPosition { control, x, y } => {
                    if let Some(owner) = owner_q0rg_id {
                        self.apply_rig_position(owner, control, *x as f32, *y as f32);
                    }
                }
                RuntimeAction::RigSetValue { control, value } => {
                    if let Some(owner) = owner_q0rg_id {
                        self.apply_rig_value(owner, control, *value as f32);
                    }
                }
                RuntimeAction::RigReset { control } => {
                    if let Some(owner) = owner_q0rg_id {
                        self.reset_rig_control(owner, control);
                    }
                }
                RuntimeAction::RigSetPose { pose, weight } => {
                    if let Some(owner) = owner_q0rg_id {
                        self.apply_rig_pose(owner, pose, *weight as f32);
                    }
                }
                RuntimeAction::RigResetPose { pose } => {
                    if let Some(owner) = owner_q0rg_id {
                        self.reset_rig_pose(owner, pose);
                    }
                }
            }
        }
    }

    fn public_rig_control_id(&mut self, q0rg_id: u16, name: &str) -> Option<u16> {
        let control = self
            .project_v2()
            .and_then(|project| q0s_format::rig::rig_for_q0rg(project, q0rg_id))
            .and_then(|rig| rig.controls.iter().find(|control| control.name == name))
            .map(|control| (control.control_id, control.public_in_simple));
        match control {
            Some((id, true)) => Some(id),
            Some((_id, false)) => {
                self.initial_script_diagnostics.push(RuntimeDiagnostic {
                    line: 0,
                    message: format!(
                        "q0.rig control `{name}` is private; enable Visible in Simple to expose it at runtime"
                    ),
                });
                None
            }
            None => {
                self.initial_script_diagnostics.push(RuntimeDiagnostic {
                    line: 0,
                    message: format!("q0.rig control `{name}` was not found on q0rg {q0rg_id}"),
                });
                None
            }
        }
    }

    fn apply_rig_position(&mut self, q0rg_id: u16, name: &str, x: f32, y: f32) {
        let Some(control_id) = self.public_rig_control_id(q0rg_id, name) else {
            return;
        };
        let overrides = self.rig_runtime_overrides.entry(q0rg_id).or_default();
        overrides.retain(|value| {
            !matches!(
                value,
                RigControlOverride::Position { control_id: id, .. } if *id == control_id
            )
        });
        overrides.push(RigControlOverride::Position { control_id, x, y });
    }

    fn apply_rig_value(&mut self, q0rg_id: u16, name: &str, value: f32) {
        let Some(control_id) = self.public_rig_control_id(q0rg_id, name) else {
            return;
        };
        let overrides = self.rig_runtime_overrides.entry(q0rg_id).or_default();
        overrides.retain(|entry| {
            !matches!(
                entry,
                RigControlOverride::Value { control_id: id, .. } if *id == control_id
            )
        });
        overrides.push(RigControlOverride::Value { control_id, value });
    }

    fn reset_rig_control(&mut self, q0rg_id: u16, name: &str) {
        let Some(control_id) = self.public_rig_control_id(q0rg_id, name) else {
            return;
        };
        if let Some(overrides) = self.rig_runtime_overrides.get_mut(&q0rg_id) {
            overrides.retain(|entry| match entry {
                RigControlOverride::Position { control_id: id, .. }
                | RigControlOverride::Value { control_id: id, .. } => *id != control_id,
                RigControlOverride::Pose { .. } => true,
            });
            if overrides.is_empty() {
                self.rig_runtime_overrides.remove(&q0rg_id);
            }
        }
    }

    fn rig_pose_id(&mut self, q0rg_id: u16, name: &str) -> Option<u16> {
        let pose_id = self
            .project_v2()
            .and_then(|project| q0s_format::rig::rig_for_q0rg(project, q0rg_id))
            .and_then(|rig| rig.poses.iter().find(|pose| pose.name == name))
            .map(|pose| pose.pose_id);
        if pose_id.is_none() {
            self.initial_script_diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!("q0.rig pose `{name}` was not found on q0rg {q0rg_id}"),
            });
        }
        pose_id
    }

    fn apply_rig_pose(&mut self, q0rg_id: u16, name: &str, weight: f32) {
        let Some(pose_id) = self.rig_pose_id(q0rg_id, name) else {
            return;
        };
        let overrides = self.rig_runtime_overrides.entry(q0rg_id).or_default();
        overrides.retain(|entry| {
            !matches!(
                entry,
                RigControlOverride::Pose { pose_id: id, .. } if *id == pose_id
            )
        });
        overrides.push(RigControlOverride::Pose {
            pose_id,
            weight: weight.clamp(0.0, 1.0),
        });
    }

    fn reset_rig_pose(&mut self, q0rg_id: u16, name: &str) {
        let Some(pose_id) = self.rig_pose_id(q0rg_id, name) else {
            return;
        };
        if let Some(overrides) = self.rig_runtime_overrides.get_mut(&q0rg_id) {
            overrides.retain(|entry| {
                !matches!(
                    entry,
                    RigControlOverride::Pose { pose_id: id, .. } if *id == pose_id
                )
            });
            if overrides.is_empty() {
                self.rig_runtime_overrides.remove(&q0rg_id);
            }
        }
    }

    fn resolve_timeline_target(&self, target: &TimelineTarget) -> Option<usize> {
        match target {
            TimelineTarget::Frame(frame) => Some(usize::from(*frame)),
            TimelineTarget::Label(label) => label.parse::<usize>().ok(),
        }
    }

    /// Render the current frame at supersample factor 1 (no AA). Kept
    /// for backwards compat — call `render_with_quality` from new UI.
    pub fn render(&self, frame: &mut [u8], viewport_width: u32, viewport_height: u32) {
        self.render_with_quality(frame, viewport_width, viewport_height, 1);
    }

    /// Render with explicit supersample factor `ss` (1, 2, or 4). For
    /// vector v2 movies this drives the rasteriser's AA quality —
    /// `ss=2` is the sane default for live playback. Bitmap v1 ignores
    /// `ss` (the source pixels can't gain detail from upsampling).
    pub fn render_with_quality(
        &self,
        frame: &mut [u8],
        viewport_width: u32,
        viewport_height: u32,
        ss: u8,
    ) {
        match &self.inner {
            Inner::V1 { movie, frame_index } => {
                render_v1(movie, *frame_index, frame, viewport_width, viewport_height)
            }
            Inner::V2 {
                project,
                entry_q0rg_id,
                frame_index,
                ..
            } => {
                let buf = q0s_format::raster::rasterize_q0rg_frame_with_rig_overrides(
                    project,
                    *entry_q0rg_id,
                    *frame_index,
                    viewport_width,
                    viewport_height,
                    ss.max(1) as u32,
                    [0xFF, 0xFF, 0xFF, 0xFF],
                    &self.rig_runtime_overrides,
                );
                let n = (viewport_width as usize)
                    .saturating_mul(viewport_height as usize)
                    .saturating_mul(4)
                    .min(frame.len())
                    .min(buf.len());
                frame[..n].copy_from_slice(&buf[..n]);
            }
            Inner::Q0v { media, frame_index } => render_q0v(
                media,
                *frame_index as usize,
                frame,
                viewport_width,
                viewport_height,
            ),
        }
    }
}

fn render_q0v(
    media: &Q0vFile,
    frame_index: usize,
    frame: &mut [u8],
    viewport_width: u32,
    viewport_height: u32,
) {
    for pixel in frame.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[255, 255, 255, 255]);
    }
    if !media.spec.video || viewport_width == 0 || viewport_height == 0 {
        return;
    }
    let Ok(source) = media.decode_frame_rgba(frame_index) else {
        return;
    };
    let source_width = media.spec.width.max(1) as usize;
    let source_height = media.spec.height.max(1) as usize;
    let destination_width = viewport_width as usize;
    let destination_height = viewport_height as usize;
    for y in 0..destination_height {
        let source_y = y.saturating_mul(source_height) / destination_height.max(1);
        for x in 0..destination_width {
            let source_x = x.saturating_mul(source_width) / destination_width.max(1);
            let source_index = (source_y * source_width + source_x) * 4;
            let destination_index = (y * destination_width + x) * 4;
            if source_index + 4 <= source.len() && destination_index + 4 <= frame.len() {
                frame[destination_index..destination_index + 4]
                    .copy_from_slice(&source[source_index..source_index + 4]);
            }
        }
    }
}

fn render_v1(
    movie: &Movie,
    frame_index: usize,
    frame: &mut [u8],
    viewport_width: u32,
    viewport_height: u32,
) {
    let color = &movie.background;
    for px in frame.chunks_exact_mut(4) {
        px[0] = color.r;
        px[1] = color.g;
        px[2] = color.b;
        px[3] = color.a;
    }
    let placements = movie
        .placements_by_frame
        .get(frame_index)
        .map(|it| it.as_slice())
        .unwrap_or(&[]);
    for placement in placements {
        let Some(bitmap) = movie.bitmaps.get(&placement.bitmap_id) else {
            continue;
        };
        blit_bitmap(frame, viewport_width, viewport_height, bitmap, placement);
    }
}

fn blit_bitmap(
    frame: &mut [u8],
    viewport_width: u32,
    viewport_height: u32,
    bitmap: &Bitmap,
    placement: &Placement,
) {
    let dst_w = (bitmap.width as f32 * placement.scale_x).max(1.0) as i32;
    let dst_h = (bitmap.height as f32 * placement.scale_y).max(1.0) as i32;

    for dy in 0..dst_h {
        let y = placement.y as i32 + dy;
        if y < 0 || y >= viewport_height as i32 {
            continue;
        }
        let src_y = ((dy as f32 / placement.scale_y) as u32).min(bitmap.height as u32 - 1);

        for dx in 0..dst_w {
            let x = placement.x as i32 + dx;
            if x < 0 || x >= viewport_width as i32 {
                continue;
            }
            let src_x = ((dx as f32 / placement.scale_x) as u32).min(bitmap.width as u32 - 1);

            let src_idx = ((src_y * bitmap.width as u32 + src_x) * 4) as usize;
            let dst_idx = ((y as u32 * viewport_width + x as u32) * 4) as usize;

            // Source-over alpha composition: dst = src*a + dst*(1-a).
            // Without this, fully-transparent source pixels would erase the
            // background; previously the player did a raw copy_from_slice and
            // every sprite punched a rectangular hole through the stage.
            let sr = bitmap.rgba[src_idx];
            let sg = bitmap.rgba[src_idx + 1];
            let sb = bitmap.rgba[src_idx + 2];
            let sa = bitmap.rgba[src_idx + 3];
            if sa == 0 {
                continue;
            }
            if sa == 255 {
                frame[dst_idx..dst_idx + 4].copy_from_slice(&[sr, sg, sb, sa]);
                continue;
            }
            let alpha = sa as u32;
            let inv = 255 - alpha;
            let dr = frame[dst_idx] as u32;
            let dg = frame[dst_idx + 1] as u32;
            let db = frame[dst_idx + 2] as u32;
            let da = frame[dst_idx + 3] as u32;
            frame[dst_idx] = ((sr as u32 * alpha + dr * inv) / 255) as u8;
            frame[dst_idx + 1] = ((sg as u32 * alpha + dg * inv) / 255) as u8;
            frame[dst_idx + 2] = ((sb as u32 * alpha + db * inv) / 255) as u8;
            frame[dst_idx + 3] = (da + (255 - da) * alpha / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Player, PlayerFormat, PlayerLoadError, MAX_Q0S_FILE_BYTES};
    use image::ImageEncoder;
    use q0video::q0v::{Q0vSpec, Q0vWriter, MEDIA_TICKS_PER_SECOND};
    use std::io::Cursor;

    static NEXT_TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(rgba, width, height, image::ColorType::Rgba8)
            .expect("encode fixture png");
        bytes
    }

    fn png_pixel(rgba: [u8; 4]) -> Vec<u8> {
        png_rgba(1, 1, &rgba)
    }

    fn q0v_fixture() -> Vec<u8> {
        let spec = Q0vSpec {
            width: 1,
            height: 1,
            fps: 2,
            timeline_frames: 2,
            video: true,
            audio: true,
            audio_sample_rate: 4,
            audio_channels: 1,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).expect("q0v writer");
        writer
            .write_video_frame(0, &png_pixel([255, 0, 0, 255]))
            .expect("red frame");
        writer
            .write_video_frame(MEDIA_TICKS_PER_SECOND / 2, &png_pixel([0, 255, 0, 255]))
            .expect("green frame");
        writer
            .write_audio_pcm_i16(&[0, 8192, 16384, 32767])
            .expect("fixture pcm");
        writer.finish().expect("finish q0v").into_inner()
    }

    fn q0s_v2_with_entry_script(script: &str, frame_count: u16) -> Vec<u8> {
        use q0s_format::v2::{ProjectMeta, ProjectV2, Q0rg};

        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "script-test".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count,
                script: script.to_string(),
                layers: Vec::new(),
            }],
        };
        q0s_format::write_q0s_v2(&project).expect("write q0s v2")
    }

    #[test]
    fn q0v_native_frame_keeps_the_full_source_dimensions_and_pixels() {
        let spec = Q0vSpec {
            width: 2,
            height: 1,
            fps: 1,
            timeline_frames: 1,
            video: true,
            audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).expect("q0v writer");
        writer
            .write_video_frame(0, &png_rgba(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]))
            .expect("two-pixel frame");
        let bytes = writer.finish().expect("finish q0v").into_inner();
        let player = Player::from_q0v_bytes(bytes).expect("load q0v");

        let (decoded_spec, rgba) = player.q0v_current_frame_rgba().expect("native q0v frame");
        assert_eq!((decoded_spec.width, decoded_spec.height), (2, 1));
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn q0v_player_seeks_video_and_audio_on_one_clock() {
        let mut player = Player::from_q0v_bytes(q0v_fixture()).expect("load q0v fixture");
        assert_eq!(player.format(), PlayerFormat::Q0v);
        assert_eq!(player.total_frames(), 2);
        assert_eq!(player.fps(), 2);

        let audio = player.q0v_audio_data().expect("q0v audio");
        assert_eq!(audio.start_interleaved, 0);
        assert_eq!(audio.samples.len(), 4);

        let (spec, native) = player.q0v_current_frame_rgba().expect("native q0v frame");
        assert_eq!((spec.width, spec.height), (1, 1));
        assert_eq!(native, vec![255, 0, 0, 255]);

        let mut frame = vec![0_u8; 4];
        player.render(&mut frame, 1, 1);
        assert_eq!(frame, vec![255, 0, 0, 255]);

        player.seek_to_frame(1);
        let audio = player.q0v_audio_data().expect("seeked q0v audio");
        assert_eq!(audio.start_interleaved, 2);
        player.render(&mut frame, 1, 1);
        assert_eq!(frame, vec![0, 255, 0, 255]);

        player.seek_to_frame(0);
        player.tick(0.5);
        assert_eq!(player.current_frame(), 1);
        player.tick(0.5);
        assert_eq!(
            player.current_frame(),
            0,
            "video and audio loop at the same duration"
        );
    }

    #[test]
    fn q0v_file_magic_is_auto_detected_by_from_file() {
        let directory = std::env::temp_dir().join(format!(
            "q0player-q0v-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create q0v test directory");
        let path = directory.join("fixture.q0v");
        std::fs::write(&path, q0v_fixture()).expect("write q0v fixture");
        let player = Player::from_file(&path).expect("auto-detect q0v");
        assert_eq!(player.format(), PlayerFormat::Q0v);
        std::fs::remove_dir_all(directory).expect("cleanup q0v fixture");
    }

    #[test]
    fn player_stops_at_last_frame_when_loop_is_disabled() {
        let bytes = include_bytes!("../../q0s-format/testdata/one_sprite.q0s");
        let mut player = Player::from_bytes(bytes).expect("must parse");
        player.set_loop_enabled(false);
        player.set_playing(true);
        player.tick(1.0);
        assert_eq!(player.current_frame(), player.total_frames() - 1);
        assert!(!player.is_playing());
    }

    fn q0s_v2_with_rig_script(script: &str, public: bool) -> Vec<u8> {
        use q0s_format::transform::Affine;
        use q0s_format::v2::{
            Anchor, Asset, Layer, Path, Placement as VPlacement, ProjectMeta, ProjectV2, Q0rg,
            Rgba, RigAsset, RigBinding, RigControl, RigControlKind, RigNode, Target, Transform2D,
            Tween, Vec2, VectorAsset,
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "rig-runtime".into(),
                fps: 24,
                stage_width: 32,
                stage_height: 16,
                entry_q0rg_id: 1,
            },
            assets: vec![
                Asset::Vector(VectorAsset {
                    asset_id: 1,
                    paths: vec![Path {
                        closed: true,
                        anchors: vec![
                            Anchor {
                                point: Vec2::new(0.0, 0.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(4.0, 0.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(4.0, 4.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(0.0, 4.0),
                                in_handle: None,
                                out_handle: None,
                            },
                        ],
                    }],
                    fill: Some(Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    }),
                    stroke: None,
                }),
                Asset::Rig(RigAsset {
                    asset_id: 2,
                    owner_q0rg_id: 1,
                    nodes: vec![RigNode {
                        node_id: 1,
                        name: "root".into(),
                        parent: None,
                        rest: Transform2D::IDENTITY,
                        length: 4.0,
                        binding: Some(RigBinding {
                            instance_id: 1,
                            bind_offset: Affine::IDENTITY,
                        }),
                    }],
                    controls: vec![RigControl {
                        control_id: 1,
                        name: "look".into(),
                        kind: RigControlKind::Position2D,
                        target_node: Some(1),
                        rest_x: 2.0,
                        rest_y: 4.0,
                        rest_value: 0.0,
                        min_value: -100.0,
                        max_value: 100.0,
                        public_in_simple: public,
                    }],
                    constraints: Vec::new(),
                    channels: Vec::new(),
                    drivers: Vec::new(),
                    poses: Vec::new(),
                    deformers: Vec::new(),
                    pose_drivers: Vec::new(),
                    mirror_pairs: Vec::new(),
                    variants: Vec::new(),
                }),
            ],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: script.into(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![VPlacement {
                        instance_id: 1,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        q0s_format::write_q0s_v2(&project).expect("write rig-runtime q0s")
    }

    fn non_white_x_bounds(rgba: &[u8], width: usize) -> Option<(usize, usize)> {
        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        let mut found = false;
        for (index, pixel) in rgba.chunks_exact(4).enumerate() {
            if pixel[0..3] != [255, 255, 255] {
                let x = index % width;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                found = true;
            }
        }
        found.then_some((min_x, max_x))
    }

    #[test]
    fn q0player_rigged_frame_matches_shared_q0s_raster_exactly() {
        let bytes = q0s_v2_with_rig_script("", true);
        let project = q0s_format::parse_q0s_v2(&bytes).expect("parse exported rig project");
        let player = Player::from_bytes(&bytes).expect("load rigged player movie");
        let mut actual = vec![0_u8; 32 * 16 * 4];
        player.render_with_quality(&mut actual, 32, 16, 1);
        let expected = q0s_format::raster::rasterize_q0rg_frame(
            &project,
            project.meta.entry_q0rg_id,
            0,
            32,
            16,
            1,
            [255, 255, 255, 255],
        );
        assert_eq!(actual, expected, "q0player diverged from shared rig raster");
    }

    #[test]
    fn q0lang_rig_position_reaches_player_runtime_and_moves_pixels() {
        let baseline_bytes = q0s_v2_with_rig_script("", true);
        let scripted_bytes =
            q0s_v2_with_rig_script("import q0.rig\nq0rig.position! \"look\", 20, 4\n", true);
        let baseline = Player::from_bytes(&baseline_bytes).expect("baseline player");
        let scripted = Player::from_bytes(&scripted_bytes).expect("scripted player");

        let mut base_pixels = vec![0_u8; 32 * 16 * 4];
        let mut scripted_pixels = vec![0_u8; 32 * 16 * 4];
        baseline.render_with_quality(&mut base_pixels, 32, 16, 1);
        scripted.render_with_quality(&mut scripted_pixels, 32, 16, 1);
        let base_bounds = non_white_x_bounds(&base_pixels, 32).expect("baseline painted pixels");
        let scripted_bounds =
            non_white_x_bounds(&scripted_pixels, 32).expect("scripted painted pixels");
        assert!(base_bounds.1 < 10, "baseline bounds={base_bounds:?}");
        assert!(
            scripted_bounds.0 >= 19,
            "scripted bounds={scripted_bounds:?}"
        );
        assert_ne!(base_pixels, scripted_pixels);
        assert!(matches!(
            scripted.rig_runtime_overrides().get(&1).and_then(|values| values.first()),
            Some(q0s_format::rig::RigControlOverride::Position { control_id: 1, x, y })
                if (*x - 20.0).abs() < 1.0e-6 && (*y - 4.0).abs() < 1.0e-6
        ));
    }

    #[test]
    fn q0lang_rig_reset_removes_runtime_override() {
        let bytes = q0s_v2_with_rig_script(
            "q0rig.position! \"look\", 20, 4\nq0rig.reset! \"look\"\n",
            true,
        );
        let player = Player::from_bytes(&bytes).expect("player");
        assert!(player.rig_runtime_overrides().get(&1).is_none());
        let mut pixels = vec![0_u8; 32 * 16 * 4];
        player.render_with_quality(&mut pixels, 32, 16, 1);
        let bounds = non_white_x_bounds(&pixels, 32).expect("painted pixels");
        assert!(
            bounds.1 < 10,
            "reset must restore authored pose; bounds={bounds:?}"
        );
    }

    #[test]
    fn q0lang_cannot_drive_private_rig_control() {
        let bytes = q0s_v2_with_rig_script("q0rig.position! \"look\", 20, 4\n", false);
        let player = Player::from_bytes(&bytes).expect("private control still loads movie");
        assert!(player.rig_runtime_overrides().is_empty());
        assert!(player
            .initial_script_diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("private")));
    }

    #[test]
    fn v2_entry_q0lang_gostop_runs_on_load() {
        let bytes = q0s_v2_with_entry_script("gostop! 2\n", 4);
        let player = Player::from_bytes(&bytes).expect("must parse");

        assert_eq!(player.current_frame(), 2);
        assert!(!player.is_playing());
    }

    #[test]
    fn v2_entry_q0lang_gorun_runs_on_load() {
        let bytes = q0s_v2_with_entry_script("gorun! 3\n", 5);
        let player = Player::from_bytes(&bytes).expect("must parse");

        assert_eq!(player.current_frame(), 3);
        assert!(player.is_playing());
    }

    #[test]
    fn entry_script_diagnostics_are_preserved_as_non_fatal_warnings() {
        let bytes = q0s_v2_with_entry_script("gostop 2\n", 4);
        let player = Player::from_bytes(&bytes).expect("movie still opens");

        assert!(!player.initial_script_diagnostics().is_empty());
        let warning = player.initial_script_warning().expect("warning summary");
        assert!(warning.contains("line 1"));
        assert!(warning.contains("must end with"));
    }

    #[test]
    fn oversized_file_is_rejected_before_reading_it() {
        let directory = std::env::temp_dir().join(format!(
            "q0player-large-file-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("oversized.q0s");
        let file = std::fs::File::create(&path).expect("create sparse test file");
        file.set_len(MAX_Q0S_FILE_BYTES + 1)
            .expect("extend test file");
        drop(file);

        let error = match Player::from_file(&path) {
            Ok(_) => panic!("oversized file must be rejected"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            PlayerLoadError::TooLarge {
                size,
                max: MAX_Q0S_FILE_BYTES
            } if size == MAX_Q0S_FILE_BYTES + 1
        ));
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn direct_seek_supports_backwards_frames() {
        let bytes = q0s_v2_with_entry_script("", 5);
        let mut player = Player::from_bytes(&bytes).expect("must parse");

        player.seek_to_frame(4);
        assert_eq!(player.current_frame(), 4);
        player.seek_to_frame(1);
        assert_eq!(player.current_frame(), 1);
    }

    #[test]
    fn render_writes_bitmap_pixels() {
        let bytes = include_bytes!("../../q0s-format/testdata/one_sprite.q0s");
        let player = Player::from_bytes(bytes).expect("must parse");
        let width = 64_u32;
        let height = 64_u32;
        let mut frame = vec![0_u8; (width * height * 4) as usize];
        player.render(&mut frame, width, height);

        let idx = ((24 * width + 32) * 4) as usize;
        assert_eq!(&frame[idx..idx + 4], &[255, 0, 0, 255]);
    }

    /// Regression: previously `blit_bitmap` did a straight `copy_from_slice`,
    /// so an entirely transparent sprite still erased the background to all
    /// zeros. After the source-over fix, transparent source pixels MUST leave
    /// the destination intact.
    #[test]
    fn transparent_sprite_pixel_preserves_background() {
        use q0s_format::{Background, Bitmap, Header, Movie, Placement, SUPPORTED_VERSION};
        use std::collections::HashMap;

        let bitmap = Bitmap {
            id: 1,
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 0], // fully transparent red
        };
        let mut bitmaps = HashMap::new();
        bitmaps.insert(1, bitmap);

        let movie = Movie {
            header: Header {
                version: SUPPORTED_VERSION,
                fps: 24,
                frame_count: 1,
            },
            background: Background {
                r: 50,
                g: 60,
                b: 70,
                a: 255,
            },
            bitmaps,
            placements_by_frame: vec![vec![Placement {
                frame: 0,
                bitmap_id: 1,
                x: 0,
                y: 0,
                scale_x: 1.0,
                scale_y: 1.0,
            }]],
        };
        let mut player = Player::from_bytes(&q0s_format::write_q0s(&movie).expect("write"))
            .expect("parse roundtrip");
        player.set_playing(false);
        let mut frame = vec![0_u8; 4];
        player.render(&mut frame, 1, 1);
        // Background should be intact — alpha-zero source pixel must NOT punch through.
        assert_eq!(frame, vec![50, 60, 70, 255]);
    }

    fn q0s_v2_with_bend_script(script: &str) -> Vec<u8> {
        use q0s_format::transform::Affine;
        use q0s_format::v2::{
            Anchor, Asset, Layer, Path, Placement as VPlacement, ProjectMeta, ProjectV2, Q0rg,
            Rgba, RigAsset, RigControl, RigControlKind, RigDeformer, Target, Transform2D, Tween,
            Vec2, VectorAsset,
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "bend-runtime".into(),
                fps: 24,
                stage_width: 32,
                stage_height: 20,
                entry_q0rg_id: 1,
            },
            assets: vec![
                Asset::Vector(VectorAsset {
                    asset_id: 1,
                    paths: vec![Path {
                        closed: true,
                        anchors: vec![
                            Anchor {
                                point: Vec2::new(0.0, -2.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(16.0, -2.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(16.0, 2.0),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(0.0, 2.0),
                                in_handle: None,
                                out_handle: None,
                            },
                        ],
                    }],
                    fill: Some(Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    }),
                    stroke: None,
                }),
                Asset::Rig(RigAsset {
                    asset_id: 2,
                    owner_q0rg_id: 1,
                    nodes: Vec::new(),
                    controls: vec![
                        RigControl {
                            control_id: 1,
                            name: "start".into(),
                            kind: RigControlKind::Position2D,
                            target_node: None,
                            rest_x: 4.0,
                            rest_y: 10.0,
                            rest_value: 0.0,
                            min_value: -100.0,
                            max_value: 100.0,
                            public_in_simple: true,
                        },
                        RigControl {
                            control_id: 2,
                            name: "bend".into(),
                            kind: RigControlKind::Position2D,
                            target_node: None,
                            rest_x: 12.0,
                            rest_y: 10.0,
                            rest_value: 0.0,
                            min_value: -100.0,
                            max_value: 100.0,
                            public_in_simple: true,
                        },
                        RigControl {
                            control_id: 3,
                            name: "end".into(),
                            kind: RigControlKind::Position2D,
                            target_node: None,
                            rest_x: 20.0,
                            rest_y: 10.0,
                            rest_value: 0.0,
                            min_value: -100.0,
                            max_value: 100.0,
                            public_in_simple: true,
                        },
                    ],
                    constraints: Vec::new(),
                    channels: Vec::new(),
                    drivers: Vec::new(),
                    poses: Vec::new(),
                    deformers: vec![RigDeformer::Bend {
                        deformer_id: 1,
                        instance_id: 7,
                        asset_id: 1,
                        bind_transform: Affine {
                            tx: 4.0,
                            ty: 10.0,
                            ..Affine::IDENTITY
                        },
                        axis_start: Vec2::new(0.0, 0.0),
                        axis_end: Vec2::new(16.0, 0.0),
                        start_control: 1,
                        middle_control: 2,
                        end_control: 3,
                    }],
                    pose_drivers: Vec::new(),
                    mirror_pairs: Vec::new(),
                    variants: Vec::new(),
                }),
            ],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: script.into(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![VPlacement {
                        instance_id: 7,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            tx: 4.0,
                            ty: 10.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        q0s_format::write_q0s_v2(&project).expect("write bend-runtime q0s")
    }

    #[test]
    fn q0player_deformer_uses_shared_raster_and_q0lang_runtime_override() {
        let baseline_bytes = q0s_v2_with_bend_script("");
        let scripted_bytes =
            q0s_v2_with_bend_script("import q0.rig\nq0rig.position! \"bend\", 12, 4\n");
        let baseline_project =
            q0s_format::parse_q0s_v2(&baseline_bytes).expect("parse baseline bend");
        let baseline = Player::from_bytes(&baseline_bytes).expect("baseline bend player");
        let scripted = Player::from_bytes(&scripted_bytes).expect("scripted bend player");
        let mut baseline_pixels = vec![0_u8; 32 * 20 * 4];
        let mut scripted_pixels = vec![0_u8; 32 * 20 * 4];
        baseline.render_with_quality(&mut baseline_pixels, 32, 20, 1);
        scripted.render_with_quality(&mut scripted_pixels, 32, 20, 1);
        let expected = q0s_format::raster::rasterize_q0rg_frame(
            &baseline_project,
            1,
            0,
            32,
            20,
            1,
            [255, 255, 255, 255],
        );
        assert_eq!(
            baseline_pixels, expected,
            "player baseline diverged from shared deformer raster"
        );
        assert_ne!(
            baseline_pixels, scripted_pixels,
            "runtime bend control did not deform player pixels"
        );
        let scripted_dark_y = scripted_pixels
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(index, pixel)| (pixel[0..3] != [255, 255, 255]).then_some(index / 32))
            .min()
            .expect("scripted bend painted pixels");
        let baseline_dark_y = baseline_pixels
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(index, pixel)| (pixel[0..3] != [255, 255, 255]).then_some(index / 32))
            .min()
            .expect("baseline bend painted pixels");
        assert!(scripted_dark_y < baseline_dark_y, "bend did not move silhouette upward: baseline={baseline_dark_y}, scripted={scripted_dark_y}");
    }

    fn q0s_v2_with_variant_script(script: &str) -> Vec<u8> {
        use q0s_format::v2::{
            Anchor, Asset, Layer, Path, Placement as VPlacement, ProjectMeta, ProjectV2, Q0rg,
            Rgba, RigAsset, RigControl, RigControlKind, RigPosePreset, RigPoseValue,
            RigPropertyRef, RigVariantChoice, RigVariantSet, Target, Transform2D, Tween, Vec2,
            VectorAsset,
        };
        let shape = |asset_id: u16, color: Rgba| {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![Path {
                    closed: true,
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(0.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(12.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(12.0, 8.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(0.0, 8.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                }],
                fill: Some(color),
                stroke: None,
            })
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "variant-runtime".into(),
                fps: 24,
                stage_width: 24,
                stage_height: 16,
                entry_q0rg_id: 1,
            },
            assets: vec![
                shape(
                    1,
                    Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                ),
                shape(
                    2,
                    Rgba {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                ),
                Asset::Rig(RigAsset {
                    asset_id: 3,
                    owner_q0rg_id: 1,
                    nodes: Vec::new(),
                    controls: vec![RigControl {
                        control_id: 1,
                        name: "mouth".into(),
                        kind: RigControlKind::Slider,
                        target_node: None,
                        rest_x: 0.0,
                        rest_y: 0.0,
                        rest_value: 0.0,
                        min_value: 0.0,
                        max_value: 1.0,
                        public_in_simple: true,
                    }],
                    constraints: Vec::new(),
                    channels: Vec::new(),
                    drivers: Vec::new(),
                    poses: vec![RigPosePreset {
                        pose_id: 1,
                        name: "red pose".into(),
                        values: vec![RigPoseValue {
                            property: RigPropertyRef::ControlValue(1),
                            value: 1.0,
                        }],
                    }],
                    deformers: Vec::new(),
                    pose_drivers: Vec::new(),
                    mirror_pairs: Vec::new(),
                    variants: vec![RigVariantSet {
                        variant_id: 1,
                        name: "mouths".into(),
                        instance_id: 7,
                        source_control: 1,
                        choices: vec![
                            RigVariantChoice {
                                name: "black".into(),
                                target: Target::Asset(1),
                            },
                            RigVariantChoice {
                                name: "red".into(),
                                target: Target::Asset(2),
                            },
                        ],
                    }],
                }),
            ],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: script.into(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![VPlacement {
                        instance_id: 7,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            tx: 5.0,
                            ty: 4.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        q0s_format::write_q0s_v2(&project).expect("write variant-runtime q0s")
    }

    #[test]
    fn q0player_pose_apply_and_reset_are_ephemeral() {
        let bytes = q0s_v2_with_variant_script("");
        let baseline = Player::from_bytes(&bytes).expect("baseline pose player");
        let mut posed = Player::from_bytes(&bytes).expect("posed player");
        posed.apply_rig_pose(1, "red pose", 1.0);
        let mut reset = Player::from_bytes(&bytes).expect("reset pose player");
        reset.apply_rig_pose(1, "red pose", 1.0);
        reset.reset_rig_pose(1, "red pose");
        let mut baseline_pixels = vec![0_u8; 24 * 16 * 4];
        let mut posed_pixels = vec![0_u8; 24 * 16 * 4];
        let mut reset_pixels = vec![0_u8; 24 * 16 * 4];
        baseline.render_with_quality(&mut baseline_pixels, 24, 16, 1);
        posed.render_with_quality(&mut posed_pixels, 24, 16, 1);
        reset.render_with_quality(&mut reset_pixels, 24, 16, 1);
        assert_ne!(baseline_pixels, posed_pixels);
        assert_eq!(baseline_pixels, reset_pixels);
        assert!(posed_pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0..3] == [255, 0, 0]));
    }

    #[test]
    fn q0player_variant_switches_same_stable_instance_from_q0lang_value() {
        let baseline_bytes = q0s_v2_with_variant_script("");
        let scripted_bytes =
            q0s_v2_with_variant_script("import q0.rig\nq0rig.value! \"mouth\", 1\n");
        let baseline = Player::from_bytes(&baseline_bytes).expect("baseline variant player");
        let scripted = Player::from_bytes(&scripted_bytes).expect("scripted variant player");
        let mut baseline_pixels = vec![0_u8; 24 * 16 * 4];
        let mut scripted_pixels = vec![0_u8; 24 * 16 * 4];
        baseline.render_with_quality(&mut baseline_pixels, 24, 16, 1);
        scripted.render_with_quality(&mut scripted_pixels, 24, 16, 1);
        assert_ne!(
            baseline_pixels, scripted_pixels,
            "variant control did not switch rendered target"
        );
        assert!(baseline_pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0..3] == [0, 0, 0]));
        assert!(scripted_pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0..3] == [255, 0, 0]));
    }
}
