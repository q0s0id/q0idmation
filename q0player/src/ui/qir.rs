//! q0player running a QIR app: a movie that arrived inside the program, with
//! the manifest Q0Build wrote deciding the window, the keys and where saves go.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontId};
use q0qir::{Bundle, CursorMode, Manifest};
use q0s_format::q0lang::runtime::RuntimeValue;

/// Values a movie keeps with `q0app.save!`, one JSON object per movie or app.
pub(crate) struct SaveFile {
    path: PathBuf,
    pub values: BTreeMap<String, RuntimeValue>,
}

impl SaveFile {
    pub fn open(path: PathBuf) -> Self {
        let values = std::fs::read(&path)
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<BTreeMap<String, serde_json::Value>>(&bytes).ok()
            })
            .map(|stored| {
                stored
                    .into_iter()
                    .map(|(key, value)| (key, from_json(value)))
                    .collect()
            })
            .unwrap_or_default();
        Self { path, values }
    }

    /// A movie opened in the player keeps its saves under the player's data,
    /// named after the file.
    pub fn for_movie(movie: &Path) -> Option<Self> {
        let folder = super::settings::Settings::path()?.parent()?.join("saves");
        let stem = movie.file_stem()?.to_string_lossy();
        Some(Self::open(
            folder.join(format!("{}.json", q0qir::file_safe(&stem))),
        ))
    }

    pub fn set(&mut self, key: String, value: RuntimeValue) -> std::io::Result<()> {
        self.values.insert(key, value);
        self.store()
    }

    pub fn forget(&mut self, key: Option<&str>) -> std::io::Result<()> {
        match key {
            Some(key) => {
                self.values.remove(key);
            }
            None => self.values.clear(),
        }
        self.store()
    }

    fn store(&self) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let stored: BTreeMap<&String, serde_json::Value> = self
            .values
            .iter()
            .map(|(key, value)| (key, to_json(value)))
            .collect();
        let bytes = serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)?;
        super::atomic_file::write(&self.path, &bytes)
    }
}

fn to_json(value: &RuntimeValue) -> serde_json::Value {
    match value {
        RuntimeValue::Number(number) => serde_json::Number::from_f64(*number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        RuntimeValue::Bool(on) => serde_json::Value::Bool(*on),
        RuntimeValue::Null => serde_json::Value::Null,
        other => serde_json::Value::String(other.display_lossy()),
    }
}

fn from_json(value: serde_json::Value) -> RuntimeValue {
    match value {
        serde_json::Value::Number(number) => RuntimeValue::Number(number.as_f64().unwrap_or(0.0)),
        serde_json::Value::Bool(on) => RuntimeValue::Bool(on),
        serde_json::Value::String(text) => RuntimeValue::String(text),
        _ => RuntimeValue::Null,
    }
}

/// Where an app keeps its saves and log.
pub(crate) fn data_dir(manifest: &Manifest) -> PathBuf {
    let folder = manifest.data_folder();
    if manifest.data.portable {
        if let Some(beside) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            return beside.join(format!("{folder} data"));
        }
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(folder)
}

const IDLE_CURSOR: Duration = Duration::from_secs(2);
/// Where through the splash it starts fading over the movie.
const SPLASH_FADE_OUT_AT: f32 = 0.72;

pub struct QirSession {
    pub manifest: Manifest,
    pub icon_png: Option<Vec<u8>>,
    movie: Option<Vec<u8>>,
    pub data_dir: PathBuf,
    /// What `app.args` reads: the command line after the program.
    pub args: Vec<String>,
    log: Option<File>,
    started: Instant,
    /// Set while the window is away: whether the movie was playing.
    pub paused_for_focus: Option<bool>,
    /// The movie waits behind the splash.
    pub splash_hold: bool,
    splash_skipped: Option<Instant>,
    splash_skipped_progress: f32,
    icon_texture: Option<egui::TextureHandle>,
    pointer_moved_at: Instant,
    last_pointer: Option<egui::Pos2>,
    _instance: Option<InstanceLock>,
}

impl QirSession {
    pub fn new(bundle: Bundle, args: Vec<String>) -> Self {
        let Bundle {
            manifest,
            icon_png,
            movie,
        } = bundle;
        let data_dir = data_dir(&manifest);
        let log = manifest
            .diagnostics
            .log_file
            .then(|| {
                std::fs::create_dir_all(&data_dir).ok()?;
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(data_dir.join("log.txt"))
                    .ok()
            })
            .flatten();
        if manifest.diagnostics.console {
            open_console();
        }
        let mut session = Self {
            manifest,
            icon_png,
            movie: Some(movie),
            data_dir,
            args,
            log,
            started: Instant::now(),
            paused_for_focus: None,
            splash_hold: false,
            splash_skipped: None,
            splash_skipped_progress: 0.0,
            icon_texture: None,
            pointer_moved_at: Instant::now(),
            last_pointer: None,
            _instance: None,
        };
        session.log(&format!(
            "{} {} started",
            session.manifest.app.name, session.manifest.app.version
        ));
        session
    }

    /// Only one copy runs: false when another one already does.
    pub fn claim_single_instance(&mut self) -> bool {
        if !self.manifest.behaviour.single_instance {
            return true;
        }
        match InstanceLock::claim(&self.manifest.app.id) {
            Some(lock) => {
                self._instance = Some(lock);
                true
            }
            None => false,
        }
    }

    pub fn take_movie(&mut self) -> Option<Vec<u8>> {
        self.movie.take()
    }

    pub(crate) fn saves(&self) -> SaveFile {
        SaveFile::open(self.data_dir.join("save.json"))
    }

    /// A line in the console and `log.txt`, as the manifest asks.
    pub fn log(&mut self, line: &str) {
        let elapsed = self.started.elapsed().as_secs_f32();
        let line = format!("[{elapsed:8.2}] {line}");
        if self.manifest.diagnostics.console {
            println!("{line}");
        }
        if let Some(log) = self.log.as_mut() {
            let _ = writeln!(log, "{line}");
        }
    }

    fn splash_total(&self) -> Duration {
        Duration::from_millis(u64::from(self.manifest.splash.duration_ms.max(300)))
    }

    /// How far through the splash, 0 to 1; past 1 it is over.
    fn splash_progress(&self) -> f32 {
        if let Some(skipped) = self.splash_skipped {
            // A skip fades out in a quarter second from wherever it was.
            let from = self.splash_skipped_progress.max(SPLASH_FADE_OUT_AT);
            return from + skipped.elapsed().as_secs_f32() / 0.25 * (1.0 - from);
        }
        self.started.elapsed().as_secs_f32() / self.splash_total().as_secs_f32()
    }

    pub fn splash_active(&self) -> bool {
        self.manifest.splash.enabled && self.splash_progress() < 1.0
    }

    /// The splash has begun fading over the movie, which can start playing.
    pub fn splash_revealing(&self) -> bool {
        !self.splash_active() || self.splash_progress() >= SPLASH_FADE_OUT_AT
    }

    /// A click, tap or key ends the splash early, if the manifest lets it.
    pub fn skip_splash_on_input(&mut self, ctx: &Context) {
        if !self.manifest.splash.skippable || self.splash_skipped.is_some() || !self.splash_active()
        {
            return;
        }
        let pressed = ctx.input(|input| {
            input.pointer.any_pressed()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { pressed: true, .. } | egui::Event::Touch { .. }
                    )
                })
        });
        if pressed {
            self.splash_skipped_progress = self.splash_progress();
            self.splash_skipped = Some(Instant::now());
        }
    }

    /// The app's icon as a window icon.
    pub fn window_icon(&self) -> Option<egui::IconData> {
        self.icon_png
            .as_deref()
            .and_then(|png| eframe::icon_data::from_png_bytes(png).ok())
    }

    /// The splash over everything while it lasts: the app's icon, its name
    /// and its line, fading in, then fading away over the running movie.
    pub fn draw_splash(&mut self, ctx: &Context) {
        if !self.splash_active() {
            return;
        }
        let t = self.splash_progress().clamp(0.0, 1.0);
        let shown = (t / 0.18).min(1.0);
        let overlay = if t > SPLASH_FADE_OUT_AT {
            1.0 - (t - SPLASH_FADE_OUT_AT) / (1.0 - SPLASH_FADE_OUT_AT)
        } else {
            1.0
        };
        let ease = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
        let alpha = ease(shown) * overlay;
        let [r, g, b] = self.manifest.splash.background;
        let light = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b) > 140.0;
        let ink = |strength: f32| {
            let value = (alpha * strength * 255.0) as u8;
            if light {
                Color32::from_rgba_unmultiplied(18, 18, 22, value)
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, value)
            }
        };
        if self.manifest.splash.show_icon && self.icon_texture.is_none() {
            if let Some(icon) = self.window_icon() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                );
                self.icon_texture =
                    Some(ctx.load_texture("qir-splash-icon", image, egui::TextureOptions::LINEAR));
            }
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("qir-splash"),
        ));
        let screen = ctx.screen_rect();
        painter.rect_filled(
            screen,
            0.0,
            Color32::from_rgba_unmultiplied(r, g, b, (overlay * 255.0) as u8),
        );
        let has_icon = self.manifest.splash.show_icon && self.icon_texture.is_some();
        let center = screen.center();
        let name_y = if has_icon {
            center.y + 46.0
        } else {
            center.y - 8.0
        };
        if let (true, Some(texture)) = (has_icon, self.icon_texture.as_ref()) {
            let side = 104.0 * (0.94 + 0.06 * ease(shown));
            let rect = egui::Rect::from_center_size(
                egui::pos2(center.x, center.y - 30.0),
                egui::vec2(side, side),
            );
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::from_white_alpha((alpha * 255.0) as u8),
            );
        }
        painter.text(
            egui::pos2(center.x, name_y),
            Align2::CENTER_CENTER,
            &self.manifest.app.name,
            FontId::proportional(24.0),
            ink(0.95),
        );
        if !self.manifest.splash.text.trim().is_empty() {
            painter.text(
                egui::pos2(center.x, name_y + 26.0),
                Align2::CENTER_CENTER,
                &self.manifest.splash.text,
                FontId::proportional(13.0),
                ink(0.55),
            );
        }
        // A hairline of how long is left.
        let track = egui::Rect::from_center_size(
            egui::pos2(center.x, screen.bottom() - 40.0),
            egui::vec2(120.0, 2.0),
        );
        painter.rect_filled(track, 1.0, ink(0.12));
        let filled = (t / SPLASH_FADE_OUT_AT).min(1.0);
        painter.rect_filled(
            egui::Rect::from_min_size(
                track.min,
                egui::vec2(track.width() * filled, track.height()),
            ),
            1.0,
            ink(0.55),
        );
        ctx.request_repaint();
    }

    /// The manifest's cursor rule for this frame.
    pub fn apply_cursor(&mut self, ctx: &Context) {
        let pointer = ctx.input(|input| input.pointer.latest_pos());
        if pointer != self.last_pointer {
            self.last_pointer = pointer;
            self.pointer_moved_at = Instant::now();
        }
        let hide = match self.manifest.behaviour.cursor {
            CursorMode::Show => false,
            CursorMode::Hide => true,
            CursorMode::HideWhenIdle => {
                let rested = self.pointer_moved_at.elapsed();
                if rested < IDLE_CURSOR {
                    ctx.request_repaint_after(IDLE_CURSOR - rested);
                }
                rested >= IDLE_CURSOR
            }
        };
        if hide {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }
}

/// Windows: a console for the log, since the runtime has none of its own.
fn open_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AllocConsole();
    }
}

/// Held while the app runs; a second copy cannot take it.
struct InstanceLock {
    #[cfg(windows)]
    _mutex: isize,
    #[cfg(not(windows))]
    _file: File,
}

impl InstanceLock {
    fn claim(app_id: &str) -> Option<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
            use windows_sys::Win32::System::Threading::CreateMutexW;
            let name: Vec<u16> = format!("Local\\q0qir-{app_id}")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
            if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                return None;
            }
            Some(Self {
                _mutex: mutex as isize,
            })
        }
        #[cfg(not(windows))]
        {
            // Creating the file fails while another copy holds it; a copy
            // that crashed leaves it behind, so stale files older than the
            // last boot would need clearing by hand.
            let path = std::env::temp_dir().join(format!("q0qir-{app_id}.lock"));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .ok()?;
            Some(Self { _file: file })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_come_back_as_the_values_scripts_wrote() {
        let path = std::env::temp_dir().join(format!("q0qir-save-{}.json", std::process::id()));
        let mut file = SaveFile::open(path.clone());
        file.set("best".into(), RuntimeValue::Number(120.0))
            .unwrap();
        file.set("name".into(), RuntimeValue::String("grunt".into()))
            .unwrap();
        file.set("won".into(), RuntimeValue::Bool(true)).unwrap();
        let again = SaveFile::open(path.clone());
        assert_eq!(again.values.get("best"), Some(&RuntimeValue::Number(120.0)));
        assert_eq!(
            again.values.get("name"),
            Some(&RuntimeValue::String("grunt".into()))
        );
        assert_eq!(again.values.get("won"), Some(&RuntimeValue::Bool(true)));
        let mut again = again;
        again.forget(Some("best")).unwrap();
        assert!(!SaveFile::open(path.clone()).values.contains_key("best"));
        again.forget(None).unwrap();
        assert!(SaveFile::open(path.clone()).values.is_empty());
        std::fs::remove_file(path).ok();
    }
}
