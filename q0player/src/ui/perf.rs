//! The performance log F4 copies: how long the player's frames take, kept
//! since the movie opened, so playback that stutters on someone else's
//! computer can be sent back and read.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Frames kept one by one: about ten seconds of playback.
const HISTORY_FRAMES: usize = 600;
const RETAINED_STALLS: usize = 32;
/// Movie frames per row of the table by position.
const POSITION_ROW: u32 = 50;
/// Playback time per row of the table by time.
const TIME_ROW: Duration = Duration::from_secs(10);
const TIME_ROWS: usize = 360;
const MEMORY_SAMPLE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Span {
    /// All of `App::update`.
    Update = 0,
    /// Scripts, objects, physics and timeline: `Player::tick`.
    Tick,
    /// Drawing the movie.
    Scene,
    /// Live particles and liquids drawn over it.
    Live,
    /// Menus, transport and side panels.
    Chrome,
    /// Bitmap and q0v frames turned into a texture.
    Texture,
    /// Seeking from the seek bar or the keys.
    Seek,
    /// What eframe spent after `update`: tessellation, texture uploads and
    /// GL submission, without waiting for vsync.
    Backend,
    /// From one update to the next while the movie plays.
    Interval,
}

impl Span {
    const COUNT: usize = 9;
    const ALL: [Self; Self::COUNT] = [
        Self::Update,
        Self::Tick,
        Self::Scene,
        Self::Live,
        Self::Chrome,
        Self::Texture,
        Self::Seek,
        Self::Backend,
        Self::Interval,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Update => "frame.update",
            Self::Tick => "player.tick",
            Self::Scene => "draw.scene",
            Self::Live => "draw.live-effects",
            Self::Chrome => "ui.chrome",
            Self::Texture => "draw.texture",
            Self::Seek => "player.seek",
            Self::Backend => "frame.backend-cpu",
            Self::Interval => "frame.interval-playing",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    index: u64,
    /// Seconds since the movie opened.
    at: f32,
    ms: [f32; Span::COUNT],
    movie_frame: u32,
    /// Movie frames the tick played this update.
    steps: u16,
    playing: bool,
}

impl Frame {
    fn get(&self, span: Span) -> f32 {
        self.ms[span as usize]
    }

    fn stalled(&self) -> bool {
        self.get(Span::Update) >= 33.0
            || self.get(Span::Backend) >= 33.0
            || (self.playing && self.get(Span::Interval) >= 50.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Row {
    frames: u32,
    update: f64,
    backend: f64,
    interval: f64,
    intervals: u32,
    worst: f32,
    steps: u32,
    stalls: u32,
    /// Resident memory in bytes when the row was last written.
    memory: Option<u64>,
}

impl Row {
    fn add(&mut self, frame: &Frame, memory: Option<u64>) {
        self.frames += 1;
        self.update += f64::from(frame.get(Span::Update));
        self.backend += f64::from(frame.get(Span::Backend));
        if frame.playing && frame.get(Span::Interval) > 0.0 {
            self.interval += f64::from(frame.get(Span::Interval));
            self.intervals += 1;
        }
        self.worst = self
            .worst
            .max(frame.get(Span::Update) + frame.get(Span::Backend));
        self.steps += u32::from(frame.steps);
        self.stalls += u32::from(frame.stalled());
        if memory.is_some() {
            self.memory = memory;
        }
    }

    fn line(&self) -> String {
        let n = f64::from(self.frames.max(1));
        let interval = if self.intervals > 0 {
            format!("{:>7.2}", self.interval / f64::from(self.intervals))
        } else {
            "      -".to_string()
        };
        format!(
            "updates={:<5} update={:>6.2} backend={:>6.2} interval={} worst={:>7.2} stalls={:<4} movie_frames={:<5} memory={}",
            self.frames,
            self.update / n,
            self.backend / n,
            interval,
            self.worst,
            self.stalls,
            self.steps,
            self.memory.map_or_else(|| "-".to_string(), megabytes),
        )
    }
}

pub struct PerfLog {
    opened: Instant,
    started: Option<Instant>,
    current: Frame,
    /// The last finished frame, waiting for its backend time, which eframe
    /// only reports at the start of the next one.
    pending: Option<Frame>,
    next_index: u64,
    last_update: Option<Instant>,
    history: VecDeque<Frame>,
    stalls: VecDeque<Frame>,
    by_position: BTreeMap<u32, Row>,
    by_time: VecDeque<(u64, Row)>,
    memory: Option<u64>,
    memory_at: Option<Instant>,
    first_memory: Option<u64>,
}

impl Default for PerfLog {
    fn default() -> Self {
        Self {
            opened: Instant::now(),
            started: None,
            current: Frame::default(),
            pending: None,
            next_index: 0,
            last_update: None,
            history: VecDeque::with_capacity(HISTORY_FRAMES),
            stalls: VecDeque::with_capacity(RETAINED_STALLS),
            by_position: BTreeMap::new(),
            by_time: VecDeque::new(),
            memory: None,
            memory_at: None,
            first_memory: None,
        }
    }
}

impl PerfLog {
    /// Starts over for a movie that was just opened.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Starts timing an update. `backend_seconds` is eframe's CPU time for
    /// the previous frame, which belongs to that frame.
    pub fn begin(&mut self, backend_seconds: Option<f32>, playing: bool) {
        if let Some(mut frame) = self.pending.take() {
            if let Some(seconds) = backend_seconds.filter(|s| s.is_finite() && *s >= 0.0) {
                frame.ms[Span::Backend as usize] = seconds * 1000.0;
            }
            self.keep(frame);
        }
        let now = Instant::now();
        let interval = self.last_update.replace(now).map(|last| now - last);
        self.current = Frame {
            index: self.next_index,
            at: (now - self.opened).as_secs_f32(),
            playing,
            ..Frame::default()
        };
        if let Some(interval) = interval.filter(|_| playing) {
            self.record(Span::Interval, interval);
        }
        self.started = Some(now);
    }

    pub fn record(&mut self, span: Span, duration: Duration) {
        self.current.ms[span as usize] += duration.as_secs_f32() * 1000.0;
    }

    pub fn finish(&mut self, movie_frame: usize, steps: usize) {
        let Some(started) = self.started.take() else {
            return;
        };
        self.current.ms[Span::Update as usize] = started.elapsed().as_secs_f32() * 1000.0;
        self.current.movie_frame = movie_frame.min(u32::MAX as usize) as u32;
        self.current.steps = steps.min(usize::from(u16::MAX)) as u16;
        self.next_index += 1;
        self.pending = Some(self.current);
    }

    fn keep(&mut self, frame: Frame) {
        if self
            .memory_at
            .is_none_or(|at| at.elapsed() >= MEMORY_SAMPLE)
        {
            self.memory_at = Some(Instant::now());
            self.memory = process_memory();
            self.first_memory = self.first_memory.or(self.memory);
        }
        if self.history.len() == HISTORY_FRAMES {
            self.history.pop_front();
        }
        self.history.push_back(frame);
        if frame.stalled() {
            if self.stalls.len() == RETAINED_STALLS {
                self.stalls.pop_front();
            }
            self.stalls.push_back(frame);
        }
        if frame.playing {
            self.by_position
                .entry(frame.movie_frame / POSITION_ROW)
                .or_default()
                .add(&frame, self.memory);
        }
        let row = (f64::from(frame.at) / TIME_ROW.as_secs_f64()) as u64;
        if self.by_time.back().is_none_or(|(last, _)| *last != row) {
            if self.by_time.len() == TIME_ROWS {
                self.by_time.pop_front();
            }
            self.by_time.push_back((row, Row::default()));
        }
        if let Some((_, last)) = self.by_time.back_mut() {
            last.add(&frame, self.memory);
        }
    }

    pub fn captured(&self) -> usize {
        self.history.len()
    }

    /// The log, under `header`: what the app knows about the movie, window
    /// and caches.
    pub fn report(&self, header: &str) -> String {
        let mut out = String::with_capacity(32 * 1024);
        out.push_str("q0player performance log\n");
        out.push_str(&format!(
            "version={} build={} os={} arch={}\n",
            env!("CARGO_PKG_VERSION"),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS,
            std::env::consts::ARCH,
        ));
        out.push_str(&format!(
            "threads_available={}\n",
            std::thread::available_parallelism().map_or(0, usize::from)
        ));
        out.push_str(header);
        out.push_str(&format!(
            "seconds_since_open={:.1} updates_since_open={} captured_updates={}\n",
            self.opened.elapsed().as_secs_f64(),
            self.next_index,
            self.history.len(),
        ));
        out.push_str(&format!(
            "memory_now={} memory_at_open={}\n",
            process_memory().map_or_else(|| "-".to_string(), megabytes),
            self.first_memory.map_or_else(|| "-".to_string(), megabytes),
        ));
        out.push_str("units=milliseconds unless noted\n");
        out.push_str("note: frame.update is all of App::update; frame.backend-cpu is eframe's tessellation, uploads and GL submission for the same frame; frame.interval-playing is the time from one update to the next while the movie plays, which follows the display (16.7 at 60 Hz); a movie frame is due every 1000/fps\n\n");

        out.push_str("summary of the last captured updates\n");
        for span in Span::ALL {
            let values: Vec<f64> = self
                .history
                .iter()
                .map(|frame| f64::from(frame.get(span)))
                .filter(|value| *value > 0.0)
                .collect();
            if values.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "{:<24} n={:<5} avg={:>8.3} p50={:>8.3} p95={:>8.3} p99={:>8.3} max={:>8.3}\n",
                span.label(),
                values.len(),
                mean(&values),
                percentile(&values, 0.50),
                percentile(&values, 0.95),
                percentile(&values, 0.99),
                values.iter().copied().fold(0.0_f64, f64::max),
            ));
        }
        let dropped = self.history.iter().filter(|frame| frame.steps > 1).count();
        out.push_str(&format!(
            "updates that played more than one movie frame (the movie fell behind): {dropped}\n"
        ));

        out.push_str(&format!(
            "\nby movie position while playing (every {POSITION_ROW} frames, since the movie opened)\n"
        ));
        if self.by_position.is_empty() {
            out.push_str("nothing played yet\n");
        }
        for (row, values) in &self.by_position {
            let first = row * POSITION_ROW;
            out.push_str(&format!(
                "frames {:>5}-{:<5} {}\n",
                first + 1,
                first + POSITION_ROW,
                values.line()
            ));
        }

        out.push_str(&format!(
            "\nby time since the movie opened (every {} s)\n",
            TIME_ROW.as_secs()
        ));
        for (row, values) in &self.by_time {
            let start = row * TIME_ROW.as_secs();
            out.push_str(&format!(
                "{:>5}-{:<5}s {}\n",
                start,
                start + TIME_ROW.as_secs(),
                values.line()
            ));
        }

        out.push_str(
            "\nstalls (update or backend >= 33, or interval >= 50 while playing; latest 32)\n",
        );
        if self.stalls.is_empty() {
            out.push_str("none\n");
        }
        for frame in &self.stalls {
            out.push_str(&frame_line(frame));
        }

        out.push_str("\nworst captured updates\n");
        let mut worst: Vec<&Frame> = self.history.iter().collect();
        worst.sort_by(|a, b| {
            (b.get(Span::Update) + b.get(Span::Backend))
                .total_cmp(&(a.get(Span::Update) + a.get(Span::Backend)))
        });
        for frame in worst.into_iter().take(12) {
            out.push_str(&frame_line(frame));
        }
        out
    }
}

fn frame_line(frame: &Frame) -> String {
    let mut line = format!(
        "update#{} t={:.1}s movie_frame={} playing={} steps={}",
        frame.index,
        frame.at,
        frame.movie_frame + 1,
        frame.playing,
        frame.steps
    );
    for span in Span::ALL {
        let value = frame.get(span);
        if value > 0.0 {
            line.push_str(&format!(" {}={value:.2}", span.label()));
        }
    }
    line.push('\n');
    line
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len().saturating_sub(1)) as f64 * p).round() as usize;
    sorted.get(index).copied().unwrap_or(0.0)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
}

/// How much memory the player holds now, in bytes.
#[cfg(windows)]
pub fn process_memory() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the pseudo handle needs no closing, and `counters` is as large
    // as `cb` says.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some(counters.WorkingSetSize as u64)
}

/// How much memory the player holds now, in bytes.
#[cfg(not(windows))]
pub fn process_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(log: &mut PerfLog, movie_frame: usize, update_ms: u64) {
        log.begin(Some(0.002), true);
        log.record(Span::Tick, Duration::from_millis(update_ms));
        log.finish(movie_frame, 1);
        if let Some(pending) = log.pending.as_mut() {
            pending.ms[Span::Update as usize] = update_ms as f32;
        }
    }

    #[test]
    fn the_log_shows_where_in_the_movie_updates_got_slow() {
        let mut log = PerfLog::default();
        for frame in 0..120 {
            played(&mut log, frame, if frame < 60 { 2 } else { 40 });
        }
        log.begin(None, false);
        let report = log.report("movie=test.q0s\n");
        assert!(report.contains("movie=test.q0s"));
        let row = |first: &str| {
            report
                .lines()
                .find(|line| line.starts_with(&format!("frames {first:>5}")))
                .unwrap_or_else(|| panic!("row {first} in\n{report}"))
                .to_string()
        };
        assert!(row("1").contains("stalls=0"), "{}", row("1"));
        assert!(row("101").contains("stalls=20"), "{}", row("101"));
        assert!(report.contains("player.tick"));
    }

    #[test]
    fn backend_time_belongs_to_the_frame_before() {
        let mut log = PerfLog::default();
        log.begin(None, true);
        log.finish(0, 1);
        log.begin(Some(0.050), true);
        let kept = log.history.back().expect("first frame kept");
        assert_eq!(kept.movie_frame, 0);
        assert!((kept.get(Span::Backend) - 50.0).abs() < 0.01);
        assert!(kept.stalled());
    }
}
