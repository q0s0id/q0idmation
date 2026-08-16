pub mod ui;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use q0s_format::q0lang::runtime::{
    Runtime, RuntimeAction, RuntimeDiagnostic, RuntimeValue, TimelineTarget,
};
use q0s_format::rig::{RigControlOverride, RigRuntimeOverrides};
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::v2::{Asset, ProjectDependencyKind, ProjectDependencySource, ProjectV2};
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
        project: Arc<ProjectV2>,
        entry_q0rg_id: u16,
        frame_count: u16,
        fps: u16,
        frame_index: u16,
    },
    Q0v {
        media: Arc<Q0vFile>,
        frame_index: u32,
    },
}

/// Refuse unexpectedly large movies before allocating a matching buffer.
pub const MAX_Q0S_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const PLAYER_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub(crate) const PLAYER_AUDIO_CHANNELS: u16 = 2;
const MAX_FRAME_SCRIPT_TRANSITIONS: usize = 64;

#[derive(Debug, Clone, Copy, Default)]
struct PlayerInputState {
    left: bool,
    right: bool,
    jump: bool,
    jump_pressed: bool,
}

#[derive(Clone)]
pub(crate) struct PlayerAudioClip {
    pub(crate) start_sample_frame: u64,
    pub(crate) end_sample_frame: u64,
    pub(crate) media: Arc<Q0vFile>,
    pub(crate) gain: f32,
}

#[derive(Clone)]
pub(crate) struct PlayerAudioTimeline {
    pub(crate) clips: Vec<PlayerAudioClip>,
    pub(crate) total_sample_frames: u64,
    pub(crate) start_sample_frame: u64,
}

pub struct Player {
    inner: Inner,
    q0lang: Option<Runtime>,
    initial_script_diagnostics: Vec<RuntimeDiagnostic>,
    rig_runtime_overrides: RigRuntimeOverrides,
    scene_runtime: RuntimeSceneState,
    scene_library: HashMap<String, Arc<ProjectV2>>,
    active_scene_alias: Option<String>,
    scene_generation: u64,
    input: PlayerInputState,
    audio_timeline: Option<PlayerAudioTimeline>,
    playing: bool,
    loop_enabled: bool,
    accumulator_sec: f32,
    /// Number of dispatched frame-entry events. Used during runtime init to
    /// avoid firing the destination frame twice after an entry-script goto.
    frame_script_entries: u64,
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

fn source_duration_at_player_rate(media: &Q0vFile) -> u64 {
    media
        .audio_samples_per_channel
        .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
        .div_ceil(u64::from(media.spec.audio_sample_rate.max(1)))
}

fn build_q0v_audio_timeline(media: Arc<Q0vFile>) -> Option<PlayerAudioTimeline> {
    if !media.spec.audio || media.spec.audio_channels == 0 || media.spec.audio_sample_rate == 0 {
        return None;
    }
    let total_sample_frames = u64::from(media.spec.timeline_frames.max(1))
        .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
        .div_ceil(u64::from(media.spec.fps.max(1)))
        .max(1);
    let end_sample_frame = source_duration_at_player_rate(&media).min(total_sample_frames);
    (end_sample_frame > 0).then(|| PlayerAudioTimeline {
        clips: vec![PlayerAudioClip {
            start_sample_frame: 0,
            end_sample_frame,
            media,
            gain: 1.0,
        }],
        total_sample_frames,
        start_sample_frame: 0,
    })
}

fn build_project_audio_timeline(project: &ProjectV2) -> Option<PlayerAudioTimeline> {
    let q0rg_id = project.meta.entry_q0rg_id;
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let host_fps = u64::from(project.meta.fps.max(1));
    let total_sample_frames = u64::from(q0rg.frame_count.max(1))
        .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
        .div_ceil(host_fps)
        .max(1);
    let mut parsed_by_asset: HashMap<u16, Arc<Q0vFile>> = HashMap::new();
    let mut clips = Vec::new();

    for clip in project
        .audio_clips
        .iter()
        .filter(|clip| clip.q0rg_id == q0rg_id && !clip.muted && clip.gain > 0.0)
    {
        let media = if let Some(media) = parsed_by_asset.get(&clip.asset_id) {
            Arc::clone(media)
        } else {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id() == clip.asset_id)?;
            let Asset::Q0v(asset) = asset else {
                continue;
            };
            let parsed = Arc::new(Q0vFile::parse(asset.bytes.clone()).ok()?);
            parsed_by_asset.insert(clip.asset_id, Arc::clone(&parsed));
            parsed
        };
        if !media.spec.audio || media.spec.video {
            continue;
        }
        let start_sample_frame = u64::from(clip.start_frame)
            .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
            / host_fps;
        let boundary_frame = q0s_format::v2::audio_clip_end_frame(project, *clip)?;
        let boundary_sample_frame = u64::from(boundary_frame)
            .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
            / host_fps;
        let end_sample_frame = start_sample_frame
            .saturating_add(source_duration_at_player_rate(&media))
            .min(boundary_sample_frame)
            .min(total_sample_frames);
        if end_sample_frame > start_sample_frame {
            clips.push(PlayerAudioClip {
                start_sample_frame,
                end_sample_frame,
                media,
                gain: clip.gain.clamp(0.0, 4.0),
            });
        }
    }

    (!clips.is_empty()).then_some(PlayerAudioTimeline {
        clips,
        total_sample_frames,
        start_sample_frame: 0,
    })
}

pub(crate) fn sample_audio_timeline(
    timeline: &PlayerAudioTimeline,
    sample_frame: u64,
    output_channel: u16,
) -> f32 {
    let mut mixed = 0.0_f32;
    for clip in &timeline.clips {
        if sample_frame < clip.start_sample_frame || sample_frame >= clip.end_sample_frame {
            continue;
        }
        let media = &clip.media;
        let channels = usize::from(media.spec.audio_channels);
        let source_rate = u64::from(media.spec.audio_sample_rate.max(1));
        if channels == 0 {
            continue;
        }
        let pcm = media.audio_pcm_le_bytes();
        let source_frames = pcm.len() / 2 / channels;
        if source_frames == 0 {
            continue;
        }
        let local_output_frame = sample_frame - clip.start_sample_frame;
        let source_numerator = local_output_frame.saturating_mul(source_rate);
        let source_index = (source_numerator / u64::from(PLAYER_AUDIO_SAMPLE_RATE)) as usize;
        let next_index = source_index.saturating_add(1).min(source_frames - 1);
        let fraction = (source_numerator % u64::from(PLAYER_AUDIO_SAMPLE_RATE)) as f32
            / PLAYER_AUDIO_SAMPLE_RATE as f32;
        let source_channel = if channels == 1 {
            0
        } else {
            usize::from(output_channel).min(channels - 1)
        };
        let read = |frame: usize| {
            let index = (frame.min(source_frames - 1) * channels + source_channel) * 2;
            i16::from_le_bytes([pcm[index], pcm[index + 1]]) as f32 / 32768.0
        };
        let a = read(source_index);
        let b = read(next_index);
        mixed += (a + (b - a) * fraction) * clip.gain;
    }
    mixed.clamp(-1.0, 1.0)
}

fn build_bundled_scene_library(
    project: &ProjectV2,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) -> HashMap<String, Arc<ProjectV2>> {
    let mut scenes = HashMap::new();
    for node in
        project.runtime.project_graph.nodes.iter().filter(|node| {
            node.parent_node_id.is_none() && node.kind == ProjectDependencyKind::Movie
        })
    {
        match &node.source {
            ProjectDependencySource::Embedded(bytes) => match parse_q0s_v2(bytes) {
                Ok(scene) => {
                    scenes.insert(node.alias.clone(), Arc::new(scene));
                }
                Err(error) => diagnostics.push(RuntimeDiagnostic {
                    line: 0,
                    message: format!(
                        "bundled movie dependency `{}` cannot be loaded as a scene: {error}",
                        node.alias
                    ),
                }),
            },
            ProjectDependencySource::External(path) => diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!(
                    "external movie dependency `{}` ({path}) cannot be scene-switched by q0player; export a bundled q0s",
                    node.alias
                ),
            }),
        }
    }
    scenes
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
        let media = Arc::new(Q0vFile::parse(bytes).map_err(PlayerLoadError::Media)?);
        let audio_timeline = build_q0v_audio_timeline(Arc::clone(&media));
        Ok(Self {
            inner: Inner::Q0v {
                media,
                frame_index: 0,
            },
            q0lang: None,
            initial_script_diagnostics: Vec::new(),
            rig_runtime_overrides: RigRuntimeOverrides::new(),
            scene_runtime: RuntimeSceneState::new(),
            scene_library: HashMap::new(),
            active_scene_alias: None,
            scene_generation: 0,
            input: PlayerInputState::default(),
            audio_timeline,
            playing: true,
            loop_enabled: true,
            accumulator_sec: 0.0,
            frame_script_entries: 0,
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
            let startup_q0lang = project
                .runtime
                .project_graph
                .nodes
                .iter()
                .filter(|node| {
                    node.parent_node_id.is_none() && node.kind == ProjectDependencyKind::Q0lang
                })
                .map(|node| (node.node_id, node.alias.clone(), node.source.clone()))
                .collect::<Vec<_>>();
            let audio_timeline = build_project_audio_timeline(&project);
            let mut initial_script_diagnostics = Vec::new();
            let scene_library =
                build_bundled_scene_library(&project, &mut initial_script_diagnostics);
            let inner = Inner::V2 {
                project: Arc::new(project),
                entry_q0rg_id,
                frame_count,
                fps,
                frame_index: 0,
            };
            let mut player = Self {
                inner,
                q0lang: Some(Runtime::new()),
                initial_script_diagnostics,
                rig_runtime_overrides: RigRuntimeOverrides::new(),
                scene_runtime: RuntimeSceneState::new(),
                scene_library,
                active_scene_alias: None,
                scene_generation: 0,
                input: PlayerInputState::default(),
                audio_timeline,
                playing: true,
                loop_enabled: true,
                accumulator_sec: 0.0,
                frame_script_entries: 0,
            };
            let mut transition_budget = MAX_FRAME_SCRIPT_TRANSITIONS;
            player.sync_q0lang_host_globals();
            let mut startup_q0lang = startup_q0lang;
            startup_q0lang.sort_by_key(|(node_id, _, _)| *node_id);
            for (_, alias, source) in startup_q0lang {
                match source {
                    ProjectDependencySource::Embedded(bytes) => match String::from_utf8(bytes) {
                        Ok(source) => {
                            player.run_q0lang_source_with_budget(&source, &mut transition_budget)
                        }
                        Err(_) => player.initial_script_diagnostics.push(RuntimeDiagnostic {
                            line: 0,
                            message: format!(
                                "embedded q0lang dependency `{alias}` is not valid utf-8"
                            ),
                        }),
                    },
                    ProjectDependencySource::External(path) => {
                        player.initial_script_diagnostics.push(RuntimeDiagnostic {
                            line: 0,
                            message: format!(
                                "external q0lang dependency `{alias}` ({path}) is not executed by q0player; export a bundled q0s"
                            ),
                        });
                    }
                }
            }
            let entries_before_init = player.frame_script_entries;
            player.run_q0lang_source_with_budget(&entry_script, &mut transition_budget);
            if player.frame_script_entries == entries_before_init {
                player.run_current_frame_scripts(&mut transition_budget);
            }
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
                scene_runtime: RuntimeSceneState::new(),
                scene_library: HashMap::new(),
                active_scene_alias: None,
                scene_generation: 0,
                input: PlayerInputState::default(),
                audio_timeline: None,
                playing: true,
                loop_enabled: true,
                accumulator_sec: 0.0,
                frame_script_entries: 0,
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
            Inner::V2 { project, .. } => Some(project.as_ref()),
            _ => None,
        }
    }

    pub fn active_scene_alias(&self) -> Option<&str> {
        self.active_scene_alias.as_deref()
    }

    pub fn scene_generation(&self) -> u64 {
        self.scene_generation
    }

    pub fn rig_runtime_overrides(&self) -> &RigRuntimeOverrides {
        &self.rig_runtime_overrides
    }

    pub fn scene_runtime(&self) -> &RuntimeSceneState {
        &self.scene_runtime
    }

    pub fn set_input_state(&mut self, left: bool, right: bool, jump: bool) {
        if jump && !self.input.jump {
            self.input.jump_pressed = true;
        }
        self.input.left = left;
        self.input.right = right;
        self.input.jump = jump;
    }

    pub fn uses_game_input(&self) -> bool {
        self.q0lang
            .as_ref()
            .is_some_and(|runtime| runtime.imported_libraries.contains("q0.input"))
    }

    pub fn runtime_instance_metrics(
        &self,
        runtime_name: &str,
    ) -> Option<q0s_format::raster::RuntimeInstanceMetrics> {
        let (project, owner, frame) = match &self.inner {
            Inner::V2 {
                project,
                entry_q0rg_id,
                frame_index,
                ..
            } => (project.as_ref(), *entry_q0rg_id, *frame_index),
            _ => return None,
        };
        let key = project
            .runtime
            .instance_names
            .iter()
            .find_map(|(key, name)| {
                (name == runtime_name && key.q0rg_id == owner).then_some(*key)
            })?;
        q0s_format::raster::runtime_instance_metrics(
            project,
            owner,
            frame,
            key.instance_id,
            &self.scene_runtime,
        )
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

    pub(crate) fn has_audio_timeline(&self) -> bool {
        self.audio_timeline.is_some()
    }

    pub(crate) fn audio_timeline(&self) -> Option<PlayerAudioTimeline> {
        let mut timeline = self.audio_timeline.clone()?;
        timeline.start_sample_frame = (self.current_frame() as u64)
            .saturating_mul(u64::from(PLAYER_AUDIO_SAMPLE_RATE))
            / u64::from(self.fps().max(1));
        timeline.start_sample_frame = timeline
            .start_sample_frame
            .min(timeline.total_sample_frames);
        Some(timeline)
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
        self.set_frame_raw(target);
        self.accumulator_sec = 0.0;
        if matches!(self.inner, Inner::V2 { .. }) {
            let mut transition_budget = MAX_FRAME_SCRIPT_TRANSITIONS;
            self.run_current_frame_scripts(&mut transition_budget);
        }
    }

    fn set_frame_raw(&mut self, target: usize) {
        match &mut self.inner {
            Inner::V1 { frame_index, .. } => *frame_index = target,
            Inner::V2 { frame_index, .. } => *frame_index = target as u16,
            Inner::Q0v { frame_index, .. } => *frame_index = target as u32,
        }
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
        let interactive_runtime = self.uses_game_input();
        if !self.playing && !interactive_runtime {
            return;
        }
        self.accumulator_sec += delta_sec.max(0.0);
        let frame_time = 1.0_f32 / self.fps() as f32;
        let mut transition_budget = MAX_FRAME_SCRIPT_TRANSITIONS;
        while self.accumulator_sec >= frame_time {
            self.accumulator_sec -= frame_time;

            // q0.input marks an interactive runtime. Its fixed-step game clock
            // is independent from timeline transport: pausing/end-of-timeline
            // freezes authored animation, not input/physics. The current frame's
            // script is therefore the game-tick script while that frame is held.
            if interactive_runtime {
                if self.playing && self.total_frames() > 1 {
                    self.advance_timeline_one_frame();
                }
                self.run_current_frame_scripts(&mut transition_budget);
                continue;
            }

            if self.advance_timeline_one_frame() {
                self.run_current_frame_scripts(&mut transition_budget);
                if !self.playing {
                    self.accumulator_sec = 0.0;
                    break;
                }
            } else if !self.playing {
                self.accumulator_sec = 0.0;
                break;
            }
        }
    }

    /// Advance authored timeline transport by one fixed frame. Returns true
    /// when a v2 frame was entered and its frame script should run.
    fn advance_timeline_one_frame(&mut self) -> bool {
        let total = self.total_frames();
        let mut hit_end = false;
        let mut entered_v2_frame = false;
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
                        entered_v2_frame = true;
                    } else {
                        *frame_index = total.saturating_sub(1) as u16;
                        hit_end = true;
                    }
                } else {
                    *frame_index = next;
                    entered_v2_frame = true;
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
        }
        entered_v2_frame
    }

    fn run_q0lang_source_with_budget(&mut self, source: &str, transition_budget: &mut usize) {
        self.sync_q0lang_host_globals();
        let report = match self.q0lang.as_mut() {
            Some(runtime) => runtime.execute_source(source),
            None => return,
        };
        self.initial_script_diagnostics.extend(report.diagnostics);
        self.apply_q0lang_scene_writes();
        self.sync_q0lang_host_globals();
        self.apply_q0lang_actions_with_budget(&report.actions, transition_budget);
    }

    fn run_current_frame_scripts(&mut self, transition_budget: &mut usize) {
        let (entry_q0rg_id, frame) = match &self.inner {
            Inner::V2 {
                entry_q0rg_id,
                frame_index,
                ..
            } => (*entry_q0rg_id, *frame_index),
            _ => return,
        };
        self.frame_script_entries = self.frame_script_entries.saturating_add(1);
        let mut scripts = self
            .project_v2()
            .into_iter()
            .flat_map(|project| project.runtime.frame_scripts.iter().enumerate())
            .filter(|(_, script)| script.q0rg_id == entry_q0rg_id && script.frame == frame)
            .map(|(insertion_order, script)| {
                (script.layer_id, insertion_order, script.source.clone())
            })
            .collect::<Vec<_>>();
        scripts.sort_by_key(|(layer_id, insertion_order, _)| (*layer_id, *insertion_order));
        for (_, _, source) in scripts {
            self.run_q0lang_source_with_budget(&source, transition_budget);
        }
        self.input.jump_pressed = false;
    }

    fn sync_q0lang_host_globals(&mut self) {
        let (stage_width, stage_height, owner, frame, names) = match &self.inner {
            Inner::V2 {
                project,
                entry_q0rg_id,
                frame_index,
                ..
            } => (
                f64::from(project.meta.stage_width),
                f64::from(project.meta.stage_height),
                *entry_q0rg_id,
                *frame_index,
                project
                    .runtime
                    .instance_names
                    .iter()
                    .filter(|(key, _)| key.q0rg_id == *entry_q0rg_id)
                    .map(|(key, name)| (*key, name.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => return,
        };
        let metrics = names
            .iter()
            .filter_map(|(key, name)| {
                self.project_v2().and_then(|project| {
                    q0s_format::raster::runtime_instance_metrics(
                        project,
                        owner,
                        frame,
                        key.instance_id,
                        &self.scene_runtime,
                    )
                    .map(|metrics| (name.clone(), metrics))
                })
            })
            .collect::<Vec<_>>();
        let fixed_dt = 1.0 / f64::from(self.fps().max(1));
        let Some(runtime) = self.q0lang.as_mut() else {
            return;
        };
        runtime
            .vars
            .insert("stage.width".into(), RuntimeValue::Number(stage_width));
        runtime
            .vars
            .insert("stage.height".into(), RuntimeValue::Number(stage_height));
        runtime
            .vars
            .insert("time.dt".into(), RuntimeValue::Number(fixed_dt));
        runtime
            .vars
            .insert("input.left".into(), RuntimeValue::Bool(self.input.left));
        runtime
            .vars
            .insert("input.right".into(), RuntimeValue::Bool(self.input.right));
        runtime
            .vars
            .insert("input.jump".into(), RuntimeValue::Bool(self.input.jump));
        runtime.vars.insert(
            "input.jump_pressed".into(),
            RuntimeValue::Bool(self.input.jump_pressed),
        );
        for (name, metrics) in metrics {
            for (suffix, value) in [
                ("x", metrics.x),
                ("y", metrics.y),
                ("left", metrics.left),
                ("right", metrics.right),
                ("top", metrics.top),
                ("bottom", metrics.bottom),
                ("width", metrics.width()),
                ("height", metrics.height()),
            ] {
                runtime.vars.insert(
                    format!("{name}.{suffix}"),
                    RuntimeValue::Number(f64::from(value)),
                );
            }
        }
    }

    fn apply_q0lang_scene_writes(&mut self) {
        let names = match &self.inner {
            Inner::V2 {
                project,
                entry_q0rg_id,
                ..
            } => project
                .runtime
                .instance_names
                .iter()
                .filter(|(key, _)| key.q0rg_id == *entry_q0rg_id)
                .map(|(key, name)| (*key, name.clone()))
                .collect::<Vec<_>>(),
            _ => return,
        };
        let Some(runtime) = self.q0lang.as_ref() else {
            return;
        };
        let writes = names
            .into_iter()
            .filter_map(|(key, name)| {
                let x = match runtime.vars.get(&format!("{name}.x")) {
                    Some(RuntimeValue::Number(value)) if value.is_finite() => *value as f32,
                    _ => return None,
                };
                let y = match runtime.vars.get(&format!("{name}.y")) {
                    Some(RuntimeValue::Number(value)) if value.is_finite() => *value as f32,
                    _ => return None,
                };
                Some((key, x, y))
            })
            .collect::<Vec<_>>();
        for (key, x, y) in writes {
            self.scene_runtime.set_position(key, x, y);
        }
    }

    fn runtime_transition_to_frame(&mut self, target_frame: usize, transition_budget: &mut usize) {
        if *transition_budget == 0 {
            self.initial_script_diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!(
                    "frame-script transition budget exceeded ({MAX_FRAME_SCRIPT_TRANSITIONS})"
                ),
            });
            return;
        }
        *transition_budget -= 1;
        let target = target_frame.min(self.total_frames().saturating_sub(1));
        self.set_frame_raw(target);
        self.accumulator_sec = 0.0;
        self.run_current_frame_scripts(transition_budget);
    }

    fn apply_q0lang_actions_with_budget(
        &mut self,
        actions: &[RuntimeAction],
        transition_budget: &mut usize,
    ) {
        for action in actions {
            match action {
                RuntimeAction::GoRun(target) => {
                    self.playing = true;
                    if let Some(frame) = self.resolve_timeline_target(target) {
                        self.runtime_transition_to_frame(frame, transition_budget);
                    }
                }
                RuntimeAction::GoStop(target) => {
                    self.playing = false;
                    if let Some(frame) = self.resolve_timeline_target(target) {
                        self.runtime_transition_to_frame(frame, transition_budget);
                    }
                }
                RuntimeAction::ShellCommand { .. } => {}
                RuntimeAction::SceneSwitch { alias } => {
                    self.switch_to_bundled_scene(alias, transition_budget);
                }
                RuntimeAction::RigSetPosition { control, x, y } => {
                    if let Some(owner) = self.current_entry_q0rg_id() {
                        self.apply_rig_position(owner, control, *x as f32, *y as f32);
                    }
                }
                RuntimeAction::RigSetValue { control, value } => {
                    if let Some(owner) = self.current_entry_q0rg_id() {
                        self.apply_rig_value(owner, control, *value as f32);
                    }
                }
                RuntimeAction::RigReset { control } => {
                    if let Some(owner) = self.current_entry_q0rg_id() {
                        self.reset_rig_control(owner, control);
                    }
                }
                RuntimeAction::RigSetPose { pose, weight } => {
                    if let Some(owner) = self.current_entry_q0rg_id() {
                        self.apply_rig_pose(owner, pose, *weight as f32);
                    }
                }
                RuntimeAction::RigResetPose { pose } => {
                    if let Some(owner) = self.current_entry_q0rg_id() {
                        self.reset_rig_pose(owner, pose);
                    }
                }
            }
        }
    }

    fn current_entry_q0rg_id(&self) -> Option<u16> {
        match &self.inner {
            Inner::V2 { entry_q0rg_id, .. } => Some(*entry_q0rg_id),
            _ => None,
        }
    }

    fn switch_to_bundled_scene(&mut self, alias: &str, transition_budget: &mut usize) {
        let Some(project) = self.scene_library.get(alias).cloned() else {
            self.initial_script_diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!("q0scene.switch! could not find bundled movie alias `{alias}`"),
            });
            return;
        };
        let entry_q0rg_id = project.meta.entry_q0rg_id;
        let Some(entry) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == entry_q0rg_id)
        else {
            self.initial_script_diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!("scene `{alias}` has no entry q0rg {entry_q0rg_id}"),
            });
            return;
        };
        let frame_count = entry.frame_count.max(1);
        let entry_script = entry.script.clone();
        let fps = project.meta.fps.max(1);
        let audio_timeline = build_project_audio_timeline(project.as_ref());

        self.inner = Inner::V2 {
            project,
            entry_q0rg_id,
            frame_count,
            fps,
            frame_index: 0,
        };
        self.scene_runtime = RuntimeSceneState::new();
        self.rig_runtime_overrides.clear();
        self.audio_timeline = audio_timeline;
        self.accumulator_sec = 0.0;
        self.active_scene_alias = Some(alias.to_string());
        self.scene_generation = self.scene_generation.wrapping_add(1);

        // Deliberately keep the root Runtime alive. In particular, do not
        // execute this scene's project-graph q0lang dependencies: bundled
        // movies are scene data, while the root q0lang runtime is the game
        // kernel shared across all scenes.
        self.sync_q0lang_host_globals();
        let entries_before_init = self.frame_script_entries;
        self.run_q0lang_source_with_budget(&entry_script, transition_budget);
        if self.frame_script_entries == entries_before_init {
            self.run_current_frame_scripts(transition_budget);
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
                let buf = q0s_format::raster::rasterize_q0rg_frame_with_runtime_overrides(
                    project,
                    *entry_q0rg_id,
                    *frame_index,
                    viewport_width,
                    viewport_height,
                    ss.max(1) as u32,
                    [0xFF, 0xFF, 0xFF, 0xFF],
                    &self.rig_runtime_overrides,
                    &self.scene_runtime,
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
    use super::{
        sample_audio_timeline, Player, PlayerFormat, PlayerLoadError, MAX_Q0S_FILE_BYTES,
        PLAYER_AUDIO_SAMPLE_RATE,
    };
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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

    fn q0s_v2_with_frame_scripts(frame_count: u16, scripts: &[(u16, u16, &str)]) -> Vec<u8> {
        q0s_v2_with_entry_and_frame_scripts("", frame_count, scripts)
    }

    fn q0s_v2_with_entry_and_frame_scripts(
        entry_script: &str,
        frame_count: u16,
        scripts: &[(u16, u16, &str)],
    ) -> Vec<u8> {
        use q0s_format::v2::{FrameScript, Layer, ProjectMeta, ProjectV2, Q0rg};

        let max_layer = scripts
            .iter()
            .map(|(_, layer, _)| *layer)
            .max()
            .unwrap_or(1);
        let layers = (1..=max_layer)
            .map(|layer_id| Layer {
                layer_id,
                name: format!("layer {layer_id}"),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            })
            .collect();
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "frame-script-test".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count,
                script: entry_script.to_string(),
                layers,
            }],
        };
        project.runtime.frame_scripts = scripts
            .iter()
            .map(|(frame, layer_id, source)| FrameScript {
                q0rg_id: 1,
                layer_id: *layer_id,
                frame: *frame,
                source: (*source).to_string(),
            })
            .collect();
        q0s_format::write_q0s_v2(&project).expect("write frame-script q0s")
    }

    fn runtime_var(player: &Player, name: &str) -> Option<String> {
        player
            .q0lang
            .as_ref()
            .and_then(|runtime| runtime.vars.get(name))
            .map(|value| value.display_lossy())
    }

    fn q0s_v2_game_fixture(module_source: &str) -> Vec<u8> {
        use q0s_format::v2::{
            Anchor, Asset, FrameScript, InstanceKey, Layer, Path, Placement, ProjectDependencyKind,
            ProjectDependencyNode, ProjectDependencySource, ProjectMeta, ProjectV2, Q0rg, Rgba,
            Target, Transform2D, Tween, Vec2, VectorAsset,
        };
        let square = Asset::Vector(VectorAsset {
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
                        point: Vec2::new(10.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(0.0, 10.0),
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
        });
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "embedded-game".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![square],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "Stage".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "player".into(),
                        explicit_keyframes: vec![0],
                        placements: vec![Placement {
                            instance_id: 11,
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D {
                                tx: 20.0,
                                ty: 70.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "PlayerSymbol".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 2,
                        name: "body".into(),
                        explicit_keyframes: vec![0],
                        placements: vec![Placement {
                            instance_id: 22,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
            ],
        };
        project
            .runtime
            .instance_names
            .insert(InstanceKey::new(1, 11), "player".to_string());
        project
            .runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "game".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::Embedded(module_source.as_bytes().to_vec()),
            });
        project.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 0,
            source: "vy = update(vy)\n".into(),
        });
        q0s_format::write_q0s_v2(&project).expect("write embedded game fixture")
    }

    const GAME_CONTROLLER: &str = r#"import q0.input
import q0.scene
import q0.time
speed = 240
gravity = 1440
jump_speed = -480
vy = 0
pb func update current_vy {
  dt = time.dt
  dx = 0
  if input.left and not input.right {
    dx = -speed * dt
  }
  if input.right and not input.left {
    dx = speed * dt
  }
  grounded = player.bottom >= stage.height - 0.5
  if input.jump_pressed and grounded {
    current_vy = jump_speed
  }
  current_vy = current_vy + gravity * dt
  dy = current_vy * dt
  if player.left + dx < 0 {
    dx = -player.left
  }
  if player.right + dx > stage.width {
    dx = stage.width - player.right
  }
  if player.top + dy < 0 {
    dy = -player.top
    if current_vy < 0 {
      current_vy = 0
    }
  }
  if player.bottom + dy >= stage.height {
    dy = stage.height - player.bottom
    if current_vy > 0 {
      current_vy = 0
    }
  }
  player.x = player.x + dx
  player.y = player.y + dy
  return current_vy
}
"#;

    fn q0s_v2_with_timeline_audio(muted: bool) -> Vec<u8> {
        use q0s_format::v2::{Asset, AudioClip, Layer, ProjectMeta, ProjectV2, Q0rg, Q0vAsset};

        let spec = Q0vSpec {
            width: 0,
            height: 0,
            fps: 48_000,
            timeline_frames: 2,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).expect("audio q0v writer");
        writer
            .write_audio_pcm_i16(&[16_384, -16_384, 8192, -8192])
            .expect("audio q0v pcm");
        let audio_bytes = writer.finish().expect("finish audio q0v").into_inner();
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "timeline-audio".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Q0v(Q0vAsset {
                asset_id: 77,
                bytes: audio_bytes,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: vec![AudioClip {
                q0rg_id: 1,
                layer_id: 1,
                start_frame: 1,
                asset_id: 77,
                gain: 0.5,
                muted,
            }],
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 4,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "audio".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: Vec::new(),
                }],
            }],
        };
        q0s_format::write_q0s_v2(&project).expect("write timeline audio q0s")
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
    fn q0s_timeline_audio_respects_clip_start_gain_and_seek() {
        let bytes = q0s_v2_with_timeline_audio(false);
        let mut player = Player::from_bytes(&bytes).expect("load q0s timeline audio");
        assert_eq!(player.format(), PlayerFormat::VectorV2);
        assert!(player.has_audio_timeline());

        let timeline = player.audio_timeline().expect("q0s audio timeline");
        let clip_start = u64::from(PLAYER_AUDIO_SAMPLE_RATE) / 24;
        assert_eq!(timeline.start_sample_frame, 0);
        assert_eq!(sample_audio_timeline(&timeline, clip_start - 1, 0), 0.0);
        assert!((sample_audio_timeline(&timeline, clip_start, 0) - 0.25).abs() < 0.001);
        assert!((sample_audio_timeline(&timeline, clip_start, 1) + 0.25).abs() < 0.001);
        assert!((sample_audio_timeline(&timeline, clip_start + 1, 0) - 0.125).abs() < 0.001);

        player.seek_to_frame(1);
        let seeked = player.audio_timeline().expect("seeked q0s audio timeline");
        assert_eq!(seeked.start_sample_frame, clip_start);
        assert!(
            (sample_audio_timeline(&seeked, seeked.start_sample_frame, 0) - 0.25).abs() < 0.001
        );
    }

    #[test]
    fn muted_q0s_timeline_audio_does_not_create_playback_source() {
        let bytes = q0s_v2_with_timeline_audio(true);
        let player = Player::from_bytes(&bytes).expect("load muted q0s timeline audio");
        assert!(!player.has_audio_timeline());
        assert!(player.audio_timeline().is_none());
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
    fn frame_zero_script_runs_once_after_runtime_init() {
        let bytes = q0s_v2_with_frame_scripts(3, &[(0, 1, "init = 7\n")]);
        let player = Player::from_bytes(&bytes).expect("load frame-script player");
        assert_eq!(player.current_frame(), 0);
        assert_eq!(runtime_var(&player, "init").as_deref(), Some("7"));
        assert_eq!(player.frame_script_entries, 1);
    }

    #[test]
    fn bundled_scene_switch_preserves_one_game_kernel_across_projects() {
        use q0s_format::v2::{
            FrameScript, Layer, ProjectDependencyKind, ProjectDependencyNode,
            ProjectDependencySource, ProjectMeta, ProjectV2, Q0rg,
        };

        let scene_layer = || Layer {
            layer_id: 1,
            name: "logic".into(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        };
        let mut child = ProjectV2 {
            meta: ProjectMeta {
                name: "room two".into(),
                fps: 24,
                stage_width: 80,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![scene_layer()],
            }],
        };
        child.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 0,
            source: "kernel = update(kernel)\n".into(),
        });
        // This would destroy the persistent state if a scene switch restarted
        // the child movie's q0lang graph as a second game kernel.
        child
            .runtime
            .project_graph
            .nodes
            .push(ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "wrong_kernel".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::Embedded(b"kernel = 0\n".to_vec()),
            });
        let child_bytes = q0s_format::write_q0s_v2(&child).expect("serialize child scene");

        let mut root = ProjectV2 {
            meta: ProjectMeta {
                name: "root game".into(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![scene_layer()],
            }],
        };
        root.runtime.project_graph.nodes.extend([
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "game".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::Embedded(
                    b"import q0.scene\nkernel = 40\npb func update x {\n  return x + 1\n}\n"
                        .to_vec(),
                ),
            },
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: None,
                alias: "room2".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::Embedded(child_bytes),
            },
        ]);
        root.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 0,
            source: "kernel = update(kernel)\nq0scene.switch! \"room2\"\n".into(),
        });

        let bytes = q0s_format::write_q0s_v2(&root).expect("serialize root game");
        let mut player = Player::from_bytes(&bytes).expect("load bundled game");
        assert_eq!(player.active_scene_alias(), Some("room2"));
        assert_eq!(player.scene_generation(), 1);
        assert_eq!(
            player.project_v2().expect("active v2").meta.name,
            "room two"
        );
        assert_eq!(
            player.q0lang.as_ref().expect("game kernel").vars["kernel"],
            q0s_format::q0lang::runtime::RuntimeValue::Number(42.0),
            "root frame increments to 41, child frame increments the SAME runtime to 42; child kernel=0 must not run"
        );

        player.tick(1.0 / 24.0);
        assert_eq!(
            player.q0lang.as_ref().expect("game kernel").vars["kernel"],
            q0s_format::q0lang::runtime::RuntimeValue::Number(43.0),
            "the child scene keeps calling the function defined only by the root kernel"
        );
    }

    #[test]
    fn entry_function_bytecode_is_callable_from_later_frame_script() {
        let bytes = q0s_v2_with_entry_and_frame_scripts(
            "pb func plus_one x {\n  return x + 1\n}\n",
            3,
            &[(1, 1, "result = plus_one(9)\n")],
        );
        let mut player = Player::from_bytes(&bytes).expect("load qvm function player");
        assert!(runtime_var(&player, "result").is_none());
        player.tick(1.0 / 24.0);
        assert_eq!(player.current_frame(), 1);
        assert_eq!(runtime_var(&player, "result").as_deref(), Some("10"));
        assert!(player.initial_script_diagnostics().is_empty());
    }

    #[test]
    fn interactive_one_frame_runtime_ticks_without_play_or_loop() {
        let bytes = q0s_v2_game_fixture(GAME_CONTROLLER);
        let mut player = Player::from_bytes(&bytes).expect("load embedded game");
        player.set_loop_enabled(false);
        player.set_playing(false);
        let before = player
            .runtime_instance_metrics("player")
            .expect("before metrics");

        player.set_input_state(false, true, false);
        player.tick(1.0 / 24.0);
        let after = player
            .runtime_instance_metrics("player")
            .expect("after metrics");

        assert_eq!(
            player.current_frame(),
            0,
            "game clock must not invent timeline frames"
        );
        assert!(!player.is_playing(), "timeline transport stays paused");
        assert!(
            after.x > before.x + 1.0,
            "input must run while transport is paused: before={before:?} after={after:?}"
        );
    }

    #[test]
    fn interactive_one_frame_runtime_does_not_stop_at_timeline_end() {
        let bytes = q0s_v2_game_fixture(GAME_CONTROLLER);
        let mut player = Player::from_bytes(&bytes).expect("load embedded game");
        player.set_loop_enabled(false);
        player.set_playing(true);
        player.set_input_state(false, true, false);

        player.tick(3.0 / 24.0);

        assert!(
            player.is_playing(),
            "one-frame game has no authored frame to advance past"
        );
        assert_eq!(player.current_frame(), 0);
        assert!(
            player
                .runtime_instance_metrics("player")
                .expect("metrics")
                .x
                > 20.0,
            "game runtime must keep ticking on frame zero"
        );
    }

    #[test]
    fn embedded_q0lang_game_moves_jumps_and_clamps_named_instance_to_stage() {
        let bytes = q0s_v2_game_fixture(GAME_CONTROLLER);
        let mut player = Player::from_bytes(&bytes).expect("load embedded game");
        assert!(player.uses_game_input());
        assert!(
            player.initial_script_diagnostics().is_empty(),
            "{:?}",
            player.initial_script_diagnostics()
        );
        let authored_x = player.project_v2().unwrap().q0rgs[0].layers[0].placements[0]
            .transform
            .tx;
        assert_eq!(authored_x, 20.0);

        // Gravity settles the symbol exactly on the stage floor.
        player.set_input_state(false, false, false);
        player.tick(20.0 / 24.0);
        let floor = player
            .runtime_instance_metrics("player")
            .expect("floor metrics");
        assert!((floor.bottom - 100.0).abs() < 0.01, "{floor:?}");

        // Right and left movement clamp the visible body, not merely its transform origin.
        player.set_input_state(false, true, false);
        player.tick(40.0 / 24.0);
        let right = player
            .runtime_instance_metrics("player")
            .expect("right wall metrics");
        assert!((right.right - 100.0).abs() < 0.01, "{right:?}");
        assert!(right.left >= -0.01, "{right:?}");

        let mut right_pixels = vec![0; 100 * 100 * 4];
        player.render_with_quality(&mut right_pixels, 100, 100, 1);

        player.set_input_state(true, false, false);
        player.tick(40.0 / 24.0);
        let left = player
            .runtime_instance_metrics("player")
            .expect("left wall metrics");
        assert!(left.left.abs() < 0.01, "{left:?}");
        assert!(left.right <= 100.01, "{left:?}");

        let mut left_pixels = vec![0; 100 * 100 * 4];
        player.render_with_quality(&mut left_pixels, 100, 100, 1);
        assert_ne!(
            right_pixels, left_pixels,
            "runtime transform must reach raster output"
        );

        // One rising jump edge launches upward; holding jump does not retrigger in mid-air.
        player.set_input_state(false, false, false);
        player.tick(1.0 / 24.0);
        let grounded = player
            .runtime_instance_metrics("player")
            .expect("grounded metrics");
        assert!((grounded.bottom - 100.0).abs() < 0.01);
        player.set_input_state(false, false, true);
        player.tick(1.0 / 24.0);
        let airborne = player
            .runtime_instance_metrics("player")
            .expect("airborne metrics");
        assert!(airborne.bottom < grounded.bottom - 1.0, "{airborne:?}");
        player.tick(30.0 / 24.0);
        let landed = player
            .runtime_instance_metrics("player")
            .expect("landed metrics");
        assert!((landed.bottom - 100.0).abs() < 0.01, "{landed:?}");

        // Runtime motion is an overlay: the authored movie is untouched.
        assert_eq!(
            player.project_v2().unwrap().q0rgs[0].layers[0].placements[0]
                .transform
                .tx,
            authored_x
        );
    }

    #[test]
    fn frame_script_fires_when_playback_enters_frame() {
        let bytes = q0s_v2_with_frame_scripts(3, &[(1, 1, "entered = 1\n")]);
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        assert!(runtime_var(&player, "entered").is_none());
        player.tick(1.0 / 24.0);
        assert_eq!(player.current_frame(), 1);
        assert_eq!(runtime_var(&player, "entered").as_deref(), Some("1"));
    }

    #[test]
    fn loop_reenters_frame_zero_and_refires_its_script() {
        let bytes = q0s_v2_with_frame_scripts(2, &[(0, 1, "broken signal\n")]);
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        let initial = player.initial_script_diagnostics().len();
        assert!(
            initial > 0,
            "frame zero diagnostic proves initial execution"
        );
        player.tick(2.0 / 24.0);
        assert_eq!(player.current_frame(), 0);
        assert!(
            player.initial_script_diagnostics().len() > initial,
            "loop entry must execute frame zero again"
        );
    }

    #[test]
    fn runtime_goto_runs_destination_but_skips_intermediate_frame_scripts() {
        let bytes = q0s_v2_with_frame_scripts(
            5,
            &[
                (1, 1, "gorun! 3\n"),
                (2, 1, "middle = 1\n"),
                (3, 1, "destination = 1\n"),
            ],
        );
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        player.seek_to_frame(1);
        assert_eq!(player.current_frame(), 3);
        assert!(runtime_var(&player, "middle").is_none());
        assert_eq!(runtime_var(&player, "destination").as_deref(), Some("1"));
    }

    #[test]
    fn backward_runtime_goto_runs_destination_frame_script() {
        let bytes = q0s_v2_with_frame_scripts(5, &[(1, 1, "back = 1\n"), (3, 1, "gostop! 1\n")]);
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        player.seek_to_frame(3);
        assert_eq!(player.current_frame(), 1);
        assert_eq!(runtime_var(&player, "back").as_deref(), Some("1"));
        assert!(!player.is_playing());
    }

    #[test]
    fn same_frame_scripts_run_in_deterministic_layer_order() {
        let bytes = q0s_v2_with_frame_scripts(3, &[(1, 2, "order = 2\n"), (1, 1, "order = 1\n")]);
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        player.seek_to_frame(1);
        assert_eq!(runtime_var(&player, "order").as_deref(), Some("2"));
    }

    #[test]
    fn self_jump_is_stopped_by_frame_transition_budget() {
        let bytes = q0s_v2_with_frame_scripts(3, &[(1, 1, "gorun! 1\n")]);
        let mut player = Player::from_bytes(&bytes).expect("load frame-script player");
        player.seek_to_frame(1);
        assert_eq!(player.current_frame(), 1);
        assert!(player
            .initial_script_diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("transition budget exceeded") }));
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
