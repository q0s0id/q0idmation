use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use image::{DynamicImage, ImageBuffer, ImageOutputFormat, RgbaImage};
use q0s_format::v2::ProjectV2;

const MAX_RENDER_ALLOCATION_BYTES: u64 = 512 * 1024 * 1024;
pub const MEDIA_TICKS_PER_SECOND: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Q0s,
    Mp4,
    Q0v,
    PngSequence,
    Gif,
    Image,
}

impl ExportFormat {
    pub const ALL: [Self; 6] = [
        Self::Q0s,
        Self::Mp4,
        Self::Q0v,
        Self::PngSequence,
        Self::Gif,
        Self::Image,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Q0s => "q0s",
            Self::Mp4 => "mp4",
            Self::Q0v => "q0v",
            Self::PngSequence => "png sequence",
            Self::Gif => "gif",
            Self::Image => "image",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Q0s => "q0s",
            Self::Mp4 => "mp4",
            Self::Q0v => "q0v",
            Self::PngSequence => "",
            Self::Gif => "gif",
            Self::Image => "png",
        }
    }

    pub const fn implemented(self) -> bool {
        true
    }

    pub const fn carries_video(self) -> bool {
        matches!(self, Self::Mp4 | Self::Q0v | Self::Gif)
    }

    pub const fn carries_audio(self) -> bool {
        matches!(self, Self::Mp4 | Self::Q0v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageCodec {
    Png,
    Jpeg,
    WebP,
}

impl ImageCodec {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::WebP];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    pub const fn supports_alpha(self) -> bool {
        !matches!(self, Self::Jpeg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Q0vStreams {
    pub video: bool,
    pub audio: bool,
}

impl Default for Q0vStreams {
    fn default() -> Self {
        Self {
            video: true,
            audio: true,
        }
    }
}

/// One rational media clock shared by q0v video and its optional audio stream.
/// No float time accumulation is allowed: frame and sample boundaries are
/// always derived from integer indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaClock {
    pub fps: u32,
    pub audio_sample_rate: u32,
}

impl MediaClock {
    pub fn validate(self) -> Result<(), String> {
        if self.fps == 0 {
            return Err("media clock fps must be greater than zero".to_string());
        }
        if self.audio_sample_rate == 0 {
            return Err("audio sample rate must be greater than zero".to_string());
        }
        Ok(())
    }

    pub fn video_frame_ticks(self, frame_index: u64) -> u64 {
        frame_index.saturating_mul(MEDIA_TICKS_PER_SECOND) / u64::from(self.fps.max(1))
    }

    pub fn audio_sample_ticks(self, sample_index: u64) -> u64 {
        sample_index.saturating_mul(MEDIA_TICKS_PER_SECOND)
            / u64::from(self.audio_sample_rate.max(1))
    }

    pub fn audio_samples_for_video_frame(self, frame_index: u64) -> Range<u64> {
        let rate = u64::from(self.audio_sample_rate.max(1));
        let fps = u64::from(self.fps.max(1));
        let start = frame_index.saturating_mul(rate) / fps;
        let end_exclusive = frame_index.saturating_add(1).saturating_mul(rate) / fps;
        start..end_exclusive
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub source_q0rg_id: u16,
    pub entire_timeline: bool,
    pub first_frame: u16,
    pub last_frame: u16,
    pub width: u32,
    pub height: u32,
    pub supersampling: u8,
    pub transparent: bool,
    pub image_codec: ImageCodec,
    pub jpeg_quality: u8,
    pub sequence_prefix: String,
    pub sequence_create_subfolder: bool,
    pub gif_loop: bool,
    pub gif_speed: u8,
    pub mp4_bitrate: u32,
    pub q0v_streams: Q0vStreams,
}

impl ExportOptions {
    pub fn validate(&self, project: &ProjectV2) -> Result<(), String> {
        if !project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == self.source_q0rg_id)
        {
            return Err("export source q0rg is missing".to_string());
        }
        if self.format != ExportFormat::Q0s {
            if self.width == 0 || self.height == 0 {
                return Err("export size must be greater than zero".to_string());
            }
            if !matches!(self.supersampling, 1 | 2 | 4) {
                return Err("supersampling must be 1x, 2x, or 4x".to_string());
            }
            let render_bytes = u64::from(self.width)
                .checked_mul(u64::from(self.height))
                .and_then(|pixels| pixels.checked_mul(u64::from(self.supersampling).pow(2)))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| "render size overflows the allocation limit".to_string())?;
            if render_bytes > MAX_RENDER_ALLOCATION_BYTES {
                return Err(format!(
                    "render buffer would use more than {} mb",
                    MAX_RENDER_ALLOCATION_BYTES / (1024 * 1024)
                ));
            }
        }
        if self.first_frame > self.last_frame {
            return Err("first frame must not be after last frame".to_string());
        }
        let source_last = source_last_frame(project, self.source_q0rg_id);
        if !self.entire_timeline && self.last_frame > source_last {
            return Err("export frame range exceeds the source timeline".to_string());
        }
        if self.format == ExportFormat::Q0v && !self.q0v_streams.video && !self.q0v_streams.audio {
            return Err("q0v must contain video, audio, or both".to_string());
        }
        if self.format == ExportFormat::Image
            && self.transparent
            && !self.image_codec.supports_alpha()
        {
            return Err("jpeg does not support transparency".to_string());
        }
        if !(1..=100).contains(&self.jpeg_quality) {
            return Err("jpeg quality must be from 1 to 100".to_string());
        }
        if self.format == ExportFormat::PngSequence {
            validate_sequence_prefix(&self.sequence_prefix)?;
        }
        if self.format == ExportFormat::Gif {
            if self.width > u16::MAX as u32 || self.height > u16::MAX as u32 {
                return Err("gif dimensions must fit into 65535 pixels".to_string());
            }
            if !(1..=30).contains(&self.gif_speed) {
                return Err("gif encoder speed must be from 1 to 30".to_string());
            }
        }
        if self.format == ExportFormat::Mp4 && self.mp4_bitrate < 64_000 {
            return Err("mp4 bitrate is too low".to_string());
        }
        Ok(())
    }

    pub fn frame_range(&self, project: &ProjectV2) -> Result<RangeInclusive<u16>, String> {
        self.validate(project)?;
        if self.format == ExportFormat::Image {
            return Ok(self.first_frame..=self.first_frame);
        }
        if self.entire_timeline {
            Ok(0..=source_last_frame(project, self.source_q0rg_id))
        } else {
            Ok(self.first_frame..=self.last_frame)
        }
    }

    pub fn total_frames(&self, project: &ProjectV2) -> Result<u32, String> {
        let range = self.frame_range(project)?;
        Ok(u32::from(*range.end() - *range.start()) + 1)
    }

    pub fn media_clock(&self, project: &ProjectV2) -> MediaClock {
        MediaClock {
            fps: u32::from(project.meta.fps.max(1)),
            audio_sample_rate: 48_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Completed,
    Cancelled,
    Failed(String),
}

impl JobState {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }

    pub const fn is_finished(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed(_))
    }
}

#[derive(Debug, Clone)]
pub struct ExportJob {
    pub id: u64,
    pub output_path: PathBuf,
    pub options: ExportOptions,
    pub state: JobState,
    pub completed_units: u32,
    pub total_units: u32,
    pub phase: String,
    snapshot: ProjectV2,
}

impl ExportJob {
    pub fn snapshot(&self) -> &ProjectV2 {
        &self.snapshot
    }

    pub fn progress(&self) -> f32 {
        if self.total_units == 0 {
            return 0.0;
        }
        self.completed_units.min(self.total_units) as f32 / self.total_units as f32
    }
}

enum WorkerOutcome {
    Completed,
    Cancelled,
}

enum WorkerEvent {
    Progress {
        id: u64,
        completed: u32,
        total: u32,
        phase: String,
    },
    Finished {
        id: u64,
        result: Result<WorkerOutcome, String>,
    },
}

pub struct Q0EncState {
    pub open: bool,
    pub selected_format: ExportFormat,
    pub source_q0rg_id: u16,
    pub entire_timeline: bool,
    pub first_frame: u16,
    pub last_frame: u16,
    pub width: u32,
    pub height: u32,
    pub supersampling: u8,
    pub transparent: bool,
    pub image_codec: ImageCodec,
    pub jpeg_quality: u8,
    pub sequence_prefix: String,
    pub sequence_create_subfolder: bool,
    pub sequence_subfolder: String,
    pub gif_loop: bool,
    pub gif_speed: u8,
    pub mp4_bitrate: u32,
    pub q0v_streams: Q0vStreams,
    pub output_text: String,
    pub last_export_directory: Option<PathBuf>,
    pub queue: Vec<ExportJob>,
    pub queue_running: bool,
    next_job_id: u64,
    active_job_id: Option<u64>,
    active_cancel: Option<Arc<AtomicBool>>,
    event_tx: mpsc::Sender<WorkerEvent>,
    event_rx: mpsc::Receiver<WorkerEvent>,
}

impl Default for Q0EncState {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            open: false,
            selected_format: ExportFormat::Q0s,
            source_q0rg_id: 1,
            entire_timeline: true,
            first_frame: 0,
            last_frame: 0,
            width: u32::from(crate::state::DEFAULT_STAGE_WIDTH),
            height: u32::from(crate::state::DEFAULT_STAGE_HEIGHT),
            supersampling: 2,
            transparent: false,
            image_codec: ImageCodec::Png,
            jpeg_quality: 90,
            sequence_prefix: "frame".to_string(),
            sequence_create_subfolder: true,
            sequence_subfolder: "movie-frames".to_string(),
            gif_loop: true,
            gif_speed: 10,
            mp4_bitrate: 8_000_000,
            q0v_streams: Q0vStreams::default(),
            output_text: "movie.q0s".to_string(),
            last_export_directory: None,
            queue: Vec::new(),
            queue_running: false,
            next_job_id: 1,
            active_job_id: None,
            active_cancel: None,
            event_tx,
            event_rx,
        }
    }
}

impl Q0EncState {
    pub fn apply_persisted_preferences(
        &mut self,
        last_export_directory: Option<PathBuf>,
        sequence_create_subfolder: bool,
    ) {
        self.last_export_directory = last_export_directory;
        self.sequence_create_subfolder = sequence_create_subfolder;
    }

    pub fn remember_directory(&mut self, directory: PathBuf) {
        self.last_export_directory = Some(directory);
    }

    pub fn open_for_project(&mut self, project: &ProjectV2, file_path: Option<&Path>) {
        self.open = true;
        self.source_q0rg_id = project.meta.entry_q0rg_id;
        self.width = u32::from(project.meta.stage_width);
        self.height = u32::from(project.meta.stage_height);
        self.first_frame = 0;
        self.last_frame = source_last_frame(project, self.source_q0rg_id);
        self.sequence_subfolder = format!("{}-frames", project_stem(file_path));
        self.refresh_output_for_format(file_path);
    }

    pub fn toggle_for_project(&mut self, project: &ProjectV2, file_path: Option<&Path>) {
        if self.open {
            self.open = false;
        } else {
            self.open_for_project(project, file_path);
        }
    }

    pub fn select_format(&mut self, format: ExportFormat, file_path: Option<&Path>) {
        if self.selected_format == format {
            return;
        }
        let previous = self.selected_format;
        self.selected_format = format;
        self.refresh_output_for_format(file_path);
        if format == ExportFormat::Image {
            self.entire_timeline = false;
            self.last_frame = self.first_frame;
        } else if previous == ExportFormat::Image {
            self.entire_timeline = true;
        }
    }

    fn refresh_output_for_format(&mut self, file_path: Option<&Path>) {
        let directory = self
            .last_export_directory
            .clone()
            .or_else(|| file_path.and_then(Path::parent).map(Path::to_path_buf))
            .unwrap_or_default();
        if self.selected_format == ExportFormat::PngSequence {
            self.output_text = directory.display().to_string();
            return;
        }
        let suggested = suggested_output_path(file_path, self.selected_format, self.image_codec);
        let name = suggested
            .file_name()
            .map(|name| name.to_owned())
            .unwrap_or_else(|| "movie".into());
        self.output_text = directory.join(name).display().to_string();
    }

    pub fn select_image_codec(&mut self, codec: ImageCodec) {
        self.image_codec = codec;
        if !codec.supports_alpha() {
            self.transparent = false;
        }
        let mut output = PathBuf::from(self.output_text.trim());
        output.set_extension(codec.extension());
        self.output_text = output.display().to_string();
    }

    pub fn reconcile_with_project(&mut self, project: &ProjectV2) {
        if !project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == self.source_q0rg_id)
        {
            self.source_q0rg_id = project.meta.entry_q0rg_id;
        }
        let last = source_last_frame(project, self.source_q0rg_id);
        self.first_frame = self.first_frame.min(last);
        self.last_frame = self.last_frame.min(last).max(self.first_frame);
        if self.selected_format == ExportFormat::Image {
            self.last_frame = self.first_frame;
        }
        self.width = self.width.max(1);
        self.height = self.height.max(1);
        self.supersampling = match self.supersampling {
            1 | 2 | 4 => self.supersampling,
            _ => 2,
        };
        self.jpeg_quality = self.jpeg_quality.clamp(1, 100);
        self.gif_speed = self.gif_speed.clamp(1, 30);
        self.mp4_bitrate = self.mp4_bitrate.max(64_000);
    }

    pub fn current_options(&self) -> ExportOptions {
        ExportOptions {
            format: self.selected_format,
            source_q0rg_id: self.source_q0rg_id,
            entire_timeline: self.entire_timeline,
            first_frame: self.first_frame,
            last_frame: self.last_frame,
            width: self.width,
            height: self.height,
            supersampling: self.supersampling,
            transparent: self.transparent,
            image_codec: self.image_codec,
            jpeg_quality: self.jpeg_quality,
            sequence_prefix: self.sequence_prefix.clone(),
            sequence_create_subfolder: self.sequence_create_subfolder,
            gif_loop: self.gif_loop,
            gif_speed: self.gif_speed,
            mp4_bitrate: self.mp4_bitrate,
            q0v_streams: self.q0v_streams,
        }
    }

    pub fn current_validation(&self, project: &ProjectV2) -> Result<(), String> {
        if !self.selected_format.implemented() {
            return Err(format!(
                "{} encoder belongs to the final q0enc phase",
                self.selected_format.label()
            ));
        }
        self.current_options().validate(project)?;
        if self.output_text.trim().is_empty() {
            return Err("choose an output path".to_string());
        }
        if self.selected_format == ExportFormat::PngSequence && self.sequence_create_subfolder {
            validate_sequence_subfolder(&self.sequence_subfolder)?;
        }
        Ok(())
    }

    pub fn enqueue(&mut self, project: &ProjectV2) -> Result<u64, String> {
        self.current_validation(project)?;
        let mut output_path = PathBuf::from(self.output_text.trim());
        if self.selected_format == ExportFormat::PngSequence {
            if self.sequence_create_subfolder {
                output_path = output_path.join(self.sequence_subfolder.trim());
            }
        } else {
            let expected_extension = match self.selected_format {
                ExportFormat::Image => self.image_codec.extension(),
                other => other.extension(),
            };
            if !expected_extension.is_empty() {
                output_path.set_extension(expected_extension);
            }
        }
        let options = self.current_options();
        let total_units = if options.format == ExportFormat::Q0s {
            1
        } else {
            options.total_frames(project)?
        };
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.queue.push(ExportJob {
            id,
            output_path,
            options,
            state: JobState::Pending,
            completed_units: 0,
            total_units,
            phase: "waiting".to_string(),
            snapshot: project.clone(),
        });
        Ok(id)
    }

    pub fn start_queue(&mut self) {
        if self
            .queue
            .iter()
            .any(|job| matches!(job.state, JobState::Pending))
        {
            self.queue_running = true;
            self.spawn_next_if_idle();
        }
    }

    pub fn has_active_job(&self) -> bool {
        self.active_job_id.is_some()
    }

    pub fn cancel_job(&mut self, id: u64) {
        if let Some(job) = self.queue.iter_mut().find(|job| job.id == id) {
            match job.state {
                JobState::Pending => {
                    job.state = JobState::Cancelled;
                    job.phase = "cancelled before start".to_string();
                }
                JobState::Running if self.active_job_id == Some(id) => {
                    if let Some(cancel) = &self.active_cancel {
                        cancel.store(true, Ordering::Release);
                        job.phase = "cancelling".to_string();
                    }
                }
                _ => {}
            }
        }
        if !self
            .queue
            .iter()
            .any(|job| matches!(job.state, JobState::Pending | JobState::Running))
        {
            self.queue_running = false;
        }
    }

    pub fn clear_finished(&mut self) {
        self.queue.retain(|job| !job.state.is_finished());
    }

    pub fn poll_events(&mut self) -> Vec<String> {
        let events: Vec<WorkerEvent> = self.event_rx.try_iter().collect();
        let mut statuses = Vec::new();
        for event in events {
            match event {
                WorkerEvent::Progress {
                    id,
                    completed,
                    total,
                    phase,
                } => {
                    if let Some(job) = self.queue.iter_mut().find(|job| job.id == id) {
                        job.completed_units = completed.min(total);
                        job.total_units = total.max(1);
                        job.phase = phase;
                    }
                }
                WorkerEvent::Finished { id, result } => {
                    let Some(job) = self.queue.iter_mut().find(|job| job.id == id) else {
                        continue;
                    };
                    let format = job.options.format;
                    let path = job.output_path.clone();
                    match result {
                        Ok(WorkerOutcome::Completed) => {
                            job.state = JobState::Completed;
                            job.completed_units = job.total_units;
                            job.phase = "completed".to_string();
                            statuses.push(format!(
                                "q0enc exported {}: {}",
                                format.label(),
                                path.display()
                            ));
                        }
                        Ok(WorkerOutcome::Cancelled) => {
                            job.state = JobState::Cancelled;
                            job.phase = "cancelled".to_string();
                            statuses.push(format!("q0enc cancelled {} job #{id}", format.label()));
                        }
                        Err(error) => {
                            job.state = JobState::Failed(error.clone());
                            job.phase = "failed".to_string();
                            statuses
                                .push(format!("q0enc {} export failed: {error}", format.label()));
                        }
                    }
                    self.active_job_id = None;
                    self.active_cancel = None;
                }
            }
        }

        if self.queue_running {
            self.spawn_next_if_idle();
        }
        if self.active_job_id.is_none()
            && !self
                .queue
                .iter()
                .any(|job| matches!(job.state, JobState::Pending))
        {
            self.queue_running = false;
        }
        statuses
    }

    fn spawn_next_if_idle(&mut self) {
        if self.active_job_id.is_some() || !self.queue_running {
            return;
        }
        let Some(index) = self
            .queue
            .iter()
            .position(|job| matches!(job.state, JobState::Pending))
        else {
            self.queue_running = false;
            return;
        };

        self.queue[index].state = JobState::Running;
        self.queue[index].phase = "preparing".to_string();
        let job = self.queue[index].clone();
        let id = job.id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.active_job_id = Some(id);
        self.active_cancel = Some(cancel.clone());
        let sender = self.event_tx.clone();
        std::thread::spawn(move || {
            let progress_sender = sender.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_job(&job, &cancel, |completed, total, phase| {
                    let _ = progress_sender.send(WorkerEvent::Progress {
                        id,
                        completed,
                        total,
                        phase: phase.to_string(),
                    });
                })
            }))
            .unwrap_or_else(|_| Err("encoder worker panicked".to_string()));
            let _ = sender.send(WorkerEvent::Finished { id, result });
        });
    }
}

fn execute_job(
    job: &ExportJob,
    cancel: &AtomicBool,
    progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    match job.options.format {
        ExportFormat::Q0s => export_q0s(job, cancel, progress),
        ExportFormat::Mp4 => export_mp4(job, cancel, progress),
        ExportFormat::Q0v => export_q0v(job, cancel, progress),
        ExportFormat::PngSequence => export_png_sequence(job, cancel, progress),
        ExportFormat::Gif => export_gif(job, cancel, progress),
        ExportFormat::Image => export_image(job, cancel, progress),
    }
}

fn export_q0s(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(WorkerOutcome::Cancelled);
    }
    progress(0, 1, "validating runtime build");
    let bytes = crate::export::export_to_q0s_bytes(job.snapshot())
        .map_err(|error| format!("serialize: {error}"))?;
    let parsed = q0s_format::parse_q0s_v2(&bytes)
        .map_err(|error| format!("parse-back before write: {error}"))?;
    if parsed != *job.snapshot() {
        return Err("parse-back before write changed the project".to_string());
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(WorkerOutcome::Cancelled);
    }
    crate::file_io::write_bytes_atomic(&job.output_path, &bytes)
        .map_err(|error| format!("write: {error}"))?;
    let reread = std::fs::read(&job.output_path).map_err(|error| format!("reread: {error}"))?;
    let reparsed = q0s_format::parse_q0s_v2(&reread)
        .map_err(|error| format!("parse-back after write: {error}"))?;
    if reparsed != *job.snapshot() {
        return Err("written q0s does not match the export snapshot".to_string());
    }
    progress(1, 1, "runtime build verified");
    Ok(WorkerOutcome::Completed)
}

fn export_image(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(WorkerOutcome::Cancelled);
    }
    progress(0, 1, "rendering frame");
    let frame = *job.options.frame_range(job.snapshot())?.start();
    let rgba = render_frame(job.snapshot(), &job.options, frame);
    if cancel.load(Ordering::Acquire) {
        return Ok(WorkerOutcome::Cancelled);
    }
    progress(0, 1, "encoding image");
    let bytes = encode_image(
        &rgba,
        job.options.width,
        job.options.height,
        job.options.image_codec,
        job.options.jpeg_quality,
    )?;
    verify_encoded_image(&bytes, job.options.width, job.options.height)?;
    if cancel.load(Ordering::Acquire) {
        return Ok(WorkerOutcome::Cancelled);
    }
    crate::file_io::write_bytes_atomic(&job.output_path, &bytes)
        .map_err(|error| format!("write image: {error}"))?;
    progress(1, 1, "image verified");
    Ok(WorkerOutcome::Completed)
}

fn export_gif(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    let range = job.options.frame_range(job.snapshot())?;
    let total = job.options.total_frames(job.snapshot())?;
    ensure_parent(&job.output_path)?;
    let (staging, mut file) = crate::file_io::create_staging_file(&job.output_path)
        .map_err(|error| format!("create gif staging file: {error}"))?;
    let result = (|| {
        let width =
            u16::try_from(job.options.width).map_err(|_| "gif width exceeds 65535".to_string())?;
        let height = u16::try_from(job.options.height)
            .map_err(|_| "gif height exceeds 65535".to_string())?;
        {
            let mut encoder = gif::Encoder::new(&mut file, width, height, &[])
                .map_err(|error| format!("create gif encoder: {error}"))?;
            if job.options.gif_loop {
                encoder
                    .set_repeat(gif::Repeat::Infinite)
                    .map_err(|error| format!("set gif loop: {error}"))?;
            }
            let fps = u32::from(job.snapshot().meta.fps.max(1));
            for (offset, frame_number) in range.enumerate() {
                if cancel.load(Ordering::Acquire) {
                    return Ok(WorkerOutcome::Cancelled);
                }
                progress(offset as u32, total, "quantizing gif frames");
                let mut rgba = render_frame(job.snapshot(), &job.options, frame_number);
                let mut frame = gif::Frame::from_rgba_speed(
                    width,
                    height,
                    &mut rgba,
                    i32::from(job.options.gif_speed),
                );
                frame.delay = gif_delay_centiseconds(offset as u64, fps);
                encoder
                    .write_frame(&frame)
                    .map_err(|error| format!("write gif frame: {error}"))?;
                progress(offset as u32 + 1, total, "writing gif frames");
            }
        }
        file.flush()
            .map_err(|error| format!("flush gif staging file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync gif staging file: {error}"))?;
        drop(file);
        verify_gif(&staging, job.options.width, job.options.height, total)?;
        if cancel.load(Ordering::Acquire) {
            return Ok(WorkerOutcome::Cancelled);
        }
        crate::file_io::replace_staged_file(&staging, &job.output_path)
            .map_err(|error| format!("publish gif: {error}"))?;
        Ok(WorkerOutcome::Completed)
    })();
    if !matches!(result, Ok(WorkerOutcome::Completed)) {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

fn export_q0v(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    let range = job.options.frame_range(job.snapshot())?;
    let total = job.options.total_frames(job.snapshot())?;
    ensure_parent(&job.output_path)?;
    let (staging, file) = crate::file_io::create_staging_file(&job.output_path)
        .map_err(|error| format!("create q0v staging file: {error}"))?;
    let result = (|| {
        let spec = q0video::q0v::Q0vSpec {
            width: if job.options.q0v_streams.video {
                job.options.width
            } else {
                0
            },
            height: if job.options.q0v_streams.video {
                job.options.height
            } else {
                0
            },
            fps: u32::from(job.snapshot().meta.fps.max(1)),
            timeline_frames: total,
            video: job.options.q0v_streams.video,
            audio: job.options.q0v_streams.audio,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(file, spec)?;
        if spec.video {
            for (offset, frame_number) in range.clone().enumerate() {
                if cancel.load(Ordering::Acquire) {
                    return Ok(WorkerOutcome::Cancelled);
                }
                progress(offset as u32, total, "encoding q0v video frames");
                let rgba = render_frame(job.snapshot(), &job.options, frame_number);
                let png = encode_image(
                    &rgba,
                    job.options.width,
                    job.options.height,
                    ImageCodec::Png,
                    100,
                )?;
                writer.write_video_frame(
                    (offset as u64).saturating_mul(MEDIA_TICKS_PER_SECOND) / u64::from(spec.fps),
                    &png,
                )?;
                progress(offset as u32 + 1, total, "writing q0v video frames");
            }
        }
        if spec.audio {
            if cancel.load(Ordering::Acquire) {
                return Ok(WorkerOutcome::Cancelled);
            }
            progress(
                if spec.video { total } else { 0 },
                total,
                "writing q0v pcm audio",
            );
            // q0s/q1s does not have audio timeline assets yet. The container
            // is fully audio-capable now; until those assets land, an enabled
            // stream is explicit, duration-correct silence rather than fake
            // data masquerading as mixed timeline sound.
            writer.write_silence(spec.expected_audio_samples_per_channel())?;
        }
        let mut file = writer.finish()?;
        file.flush()
            .map_err(|error| format!("flush q0v staging file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync q0v staging file: {error}"))?;
        drop(file);
        let parsed = q0video::q0v::Q0vFile::parse(
            std::fs::read(&staging).map_err(|error| format!("reread q0v: {error}"))?,
        )?;
        if parsed.spec != spec {
            return Err("q0v parse-back changed the stream specification".to_string());
        }
        if spec.video {
            parsed.decode_frame_rgba(0)?;
            parsed.decode_frame_rgba(total.saturating_sub(1) as usize)?;
        }
        if spec.audio
            && parsed.audio_samples_per_channel != spec.expected_audio_samples_per_channel()
        {
            return Err("q0v audio duration changed during parse-back".to_string());
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(WorkerOutcome::Cancelled);
        }
        crate::file_io::replace_staged_file(&staging, &job.output_path)
            .map_err(|error| format!("publish q0v: {error}"))?;
        progress(total, total, "q0v verified");
        Ok(WorkerOutcome::Completed)
    })();
    if !matches!(result, Ok(WorkerOutcome::Completed)) {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

#[cfg(windows)]
fn export_mp4(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    let range = job.options.frame_range(job.snapshot())?;
    let total = job.options.total_frames(job.snapshot())?;
    ensure_parent(&job.output_path)?;
    let staging = media_staging_path(&job.output_path, job.id, "mp4");
    if staging.exists() {
        std::fs::remove_file(&staging)
            .map_err(|error| format!("remove stale mp4 staging file: {error}"))?;
    }
    let result = (|| {
        let encoded_width = h264_encoded_dimension(job.options.width);
        let encoded_height = h264_encoded_dimension(job.options.height);
        let spec = q0video::mp4::Mp4Spec {
            width: encoded_width,
            height: encoded_height,
            fps: u32::from(job.snapshot().meta.fps.max(1)),
            bitrate: job.options.mp4_bitrate,
        };
        let mut encoder = q0video::mp4::Mp4Encoder::create(&staging, spec)?;
        for (offset, frame_number) in range.enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Ok(WorkerOutcome::Cancelled);
            }
            progress(offset as u32, total, "encoding h.264 frames");
            let rgba = render_frame(job.snapshot(), &job.options, frame_number);
            let encoded = pad_rgba_to_even(
                &rgba,
                job.options.width,
                job.options.height,
                encoded_width,
                encoded_height,
            );
            encoder.write_rgba_frame(&encoded)?;
            progress(offset as u32 + 1, total, "writing mp4 samples");
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(WorkerOutcome::Cancelled);
        }
        progress(total, total, "finalizing mp4");
        encoder.finish()?;
        sync_existing_file(&staging)?;
        verify_mp4(&staging)?;
        crate::file_io::replace_staged_file(&staging, &job.output_path)
            .map_err(|error| format!("publish mp4: {error}"))?;
        Ok(WorkerOutcome::Completed)
    })();
    if !matches!(result, Ok(WorkerOutcome::Completed)) {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

#[cfg(not(windows))]
fn export_mp4(
    _job: &ExportJob,
    _cancel: &AtomicBool,
    _progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    Err("mp4 export currently requires the windows media foundation backend".to_string())
}

fn export_png_sequence(
    job: &ExportJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32, u32, &str),
) -> Result<WorkerOutcome, String> {
    let range = job.options.frame_range(job.snapshot())?;
    let total = job.options.total_frames(job.snapshot())?;
    if job.options.sequence_create_subfolder && job.output_path.exists() {
        return Err(format!(
            "output subfolder already exists: {}",
            job.output_path.display()
        ));
    }
    if job.output_path.exists() && !job.output_path.is_dir() {
        return Err(format!(
            "png sequence destination is not a folder: {}",
            job.output_path.display()
        ));
    }
    ensure_parent(&job.output_path)?;
    let digits = decimal_digits(u32::from(*range.end()) + 1).max(4);
    let frames: Vec<u16> = range.collect();
    let filenames: Vec<String> = frames
        .iter()
        .map(|frame| {
            format!(
                "{}_{:0width$}.png",
                job.options.sequence_prefix,
                u32::from(*frame) + 1,
                width = digits as usize
            )
        })
        .collect();
    if !job.options.sequence_create_subfolder {
        reject_sequence_name_conflicts(&job.output_path, &filenames)?;
    }

    let staging = sequence_staging_path(&job.output_path, job.id);
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|error| format!("remove stale staging folder: {error}"))?;
    }
    std::fs::create_dir(&staging).map_err(|error| format!("create staging folder: {error}"))?;

    let result = (|| {
        for (offset, (frame, filename)) in frames.iter().zip(&filenames).enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Ok(WorkerOutcome::Cancelled);
            }
            let completed = offset as u32;
            progress(completed, total, "rendering png frames");
            let rgba = render_frame(job.snapshot(), &job.options, *frame);
            if cancel.load(Ordering::Acquire) {
                return Ok(WorkerOutcome::Cancelled);
            }
            let bytes = encode_image(
                &rgba,
                job.options.width,
                job.options.height,
                ImageCodec::Png,
                100,
            )?;
            verify_encoded_image(&bytes, job.options.width, job.options.height)?;
            crate::file_io::write_bytes_atomic(&staging.join(filename), &bytes)
                .map_err(|error| format!("write png frame: {error}"))?;
            progress(completed + 1, total, "writing png frames");
        }
        Ok(WorkerOutcome::Completed)
    })();

    match result {
        Ok(WorkerOutcome::Completed) if job.options.sequence_create_subfolder => {
            match std::fs::rename(&staging, &job.output_path) {
                Ok(()) => Ok(WorkerOutcome::Completed),
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    Err(format!("publish sequence subfolder: {error}"))
                }
            }
        }
        Ok(WorkerOutcome::Completed) => {
            let publish = publish_sequence_into_folder(&staging, &job.output_path, &filenames);
            let _ = std::fs::remove_dir_all(&staging);
            publish.map(|()| WorkerOutcome::Completed)
        }
        Ok(WorkerOutcome::Cancelled) => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok(WorkerOutcome::Cancelled)
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

fn render_frame(project: &ProjectV2, options: &ExportOptions, frame: u16) -> Vec<u8> {
    let transparent = options.transparent
        && match options.format {
            ExportFormat::PngSequence | ExportFormat::Gif | ExportFormat::Q0v => true,
            ExportFormat::Image => options.image_codec.supports_alpha(),
            _ => false,
        };
    let background = if transparent {
        [0, 0, 0, 0]
    } else {
        [255, 255, 255, 255]
    };
    q0s_format::raster::rasterize_q0rg_frame_scaled(
        project,
        options.source_q0rg_id,
        frame,
        options.width,
        options.height,
        u32::from(options.supersampling),
        background,
    )
}

fn encode_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    codec: ImageCodec,
    jpeg_quality: u8,
) -> Result<Vec<u8>, String> {
    let image: RgbaImage = ImageBuffer::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "rgba buffer length does not match output dimensions".to_string())?;
    let dynamic = DynamicImage::ImageRgba8(image);
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    match codec {
        ImageCodec::Png => dynamic
            .write_to(&mut cursor, ImageOutputFormat::Png)
            .map_err(|error| format!("encode png: {error}"))?,
        ImageCodec::Jpeg => DynamicImage::ImageRgb8(dynamic.to_rgb8())
            .write_to(
                &mut cursor,
                ImageOutputFormat::Jpeg(jpeg_quality.clamp(1, 100)),
            )
            .map_err(|error| format!("encode jpeg: {error}"))?,
        ImageCodec::WebP => dynamic
            .write_to(&mut cursor, ImageOutputFormat::WebP)
            .map_err(|error| format!("encode webp: {error}"))?,
    }
    Ok(bytes)
}

fn verify_encoded_image(bytes: &[u8], width: u32, height: u32) -> Result<(), String> {
    let decoded =
        image::load_from_memory(bytes).map_err(|error| format!("decode-back: {error}"))?;
    if decoded.width() != width || decoded.height() != height {
        return Err(format!(
            "decode-back dimensions changed from {width}x{height} to {}x{}",
            decoded.width(),
            decoded.height()
        ));
    }
    Ok(())
}

fn verify_gif(path: &Path, width: u32, height: u32, expected_frames: u32) -> Result<(), String> {
    let file = File::open(path).map_err(|error| format!("open gif for verification: {error}"))?;
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(file)
        .map_err(|error| format!("decode gif header: {error}"))?;
    if u32::from(decoder.width()) != width || u32::from(decoder.height()) != height {
        return Err("gif decode-back dimensions changed".to_string());
    }
    let mut frames = 0_u32;
    while decoder
        .read_next_frame()
        .map_err(|error| format!("decode gif frame: {error}"))?
        .is_some()
    {
        frames = frames.saturating_add(1);
    }
    if frames != expected_frames {
        return Err(format!(
            "gif decode-back frame count changed from {expected_frames} to {frames}"
        ));
    }
    Ok(())
}

fn gif_delay_centiseconds(frame_index: u64, fps: u32) -> u16 {
    let fps = u64::from(fps.max(1));
    let start = frame_index.saturating_mul(100) / fps;
    let end = frame_index.saturating_add(1).saturating_mul(100) / fps;
    end.saturating_sub(start).max(1).min(u64::from(u16::MAX)) as u16
}

pub const fn h264_encoded_dimension(value: u32) -> u32 {
    let even = if value.is_multiple_of(2) {
        value
    } else {
        value.saturating_add(1)
    };
    if even < 64 {
        64
    } else {
        even
    }
}

fn pad_rgba_to_even(
    rgba: &[u8],
    width: u32,
    height: u32,
    encoded_width: u32,
    encoded_height: u32,
) -> Vec<u8> {
    if width == encoded_width && height == encoded_height {
        return rgba.to_vec();
    }
    let mut output = vec![255_u8; encoded_width as usize * encoded_height as usize * 4];
    for y in 0..height as usize {
        let source_start = y * width as usize * 4;
        let target_start = y * encoded_width as usize * 4;
        output[target_start..target_start + width as usize * 4]
            .copy_from_slice(&rgba[source_start..source_start + width as usize * 4]);
    }
    output
}

fn sync_existing_file(path: &Path) -> Result<(), String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("open staged file for sync: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync staged file: {error}"))
}

fn verify_mp4(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("stat mp4: {error}"))?;
    if metadata.len() < 32 {
        return Err("media foundation produced an unexpectedly small mp4".to_string());
    }
    let mut file =
        File::open(path).map_err(|error| format!("open mp4 for verification: {error}"))?;
    let mut header = [0_u8; 64];
    let read = file
        .read(&mut header)
        .map_err(|error| format!("read mp4 header: {error}"))?;
    if !header[..read].windows(4).any(|window| window == b"ftyp") {
        return Err("mp4 does not contain an ftyp box near the beginning".to_string());
    }
    Ok(())
}

fn reject_sequence_name_conflicts(folder: &Path, filenames: &[String]) -> Result<(), String> {
    if !folder.exists() {
        return Ok(());
    }
    for filename in filenames {
        let target = folder.join(filename);
        if target.exists() {
            return Err(format!(
                "png sequence would overwrite an existing file: {}",
                target.display()
            ));
        }
    }
    Ok(())
}

fn publish_sequence_into_folder(
    staging: &Path,
    folder: &Path,
    filenames: &[String],
) -> Result<(), String> {
    let folder_existed = folder.exists();
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("create png sequence folder: {error}"))?;
    reject_sequence_name_conflicts(folder, filenames)?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for filename in filenames {
        let source = staging.join(filename);
        let target = folder.join(filename);
        if let Err(error) = std::fs::rename(&source, &target) {
            for (published, original) in moved.into_iter().rev() {
                let _ = std::fs::rename(published, original);
            }
            if !folder_existed {
                let _ = std::fs::remove_dir(folder);
            }
            return Err(format!("publish png sequence frame: {error}"));
        }
        moved.push((target, source));
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create output directory: {error}"))?;
    }
    Ok(())
}

fn media_staging_path(target: &Path, job_id: u64, extension: &str) -> PathBuf {
    let name = target
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("media");
    target
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!(
            ".{name}.q0enc-{}-{job_id}.tmp.{extension}",
            std::process::id()
        ))
}

fn sequence_staging_path(target: &Path, job_id: u64) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("frames");
    target
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!(".{name}.q0enc-{}-{job_id}.tmp", std::process::id()))
}

fn decimal_digits(mut value: u32) -> u32 {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn validate_sequence_prefix(prefix: &str) -> Result<(), String> {
    validate_windows_file_component(prefix, "png sequence prefix")
}

fn validate_sequence_subfolder(name: &str) -> Result<(), String> {
    validate_windows_file_component(name, "png sequence subfolder")
}

fn validate_windows_file_component(value: &str, label: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    if value == "." || value == ".." {
        return Err(format!("{label} cannot be a relative path marker"));
    }
    if value.chars().any(|character| {
        matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
        )
    }) {
        return Err(format!("{label} contains a path character"));
    }
    Ok(())
}

pub fn source_last_frame(project: &ProjectV2, q0rg_id: u16) -> u16 {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .map(|q0rg| q0rg.frame_count.max(1).saturating_sub(1))
        .unwrap_or(0)
}

fn project_stem(file_path: Option<&Path>) -> String {
    file_path
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("movie")
        .to_string()
}

pub fn suggested_output_path(
    file_path: Option<&Path>,
    format: ExportFormat,
    image_codec: ImageCodec,
) -> PathBuf {
    let stem = project_stem(file_path);
    let name = match format {
        ExportFormat::PngSequence => format!("{stem}-frames"),
        ExportFormat::Image => format!("{stem}.{}", image_codec.extension()),
        other => format!("{stem}.{}", other.extension()),
    };
    file_path
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(""))
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Asset, BitmapAsset, Placement, Target, Transform2D, Tween};

    fn unique_output(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("q0enc-{label}-{}-{nonce}", std::process::id()))
    }

    fn visual_project(frame_count: u16) -> ProjectV2 {
        let mut project = crate::state::default_project();
        project.meta.stage_width = 1;
        project.meta.stage_height = 1;
        project.q0rgs[0].frame_count = frame_count;
        project.assets.push(Asset::Bitmap(BitmapAsset {
            asset_id: 1,
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        }));
        project.q0rgs[0].layers[0].placements.push(Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
        project
    }

    fn wait_for_queue(state: &mut Q0EncState) -> Vec<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut statuses = Vec::new();
        while state.queue_running || state.has_active_job() {
            statuses.extend(state.poll_events());
            assert!(
                std::time::Instant::now() < deadline,
                "q0enc worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        statuses.extend(state.poll_events());
        statuses
    }

    #[test]
    fn q0enc_defaults_match_the_new_project_stage() {
        let project = crate::state::default_project();
        let state = Q0EncState::default();
        assert_eq!(state.width, u32::from(project.meta.stage_width));
        assert_eq!(state.height, u32::from(project.meta.stage_height));
    }

    #[test]
    fn q0v_is_a_synchronised_video_and_audio_container() {
        assert!(ExportFormat::Q0v.carries_video());
        assert!(ExportFormat::Q0v.carries_audio());
        assert_eq!(
            Q0vStreams::default(),
            Q0vStreams {
                video: true,
                audio: true
            }
        );

        let clock = MediaClock {
            fps: 24,
            audio_sample_rate: 48_000,
        };
        assert_eq!(clock.video_frame_ticks(240), 10 * MEDIA_TICKS_PER_SECOND);
        assert_eq!(
            clock.audio_sample_ticks(480_000),
            10 * MEDIA_TICKS_PER_SECOND
        );
        assert_eq!(clock.audio_samples_for_video_frame(0), 0..2000);
        assert_eq!(clock.audio_samples_for_video_frame(23), 46_000..48_000);
    }

    #[test]
    fn q0v_rejects_an_empty_container() {
        let project = crate::state::default_project();
        let options = ExportOptions {
            format: ExportFormat::Q0v,
            source_q0rg_id: 1,
            entire_timeline: true,
            first_frame: 0,
            last_frame: 23,
            width: 640,
            height: 360,
            supersampling: 2,
            transparent: false,
            image_codec: ImageCodec::Png,
            jpeg_quality: 90,
            sequence_prefix: "frame".into(),
            sequence_create_subfolder: true,
            gif_loop: true,
            gif_speed: 10,
            mp4_bitrate: 8_000_000,
            q0v_streams: Q0vStreams {
                video: false,
                audio: false,
            },
        };
        assert_eq!(
            options.validate(&project).unwrap_err(),
            "q0v must contain video, audio, or both"
        );
    }

    #[test]
    fn queue_waits_for_an_explicit_start() {
        let project = crate::state::default_project();
        let output = unique_output("wait.q0s");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue");

        assert!(state.poll_events().is_empty());
        assert!(!output.exists());
        state.cancel_job(1);
    }

    #[test]
    fn q0s_job_uses_an_immutable_snapshot_and_round_trips() {
        let mut project = crate::state::default_project();
        project.meta.name = "snapshot before edits".to_string();
        let output = unique_output("снимок").with_extension("q0s");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue");

        project.meta.name = "changed after enqueue".to_string();
        state.start_queue();
        let statuses = wait_for_queue(&mut state);
        assert!(statuses
            .iter()
            .any(|status| status.contains("q0enc exported q0s")));
        let parsed = q0s_format::parse_q0s_v2(&std::fs::read(&output).expect("read q0s"))
            .expect("parse q0s");
        assert_eq!(parsed.meta.name, "snapshot before edits");
        assert!(matches!(state.queue[0].state, JobState::Completed));
        std::fs::remove_file(output).expect("cleanup q0s");
    }

    #[test]
    fn image_job_renders_scaled_frame_and_decode_checks_it() {
        let project = visual_project(1);
        let output = unique_output("still").with_extension("png");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Image, None);
        state.width = 4;
        state.height = 3;
        state.supersampling = 1;
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue image");
        state.start_queue();
        wait_for_queue(&mut state);

        let decoded = image::open(&output).expect("open image").to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 3));
        assert_eq!(decoded.get_pixel(3, 2).0, [255, 0, 0, 255]);
        std::fs::remove_file(output).expect("cleanup image");
    }

    #[test]
    fn png_sequence_is_published_as_one_complete_subfolder() {
        let project = visual_project(2);
        let selected_folder = unique_output("frames-base");
        let output = selected_folder.join("shot-frames");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::PngSequence, None);
        state.width = 2;
        state.height = 2;
        state.supersampling = 1;
        state.sequence_create_subfolder = true;
        state.sequence_subfolder = "shot-frames".into();
        state.output_text = selected_folder.display().to_string();
        state.enqueue(&project).expect("enqueue sequence");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(output.join("frame_0001.png").is_file());
        assert!(output.join("frame_0002.png").is_file());
        assert_eq!(
            std::fs::read_dir(&output).expect("read sequence").count(),
            2
        );
        std::fs::remove_dir_all(selected_folder).expect("cleanup sequence");
    }

    #[test]
    fn existing_sequence_subfolder_is_preserved_on_failure() {
        let project = visual_project(1);
        let selected_folder = unique_output("existing-base");
        let output = selected_folder.join("existing-frames");
        std::fs::create_dir_all(&output).expect("create existing output");
        std::fs::write(output.join("keep.txt"), b"original").expect("seed output");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::PngSequence, None);
        state.sequence_create_subfolder = true;
        state.sequence_subfolder = "existing-frames".into();
        state.output_text = selected_folder.display().to_string();
        state.enqueue(&project).expect("enqueue sequence");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(matches!(state.queue[0].state, JobState::Failed(_)));
        assert_eq!(
            std::fs::read(output.join("keep.txt")).expect("preserved file"),
            b"original"
        );
        std::fs::remove_dir_all(selected_folder).expect("cleanup existing output");
    }

    #[test]
    fn every_still_codec_encodes_and_decodes_at_the_requested_size() {
        let rgba = [255, 20, 10, 128].repeat(6);
        for codec in ImageCodec::ALL {
            let bytes = encode_image(&rgba, 3, 2, codec, 87).expect("encode still codec");
            verify_encoded_image(&bytes, 3, 2).expect("decode still codec");
        }
    }

    #[test]
    fn transparent_still_keeps_a_real_alpha_channel() {
        let mut project = crate::state::default_project();
        project.meta.stage_width = 1;
        project.meta.stage_height = 1;
        project.q0rgs[0].frame_count = 1;
        let output = unique_output("transparent").with_extension("png");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Image, None);
        state.width = 1;
        state.height = 1;
        state.supersampling = 2;
        state.transparent = true;
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue transparent still");
        state.start_queue();
        wait_for_queue(&mut state);

        let decoded = image::open(&output)
            .expect("open transparent still")
            .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0, 0]);
        std::fs::remove_file(output).expect("cleanup transparent still");
    }

    #[test]
    fn cancelled_sequence_removes_its_private_staging_folder() {
        let project = visual_project(3);
        let output = unique_output("cancelled-frames");
        let options = ExportOptions {
            format: ExportFormat::PngSequence,
            source_q0rg_id: 1,
            entire_timeline: true,
            first_frame: 0,
            last_frame: 2,
            width: 2,
            height: 2,
            supersampling: 1,
            transparent: false,
            image_codec: ImageCodec::Png,
            jpeg_quality: 90,
            sequence_prefix: "frame".into(),
            sequence_create_subfolder: true,
            gif_loop: true,
            gif_speed: 10,
            mp4_bitrate: 8_000_000,
            q0v_streams: Q0vStreams::default(),
        };
        let job = ExportJob {
            id: 91,
            output_path: output.clone(),
            options,
            state: JobState::Running,
            completed_units: 0,
            total_units: 3,
            phase: "testing".into(),
            snapshot: project,
        };
        let cancel = AtomicBool::new(true);
        let outcome = execute_job(&job, &cancel, |_, _, _| {}).expect("cancel outcome");
        assert!(matches!(outcome, WorkerOutcome::Cancelled));
        assert!(!output.exists());
        assert!(!sequence_staging_path(&output, job.id).exists());
    }

    #[test]
    fn png_sequence_can_publish_directly_into_a_selected_folder() {
        let project = visual_project(2);
        let folder = unique_output("direct-frames");
        std::fs::create_dir(&folder).expect("create selected folder");
        std::fs::write(folder.join("keep.txt"), b"keep").expect("seed unrelated file");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::PngSequence, None);
        state.width = 2;
        state.height = 2;
        state.supersampling = 1;
        state.sequence_create_subfolder = false;
        state.output_text = folder.display().to_string();
        state.enqueue(&project).expect("enqueue direct sequence");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(matches!(state.queue[0].state, JobState::Completed));
        assert_eq!(std::fs::read(folder.join("keep.txt")).unwrap(), b"keep");
        assert!(folder.join("frame_0001.png").is_file());
        assert!(folder.join("frame_0002.png").is_file());
        std::fs::remove_dir_all(folder).expect("cleanup direct sequence");
    }

    #[test]
    fn gif_job_round_trips_all_frames() {
        let project = visual_project(2);
        let output = unique_output("animation").with_extension("gif");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Gif, None);
        state.width = 2;
        state.height = 2;
        state.supersampling = 1;
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue gif");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(matches!(state.queue[0].state, JobState::Completed));
        verify_gif(&output, 2, 2, 2).expect("verify exported gif");
        std::fs::remove_file(output).expect("cleanup gif");
    }

    #[test]
    fn q0v_job_contains_seekable_video_and_duration_correct_pcm() {
        let project = visual_project(2);
        let output = unique_output("media").with_extension("q0v");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Q0v, None);
        state.width = 2;
        state.height = 2;
        state.supersampling = 1;
        state.q0v_streams = Q0vStreams {
            video: true,
            audio: true,
        };
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue q0v");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(matches!(state.queue[0].state, JobState::Completed));
        let parsed = q0video::q0v::Q0vFile::parse(std::fs::read(&output).unwrap())
            .expect("parse exported q0v");
        assert!(parsed.spec.video && parsed.spec.audio);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(
            parsed.audio_samples_per_channel,
            parsed.spec.expected_audio_samples_per_channel()
        );
        assert_eq!(parsed.decode_frame_rgba(1).unwrap().len(), 2 * 2 * 4);
        std::fs::remove_file(output).expect("cleanup q0v");
    }

    #[cfg(windows)]
    #[test]
    fn mp4_job_uses_media_foundation_and_pads_odd_dimensions() {
        let project = visual_project(2);
        let output = unique_output("movie").with_extension("mp4");
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Mp4, None);
        state.width = 1;
        state.height = 1;
        state.supersampling = 1;
        state.mp4_bitrate = 500_000;
        state.output_text = output.display().to_string();
        state.enqueue(&project).expect("enqueue mp4");
        state.start_queue();
        wait_for_queue(&mut state);

        assert!(
            matches!(state.queue[0].state, JobState::Completed),
            "{:?}",
            state.queue[0].state
        );
        verify_mp4(&output).expect("verify exported mp4");
        std::fs::remove_file(output).expect("cleanup mp4");
    }

    #[test]
    fn image_codec_forces_the_matching_output_extension() {
        let project = visual_project(1);
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(ExportFormat::Image, None);
        state.select_image_codec(ImageCodec::WebP);
        state.output_text = unique_output("wrong-extension")
            .with_extension("png")
            .display()
            .to_string();
        state.enqueue(&project).expect("enqueue webp");
        assert_eq!(
            state.queue[0]
                .output_path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("webp")
        );
        state.cancel_job(state.queue[0].id);
    }

    #[test]
    fn suggested_path_keeps_the_project_directory_and_name() {
        let path = Path::new("folder with spaces/герой.q1s");
        assert_eq!(
            suggested_output_path(Some(path), ExportFormat::Q0s, ImageCodec::Png),
            PathBuf::from("folder with spaces/герой.q0s")
        );
        assert_eq!(
            suggested_output_path(Some(path), ExportFormat::Mp4, ImageCodec::Png),
            PathBuf::from("folder with spaces/герой.mp4")
        );
        assert_eq!(
            suggested_output_path(Some(path), ExportFormat::PngSequence, ImageCodec::Png),
            PathBuf::from("folder with spaces/герой-frames")
        );
    }
}
