//! Q0Build's window.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui::{vec2, Color32, RichText, TextureHandle};
use q0qir::{Backdrop, CursorMode, EscapeKey, StartMode, Unfocused};

use crate::builder::{self, Artifact, Event, Level};
use crate::inspect;
use crate::profile::{default_app_id, Profile};
use crate::runtimes::{self, Target};
use crate::style::{self, Glyph, Tone};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Project,
    App,
    Window,
    Runtime,
    Data,
    Targets,
    Output,
    Inspector,
    Scripts,
    Reader,
    Icons,
}

impl Page {
    /// One line under the page title.
    fn blurb(self) -> &'static str {
        match self {
            Page::Project => "The movie the app plays, and its icon.",
            Page::App => "Who the app is: its name, id and version.",
            Page::Window => "The window it opens and what fills it around the movie.",
            Page::Runtime => "How it draws, behaves and starts.",
            Page::Data => "Where saves go, and how it reports problems.",
            Page::Targets => "The platforms to build for, and their runtimes.",
            Page::Output => "Where builds go and what comes with them.",
            Page::Inspector => "What a movie holds and what makes it heavy.",
            Page::Scripts => "Every q0lang script, read before the players do.",
            Page::Reader => "What a built app carries, and its movie back out.",
            Page::Icons => "The icon at every size it will be shown.",
        }
    }

    const SETTINGS: [(Page, Glyph, &'static str); 7] = [
        (Page::Project, Glyph::Film, "Project"),
        (Page::App, Glyph::Tag, "App"),
        (Page::Window, Glyph::Window, "Window"),
        (Page::Runtime, Glyph::Gear, "Runtime"),
        (Page::Data, Glyph::Disk, "Data & debug"),
        (Page::Targets, Glyph::Targets, "Targets"),
        (Page::Output, Glyph::Folder, "Output"),
    ];
    const TOOLS: [(Page, Glyph, &'static str); 4] = [
        (Page::Inspector, Glyph::Lens, "Inspector"),
        (Page::Scripts, Glyph::Check, "Script check"),
        (Page::Reader, Glyph::Package, "Bundle reader"),
        (Page::Icons, Glyph::Picture, "Icon studio"),
    ];
}

struct IconPreview {
    source: Option<PathBuf>,
    textures: Vec<(u32, TextureHandle)>,
    notes: Vec<String>,
}

pub struct Q0Build {
    profile: Profile,
    profile_path: Option<PathBuf>,
    saved: Profile,
    page: Page,
    movie: Option<Result<inspect::ProjectReport, String>>,
    movie_for: Option<PathBuf>,
    icon: Option<IconPreview>,
    log: Vec<(Level, String)>,
    progress: (f32, String),
    running: Option<Receiver<Event>>,
    finished: Option<Result<Vec<Artifact>, String>>,
    inspector: Option<Result<inspect::ProjectReport, String>>,
    scripts: Option<Result<(usize, Vec<inspect::ScriptIssue>), String>>,
    reader: Option<(PathBuf, Result<inspect::BuiltApp, String>)>,
    toast: Option<(String, Instant)>,
    settings: crate::settings::Settings,
    about: bool,
}

impl Q0Build {
    pub fn new(ctx: &egui::Context, open: Option<PathBuf>) -> Self {
        let settings = crate::settings::Settings::load();
        style::use_skin(ctx, style::Skin::by_id(&settings.skin));
        ctx.set_zoom_factor(settings.zoom());
        let mut app = Self {
            profile: Profile::default(),
            profile_path: None,
            saved: Profile::default(),
            page: Page::Project,
            movie: None,
            movie_for: None,
            icon: None,
            log: vec![(
                Level::Info,
                "Q0Build ready. Choose a movie, set up the app, press Build.".into(),
            )],
            progress: (0.0, "idle".into()),
            running: None,
            finished: None,
            inspector: None,
            scripts: None,
            reader: None,
            toast: None,
            settings,
            about: false,
        };
        if let Some(path) = open {
            app.open_any(&path);
        }
        app
    }

    fn dirty(&self) -> bool {
        self.profile != self.saved
    }

    fn say(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// A movie or a profile, whichever `path` is.
    fn open_any(&mut self, path: &Path) {
        let is_profile = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("q0build"));
        if is_profile {
            match Profile::load(path) {
                Ok(profile) => {
                    self.saved = profile.clone();
                    self.profile = profile;
                    self.profile_path = Some(path.to_path_buf());
                    self.settings.remember(path);
                    self.say(format!("opened {}", path.display()));
                }
                Err(error) => self.say(error),
            }
            return;
        }
        // A movie: its profile if it has one, else a fresh one named after it.
        let profile_path = Profile::path_for_movie(path);
        if profile_path.is_file() {
            self.open_any(&profile_path);
            self.profile.movie = Some(path.to_path_buf());
            return;
        }
        let mut profile = Profile {
            movie: Some(path.to_path_buf()),
            ..Profile::default()
        };
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        profile.manifest.app.name = stem.clone();
        profile.manifest.app.id = default_app_id(&stem, "");
        if let Ok((project, _)) = inspect::load_project(path) {
            profile.manifest.window.width = u32::from(project.meta.stage_width).max(64);
            profile.manifest.window.height = u32::from(project.meta.stage_height).max(64);
            let color = project.meta.background_color;
            profile.manifest.splash.background = [color.r, color.g, color.b];
        }
        self.profile = profile;
        self.saved = Profile::default();
        self.profile_path = Some(profile_path);
        self.say(format!("new profile for {}", path.display()));
    }

    fn save_profile(&mut self) {
        let Some(path) = self.profile_path.clone().or_else(|| {
            rfd::FileDialog::new()
                .add_filter("Q0Build profile", &["q0build"])
                .set_file_name("app.q0build")
                .save_file()
        }) else {
            return;
        };
        match self.profile.save(&path) {
            Ok(()) => {
                self.saved = self.profile.clone();
                self.profile_path = Some(path.clone());
                self.settings.remember(&path);
                self.say(format!("saved {}", path.display()));
            }
            Err(error) => self.say(error),
        }
    }

    fn refresh_movie(&mut self) {
        if self.movie_for == self.profile.movie {
            return;
        }
        self.movie_for = self.profile.movie.clone();
        self.movie = self.profile.movie.as_ref().map(|path| {
            inspect::load_project(path)
                .map(|(project, bytes)| inspect::project_report(&project, bytes))
        });
    }

    fn refresh_icon(&mut self, ctx: &egui::Context) {
        if self
            .icon
            .as_ref()
            .is_some_and(|icon| icon.source == self.profile.icon)
        {
            return;
        }
        let made = builder::app_icon(&self.profile);
        let (textures, notes) = match made {
            Ok(icon) => (
                [16_u32, 32, 48, 64, 128, 256]
                    .into_iter()
                    .map(|side| {
                        (
                            side,
                            ctx.load_texture(
                                format!("icon-{side}"),
                                icon.color_image(side),
                                egui::TextureOptions::LINEAR,
                            ),
                        )
                    })
                    .collect(),
                icon.notes,
            ),
            Err(error) => (Vec::new(), vec![error]),
        };
        self.icon = Some(IconPreview {
            source: self.profile.icon.clone(),
            textures,
            notes,
        });
    }

    fn start_build(&mut self) {
        if self.running.is_some() {
            return;
        }
        self.log.clear();
        self.finished = None;
        self.progress = (0.0, "starting".into());
        self.running = Some(builder::spawn(self.profile.clone()));
    }

    fn poll_build(&mut self) {
        let Some(receiver) = self.running.as_ref() else {
            return;
        };
        let mut done = None;
        while let Ok(event) = receiver.try_recv() {
            match event {
                Event::Log(level, text) => self.log.push((level, text)),
                Event::Progress(fraction, text) => self.progress = (fraction, text),
                Event::Finished(result) => done = Some(result),
            }
        }
        if let Some(result) = done {
            self.running = None;
            match &result {
                Ok(artifacts) => {
                    self.progress = (1.0, format!("built {} app(s)", artifacts.len()));
                    if self.profile.output.open_folder {
                        if let Some(folder) = self.profile.output_folder() {
                            open_folder(&folder);
                        }
                    }
                }
                Err(_) => self.progress.1 = "build failed".into(),
            }
            self.finished = Some(result);
        }
    }

    /// Plays the app as the runtime would, without building anything.
    fn test_run(&mut self) {
        let host = if cfg!(windows) {
            if cfg!(target_arch = "aarch64") {
                Target::WindowsArm64
            } else {
                Target::WindowsX64
            }
        } else {
            Target::LinuxAppImage
        };
        let Some(runtime) = runtimes::locate(host) else {
            self.say(format!(
                "test runs need the runtime for this computer: {}",
                host.how_to_get()
            ));
            return;
        };
        let mut notes = Vec::new();
        let movie = match builder::movie_bytes(&self.profile, &mut |_, text| notes.push(text)) {
            Ok(movie) => movie,
            Err(error) => {
                self.say(error);
                return;
            }
        };
        let icon = builder::app_icon(&self.profile)
            .ok()
            .map(|icon| icon.png(256));
        let bundle = q0qir::Bundle {
            manifest: self.profile.manifest.clone(),
            icon_png: icon,
            movie,
        };
        let path = std::env::temp_dir().join(format!("q0build-test-{}.qir", std::process::id()));
        if let Err(error) = std::fs::write(&path, bundle.encode()) {
            self.say(format!("could not write the test bundle: {error}"));
            return;
        }
        match std::process::Command::new(&runtime)
            .arg("--qir")
            .arg(&path)
            .spawn()
        {
            Ok(_) => self.say(format!("test run: {}", self.profile.manifest.title())),
            Err(error) => self.say(format!("could not start {}: {error}", runtime.display())),
        }
    }

    // ------------------------------------------------------------ layout

    fn top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("title")
            .exact_height(58.0)
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let name = self
                    .profile_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map_or("untitled profile".into(), |name| {
                        name.to_string_lossy().into_owned()
                    });
                style::title_band(ui, &name, self.dirty());
            });
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::none()
                    .fill(style::skin().panel)
                    .inner_margin(egui::Margin::symmetric(10.0, 7.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if style::button(ui, "Open movie or profile…", Tone::Plain, true).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("q0s movie or Q0Build profile", &["q0s", "q1s", "q0build"])
                            .pick_file()
                        {
                            self.open_any(&path);
                        }
                    }
                    if style::button(
                        ui,
                        "Save profile",
                        Tone::Plain,
                        self.dirty() || self.profile_path.is_none(),
                    )
                    .clicked()
                    {
                        self.save_profile();
                    }
                    if style::button(ui, "New", Tone::Plain, true).clicked() {
                        *self = Self::new(ui.ctx(), None);
                    }
                    ui.separator();
                    if style::button(
                        ui,
                        "▶  Test run",
                        Tone::Accent,
                        self.profile.movie.is_some(),
                    )
                    .clicked()
                    {
                        self.test_run();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let count = builder::targets(&self.profile).len();
                        let ready =
                            self.running.is_none() && self.profile.movie.is_some() && count > 0;
                        let label = if self.running.is_some() {
                            "Building…".to_string()
                        } else {
                            format!("Build {count} app{}", if count == 1 { "" } else { "s" })
                        };
                        if style::button(ui, &label, Tone::Go, ready).clicked() {
                            self.start_build();
                        }
                        let problems = self.profile.manifest.problems().len();
                        if problems > 0 {
                            style::pill(ui, &format!("{problems} to fix"), style::warn());
                        }
                    });
                });
            });
    }

    fn navigation(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(196.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(style::skin().panel)
                    .inner_margin(egui::Margin::symmetric(8.0, 8.0)),
            )
            .show(ctx, |ui| {
                style::nav_caption(ui, "App");
                let target_count = builder::targets(&self.profile).len().to_string();
                for (page, glyph, label) in Page::SETTINGS {
                    let badge = match page {
                        Page::App if !self.profile.manifest.problems().is_empty() => {
                            Some(("fix", style::warn()))
                        }
                        Page::Targets => Some((target_count.as_str(), style::skin().accent)),
                        Page::Project if self.profile.movie.is_none() => Some(("!", style::warn())),
                        _ => None,
                    };
                    if style::nav_item(ui, glyph, label, self.page == page, badge).clicked() {
                        self.page = page;
                    }
                }
                style::nav_caption(ui, "Tools");
                for (page, glyph, label) in Page::TOOLS {
                    if style::nav_item(ui, glyph, label, self.page == page, None).clicked() {
                        self.page = page;
                    }
                }
            });
    }

    fn console(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(170.0)
            .min_height(90.0)
            .frame(
                egui::Frame::none()
                    .fill(style::skin().panel)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0)),
            )
            .show(ctx, |ui| {
                let failed = matches!(self.finished, Some(Err(_)));
                style::progress(
                    ui,
                    self.progress.0,
                    &self.progress.1,
                    self.running.is_some(),
                    failed,
                );
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(style::skin().field)
                    .stroke(egui::Stroke::new(1.0_f32, style::skin().edge))
                    .inner_margin(egui::Margin::same(6.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for (level, text) in &self.log {
                                    let (mark, color) = match level {
                                        Level::Info => ("·", style::skin().dim),
                                        Level::Good => ("✓", style::go()),
                                        Level::Warn => ("!", style::warn()),
                                        Level::Bad => ("✗", style::bad()),
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(RichText::new(mark).monospace().color(color));
                                        style::path_label(ui, text, 12.0);
                                    });
                                }
                                if let Some(Ok(artifacts)) = &self.finished {
                                    for artifact in artifacts {
                                        ui.horizontal(|ui| {
                                            style::pill(ui, artifact.target.slug(), style::go());
                                            ui.label(
                                                RichText::new(artifact.path.display().to_string())
                                                    .monospace()
                                                    .size(12.0),
                                            );
                                            ui.label(
                                                RichText::new(builder::size(artifact.bytes))
                                                    .color(style::skin().dim),
                                            );
                                        });
                                    }
                                    ui.horizontal(|ui| {
                                        if let Some(folder) = self.profile.output_folder() {
                                            if ui.link("Open the output folder").clicked() {
                                                open_folder(&folder);
                                            }
                                        }
                                        ui.label(
                                            RichText::new(format!("{} file(s)", artifacts.len()))
                                                .color(style::skin().dim),
                                        );
                                    });
                                }
                            });
                    });
            });
    }

    fn preview(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("preview")
            .exact_width(260.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(style::skin().panel)
                    .inner_margin(egui::Margin::same(10.0)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("PREVIEW")
                        .size(10.5)
                        .color(style::skin().dim)
                        .strong(),
                );
                ui.add_space(4.0);
                self.window_mock(ui);
                ui.add_space(10.0);
                ui.label(
                    RichText::new("TARGETS")
                        .size(10.5)
                        .color(style::skin().dim)
                        .strong(),
                );
                for target in Target::ALL {
                    let wanted = builder::targets(&self.profile).contains(&target);
                    let found = runtimes::locate(target).is_some();
                    ui.horizontal(|ui| {
                        let (text, color) = match (wanted, found) {
                            (true, true) => ("ready", style::go()),
                            (true, false) => ("no runtime", style::bad()),
                            (false, true) => ("off", style::skin().dim),
                            (false, false) => ("off", style::skin().dim),
                        };
                        style::pill(ui, text, color);
                        ui.label(RichText::new(target.label()).size(12.0));
                    });
                }
                ui.add_space(10.0);
                if let Some(Ok(report)) = &self.movie {
                    ui.label(
                        RichText::new("MOVIE")
                            .size(10.5)
                            .color(style::skin().dim)
                            .strong(),
                    );
                    ui.label(format!(
                        "{} × {} at {} fps",
                        report.stage.0, report.stage.1, report.fps
                    ));
                    ui.label(format!(
                        "{} · {} symbols · {} scripts",
                        builder::size(report.file_bytes),
                        report.symbols,
                        report.scripts
                    ));
                }
                if let Some((text, at)) = &self.toast {
                    if at.elapsed().as_secs_f32() < 6.0 {
                        ui.add_space(10.0);
                        egui::Frame::none()
                            .fill(style::skin().sheet)
                            .stroke(egui::Stroke::new(1.0_f32, style::skin().accent))
                            .rounding(3.0)
                            .inner_margin(egui::Margin::same(8.0))
                            .show(ui, |ui| ui.label(RichText::new(text).size(12.0)));
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(500));
                    }
                }
            });
    }

    /// A small picture of the app's window as configured.
    fn window_mock(&self, ui: &mut egui::Ui) {
        let manifest = &self.profile.manifest;
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 170.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, style::skin().field);
        let aspect = manifest.window.width.max(1) as f32 / manifest.window.height.max(1) as f32;
        let fullscreen = manifest.window.start == StartMode::Fullscreen;
        let bar = if manifest.window.decorations && !fullscreen {
            16.0
        } else {
            0.0
        };
        let room = rect.shrink(12.0);
        let mut width = room.width();
        let mut height = width / aspect + bar;
        if height > room.height() {
            height = room.height();
            width = (height - bar) * aspect;
        }
        let window = egui::Rect::from_center_size(room.center(), vec2(width, height));
        let [r, g, b] = match manifest.window.background {
            Backdrop::Black => [0, 0, 0],
            Backdrop::Custom(color) => color,
            Backdrop::Stage => self
                .movie
                .as_ref()
                .and_then(|movie| movie.as_ref().ok())
                .map_or([0, 0, 0], |report| report.background),
        };
        painter.rect_filled(window, 2.0, Color32::from_rgb(r, g, b));
        if bar > 0.0 {
            let title = egui::Rect::from_min_size(window.min, vec2(window.width(), bar));
            style::vgradient(
                &painter,
                title,
                style::skin().title[0],
                style::skin().title[1],
            );
            if let Some((_, texture)) = self.icon.as_ref().and_then(|icon| icon.textures.first()) {
                painter.image(
                    texture.id(),
                    egui::Rect::from_min_size(title.min + vec2(3.0, 2.0), vec2(12.0, 12.0)),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            painter.text(
                title.left_center() + vec2(18.0, 0.0),
                egui::Align2::LEFT_CENTER,
                manifest.title(),
                egui::FontId::proportional(10.0),
                style::skin().title_text,
            );
            for (index, color) in [style::bad(), style::skin().dim, style::skin().dim]
                .into_iter()
                .enumerate()
            {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        egui::pos2(title.right() - 7.0 - index as f32 * 11.0, title.center().y),
                        vec2(8.0, 8.0),
                    ),
                    1.0,
                    color,
                );
            }
        }
        painter.rect_stroke(window, 2.0, egui::Stroke::new(1.0_f32, style::skin().edge));
        let note = if fullscreen {
            "fullscreen".to_string()
        } else {
            format!("{} × {}", manifest.window.width, manifest.window.height)
        };
        painter.text(
            window.center() + vec2(0.0, bar * 0.5),
            egui::Align2::CENTER_CENTER,
            note,
            egui::FontId::proportional(12.0),
            Color32::from_white_alpha(160),
        );
    }

    // ------------------------------------------------------------ pages

    fn pick_and_open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("q0s movie or Q0Build profile", &["q0s", "q1s", "q0build"])
            .pick_file()
        {
            self.open_any(&path);
        }
    }

    fn save_profile_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Q0Build profile", &["q0build"])
            .set_file_name("app.q0build")
            .save_file()
        {
            self.profile_path = Some(path);
            self.save_profile();
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let skin = style::skin();
        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::none().fill(skin.panel).inner_margin(egui::Margin::symmetric(6.0, 2.0)))
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.add(egui::Button::new("New profile")).clicked() {
                            ui.close_menu();
                            *self = Self::new(ui.ctx(), None);
                            return;
                        }
                        if ui.add(egui::Button::new("Open movie or profile…").shortcut_text("Ctrl+O")).clicked() {
                            ui.close_menu();
                            self.pick_and_open();
                        }
                        ui.menu_button("Open recent", |ui| {
                            if self.settings.recent.is_empty() {
                                ui.label(RichText::new("nothing yet").color(style::skin().dim));
                            }
                            for path in self.settings.recent.clone() {
                                if ui.button(path.display().to_string()).clicked() {
                                    ui.close_menu();
                                    self.open_any(&path);
                                }
                            }
                        });
                        ui.separator();
                        if ui.add(egui::Button::new("Save profile").shortcut_text("Ctrl+S")).clicked() {
                            ui.close_menu();
                            self.save_profile();
                        }
                        if ui.button("Save profile as…").clicked() {
                            ui.close_menu();
                            self.save_profile_as();
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("Build", |ui| {
                        if ui.add(egui::Button::new("Build all targets").shortcut_text("Ctrl+B")).clicked() {
                            ui.close_menu();
                            self.start_build();
                        }
                        if ui.add(egui::Button::new("Test run").shortcut_text("F5")).clicked() {
                            ui.close_menu();
                            self.test_run();
                        }
                        ui.separator();
                        if ui.button("Open output folder").clicked() {
                            ui.close_menu();
                            if let Some(folder) = self.profile.output_folder() {
                                let _ = std::fs::create_dir_all(&folder);
                                open_folder(&folder);
                            }
                        }
                        if ui.button("Open runtimes folder").clicked() {
                            ui.close_menu();
                            let folder = runtimes::runtimes_folder();
                            let _ = std::fs::create_dir_all(&folder);
                            open_folder(&folder);
                        }
                    });
                    ui.menu_button("View", |ui| {
                        ui.menu_button("Theme", |ui| self.theme_menu(ui));
                        ui.menu_button("Text size", |ui| {
                            for (label, zoom) in [("Small", 0.9), ("Normal", 1.0), ("Large", 1.15), ("Larger", 1.3)] {
                                if ui.radio((self.settings.zoom() - zoom).abs() < 0.01, label).clicked() {
                                    self.settings.zoom = zoom;
                                    self.settings.save();
                                    ui.ctx().set_zoom_factor(zoom);
                                }
                            }
                        });
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("QIR and Q0Build guide").clicked() {
                            ui.close_menu();
                            ui.ctx().open_url(egui::OpenUrl::new_tab(
                                "https://github.com/q0s0id/q0idmation/blob/main/docs/qir.md",
                            ));
                        }
                        if ui.button("q0lang: the q0.app library").clicked() {
                            ui.close_menu();
                            ui.ctx().open_url(egui::OpenUrl::new_tab(
                                "https://github.com/q0s0id/q0idmation/blob/main/docs/q0lang/builtins.md#q0app",
                            ));
                        }
                        ui.separator();
                        if ui.button("About Q0Build").clicked() {
                            ui.close_menu();
                            self.about = true;
                        }
                    });
                });
            });
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.pick_and_open();
        }
    }

    /// Q0Build's own skins, then the shared q0 themes, each with a swatch.
    fn theme_menu(&mut self, ui: &mut egui::Ui) {
        let current = style::skin().id;
        for (caption, own) in [("Q0Build", true), ("q0 themes", false)] {
            ui.label(RichText::new(caption).size(11.0).color(style::skin().dim));
            for skin in style::Skin::all()
                .iter()
                .filter(|skin| skin.is_own() == own)
            {
                ui.horizontal(|ui| {
                    style::swatch(ui, skin);
                    if ui.selectable_label(skin.id == current, skin.name).clicked() {
                        style::use_skin(ui.ctx(), *skin);
                        self.settings.skin = skin.id.to_string();
                        self.settings.save();
                    }
                });
            }
            if own {
                ui.separator();
            }
        }
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let skin = style::skin();
        egui::TopBottomPanel::bottom("status")
            .exact_height(22.0)
            .frame(
                egui::Frame::none()
                    .fill(style::mix(skin.panel, skin.edge, 0.25))
                    .inner_margin(egui::Margin::symmetric(10.0, 3.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let small =
                        |text: String, color: Color32| RichText::new(text).size(11.5).color(color);
                    let movie = self
                        .profile
                        .movie
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map_or("no movie".into(), |name| {
                            name.to_string_lossy().into_owned()
                        });
                    ui.label(small(movie, skin.text));
                    ui.separator();
                    let app = &self.profile.manifest.app;
                    ui.label(small(format!("{} {}", app.name, app.version), skin.dim));
                    ui.separator();
                    let targets = builder::targets(&self.profile).len();
                    ui.label(small(format!("{targets} target(s)"), skin.dim));
                    match &self.finished {
                        Some(Ok(artifacts)) => {
                            ui.separator();
                            ui.label(small(
                                format!("last build: {} app(s)", artifacts.len()),
                                style::go(),
                            ));
                        }
                        Some(Err(_)) => {
                            ui.separator();
                            ui.label(small("last build failed".into(), style::bad()));
                        }
                        None => {}
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(small(skin.name.to_string(), skin.dim));
                    });
                });
            });
    }

    /// Before a movie is chosen: a place to drop one, and the last profiles.
    fn start_page(&mut self, ui: &mut egui::Ui) {
        let skin = style::skin();
        ui.add_space(10.0);
        let width = ui.available_width().min(640.0);
        let (rect, response) = ui.allocate_exact_size(vec2(width, 170.0), egui::Sense::click());
        let dragging = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        let painter = ui.painter_at(rect.expand(2.0));
        let lit = response.hovered() || dragging;
        painter.rect_filled(
            rect,
            6.0,
            style::mix(skin.sheet, skin.accent, if lit { 0.12 } else { 0.04 }),
        );
        let corners = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(
            &corners,
            egui::Stroke::new(1.5_f32, if lit { skin.accent } else { skin.dim }),
            9.0,
            6.0,
        ));
        let mark = egui::Rect::from_center_size(rect.center() - vec2(0.0, 30.0), vec2(48.0, 48.0));
        style::glyph(&painter, mark, Glyph::Package, skin.text);
        painter.text(
            rect.center() + vec2(0.0, 22.0),
            egui::Align2::CENTER_CENTER,
            "Drop a q0s movie here",
            egui::FontId::proportional(18.0),
            skin.text,
        );
        painter.text(
            rect.center() + vec2(0.0, 46.0),
            egui::Align2::CENTER_CENTER,
            "or click to choose a .q0s or .q1s file",
            egui::FontId::proportional(12.0),
            skin.dim,
        );
        if response.clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("q0s movie", &["q0s", "q1s"])
                .pick_file()
            {
                self.open_any(&path);
            }
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if !self.settings.recent.is_empty() {
            let mut open = None;
            style::group(ui, "Recent profiles", |ui| {
                for path in &self.settings.recent {
                    ui.horizontal(|ui| {
                        let name = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        if ui.link(name).clicked() {
                            open = Some(path.clone());
                        }
                        if let Some(folder) = path.parent() {
                            style::path_label(ui, &folder.display().to_string(), 11.0);
                        }
                    });
                }
            });
            if let Some(path) = open {
                self.open_any(&path);
            }
        }
        style::group(ui, "How it works", |ui| {
            for (number, step) in [
                "Choose the movie: its settings are kept beside it as a .q0build profile.",
                "Name the app and set its window, runtime and save folder on the pages at the left.",
                "Tick the targets: Windows, Linux and Android each need their q0player runtime.",
                "Test run (F5) plays it as it will be; Build (Ctrl+B) writes and checks the apps.",
            ]
            .iter()
            .enumerate()
            {
                ui.horizontal(|ui| {
                    style::pill(ui, &(number + 1).to_string(), skin.accent);
                    ui.label(*step);
                });
            }
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.about;
        egui::Window::new("About Q0Build")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), egui::Sense::hover());
                    style::glyph(ui.painter(), rect, Glyph::Package, style::skin().text);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Q0Build").size(22.0).strong().color(style::skin().text));
                        ui.label(format!("q0idmation {}", env!("CARGO_PKG_VERSION")));
                    });
                });
                ui.add_space(6.0);
                ui.label("Packs q0s movies into Windows, Linux and Android apps: QIR, the q0 Integrated Runtime.");
                ui.hyperlink_to("q0id.com", "https://q0id.com");
                ui.hyperlink_to("q0idmation on GitHub", "https://github.com/q0s0id/q0idmation");
            });
        self.about = open;
    }

    fn page_project(&mut self, ui: &mut egui::Ui) {
        if self.profile.movie.is_none() {
            self.start_page(ui);
            return;
        }
        style::group(ui, "Movie", |ui| {
            style::row(
                ui,
                "Movie file",
                "The .q0s or .q1s the app plays. Its stage size starts the window size.",
                |ui| {
                    let text = self
                        .profile
                        .movie
                        .as_ref()
                        .map_or("none".into(), |path| path.display().to_string());
                    style::path_label(ui, &text, 12.0);
                    if style::button(ui, "Choose…", Tone::Plain, true).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("q0s movie", &["q0s", "q1s"])
                            .pick_file()
                        {
                            self.profile.movie = Some(path);
                        }
                    }
                },
            );
            match &self.movie {
                Some(Ok(report)) => {
                    ui.horizontal(|ui| {
                        ui.add_space(178.0);
                        style::pill(ui, "reads fine", style::go());
                        ui.label(format!(
                            "{} · {} × {} · {} fps · {} scene(s) · {}",
                            report.name,
                            report.stage.0,
                            report.stage.1,
                            report.fps,
                            report.scenes.len(),
                            builder::size(report.file_bytes)
                        ));
                    });
                }
                Some(Err(error)) => {
                    ui.horizontal(|ui| {
                        ui.add_space(178.0);
                        ui.add(
                            egui::Label::new(RichText::new(error).color(style::warn())).wrap(true),
                        );
                    });
                }
                None => {}
            }
            style::row(ui, "Re-pack before packing", "Smaller apps: the movie is saved packed inside the bundle. Off embeds the file byte for byte.", |ui| {
                ui.checkbox(&mut self.profile.repack, "");
            });
        });
        style::group(ui, "Icon", |ui| {
            style::row(ui, "Icon picture", "PNG, ideally 256 × 256 or larger. Used for the window, the exe, the Linux menu and the phone launcher.", |ui| {
                let text = self.profile.icon.as_ref().map_or("none (a plain one is made)".into(), |path| path.display().to_string());
                style::path_label(ui, &text, 12.0);
                if style::button(ui, "Choose…", Tone::Plain, true).clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Image", &["png"]).pick_file() {
                        self.profile.icon = Some(path);
                    }
                }
                if self.profile.icon.is_some() && style::button(ui, "Clear", Tone::Plain, true).clicked() {
                    self.profile.icon = None;
                }
            });
            if let Some(icon) = &self.icon {
                ui.horizontal(|ui| {
                    ui.add_space(178.0);
                    for (side, texture) in icon.textures.iter().filter(|(side, _)| *side <= 64) {
                        ui.image((texture.id(), vec2(*side as f32, *side as f32)));
                    }
                });
            }
        });
        style::group(ui, "Profile", |ui| {
            style::row(ui, "Profile file", "Everything on these pages. Kept beside the movie, so the next build starts where this one left off.", |ui| {
                let text = self.profile_path.as_ref().map_or("not saved yet".into(), |path| path.display().to_string());
                style::path_label(ui, &text, 12.0);
            });
        });
    }

    fn page_app(&mut self, ui: &mut egui::Ui) {
        let problems = self.profile.manifest.problems();
        if !problems.is_empty() {
            style::group(ui, "To fix", |ui| {
                for problem in &problems {
                    ui.label(RichText::new(format!("• {problem}")).color(style::warn()));
                }
            });
        }
        let app = &mut self.profile.manifest.app;
        style::group(ui, "Who the app is", |ui| {
            style::row(ui, "Name", "Shown in the title bar, the taskbar, the Linux menu and under the phone icon. Scripts read it as app.name.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.name).desired_width(280.0));
            });
            style::row(ui, "App id", "Reverse-domain, like com.author.game. It is the Android package and the save folder: keep it once people have the app.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.id).desired_width(280.0).font(egui::TextStyle::Monospace));
                if style::button(ui, "Suggest", Tone::Plain, true).clicked() {
                    app.id = default_app_id(&app.name, &app.author);
                }
            });
            style::row(
                ui,
                "Version",
                "major.minor.patch. Android needs a higher one to install an update over the old.",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.version).desired_width(90.0));
                    for (label, part) in [("+ patch", 2), ("+ minor", 1), ("+ major", 0)] {
                        if style::button(ui, label, Tone::Plain, true).clicked() {
                            app.version = bump(&app.version, part);
                        }
                    }
                },
            );
            style::row(
                ui,
                "Author",
                "Company name in the exe's details; also used by Suggest.",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.author).desired_width(280.0));
                },
            );
        });
        style::group(ui, "Details", |ui| {
            style::row(
                ui,
                "Description",
                "One line: the exe's description, the Linux menu comment.",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.description).desired_width(380.0));
                },
            );
            style::row(ui, "Copyright", "The exe's copyright line.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.copyright).desired_width(280.0));
                if style::button(ui, "© year author", Tone::Plain, !app.author.is_empty()).clicked()
                {
                    app.copyright = format!("© {} {}", &builder::today()[..4], app.author);
                }
            });
            style::row(
                ui,
                "Website",
                "Kept in the manifest for the Bundle reader and your players.",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.website).desired_width(280.0));
                },
            );
        });
    }

    fn page_window(&mut self, ui: &mut egui::Ui) {
        let stage = self
            .movie
            .as_ref()
            .and_then(|movie| movie.as_ref().ok())
            .map(|report| report.stage);
        let window = &mut self.profile.manifest.window;
        style::group(ui, "Size", |ui| {
            style::row(
                ui,
                "Window size",
                "Inside the frame, in screen points. The movie scales to fit it.",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut window.width)
                            .clamp_range(64..=7680)
                            .suffix(" w"),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut window.height)
                            .clamp_range(64..=4320)
                            .suffix(" h"),
                    );
                },
            );
            ui.horizontal(|ui| {
                ui.add_space(178.0);
                if let Some((w, h)) = stage {
                    if style::button(ui, &format!("Stage {w}×{h}"), Tone::Plain, true).clicked() {
                        window.width = u32::from(w).max(64);
                        window.height = u32::from(h).max(64);
                    }
                }
                for (w, h) in [(960, 540), (1280, 720), (1920, 1080), (800, 600)] {
                    if style::button(ui, &format!("{w}×{h}"), Tone::Plain, true).clicked() {
                        window.width = w;
                        window.height = h;
                    }
                }
            });
            style::row(
                ui,
                "Smallest size",
                "How small people can drag the window.",
                |ui| {
                    ui.add(egui::DragValue::new(&mut window.min_width).clamp_range(64..=7680));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut window.min_height).clamp_range(64..=4320));
                },
            );
            style::row(
                ui,
                "Resizable",
                "Off keeps the window exactly the size above.",
                |ui| {
                    ui.checkbox(&mut window.resizable, "");
                },
            );
        });
        style::group(ui, "Start and frame", |ui| {
            style::row(
                ui,
                "Opens",
                "Fullscreen is borderless over the whole screen; F11 or the movie can leave it.",
                |ui| {
                    ui.selectable_value(&mut window.start, StartMode::Windowed, "Windowed");
                    ui.selectable_value(&mut window.start, StartMode::Maximized, "Maximized");
                    ui.selectable_value(&mut window.start, StartMode::Fullscreen, "Fullscreen");
                },
            );
            style::row(
                ui,
                "Centred",
                "Opens in the middle of the screen instead of where the system puts it.",
                |ui| {
                    ui.checkbox(&mut window.centered, "");
                },
            );
            style::row(ui, "System frame", "The title bar and buttons. Off suits apps that draw their own (q0window.move! can drag them).", |ui| {
                ui.checkbox(&mut window.decorations, "");
            });
            style::row(
                ui,
                "Always on top",
                "Stays above other windows: overlays and desktop pets.",
                |ui| {
                    ui.checkbox(&mut window.always_on_top, "");
                },
            );
            style::row(
                ui,
                "Title",
                "Empty uses the app name. The movie can change it with q0window.title!.",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut window.title)
                            .hint_text(&self.profile.manifest.app.name)
                            .desired_width(280.0),
                    );
                },
            );
        });
        let background = &mut self.profile.manifest.window.background;
        style::group(ui, "Around the movie", |ui| {
            style::row(
                ui,
                "Background",
                "Fills the window around the stage when their shapes differ. Scripts change it any time with q0window.background! \"#202020\" (or \"stage\", or r, g, b).",
                |ui| {
                    let custom = matches!(background, Backdrop::Custom(_));
                    if ui.selectable_label(*background == Backdrop::Black, "Black").clicked() {
                        *background = Backdrop::Black;
                    }
                    if ui.selectable_label(*background == Backdrop::Stage, "Stage colour").clicked() {
                        *background = Backdrop::Stage;
                    }
                    if ui.selectable_label(custom, "Custom").clicked() && !custom {
                        *background = Backdrop::Custom([24, 24, 28]);
                    }
                    if let Backdrop::Custom(color) = background {
                        ui.color_edit_button_srgb(color);
                    }
                },
            );
        });
    }

    fn page_runtime(&mut self, ui: &mut egui::Ui) {
        let manifest = &mut self.profile.manifest;
        style::group(ui, "Picture", |ui| {
            style::row(ui, "Anti-aliasing", "Samples per pixel for vector edges. 1 for pixel art and old machines; 8 for the smoothest lines.", |ui| {
                for samples in [1_u16, 2, 4, 8] {
                    ui.selectable_value(&mut manifest.render.msaa, samples, format!("{samples}×"));
                }
            });
            style::row(
                ui,
                "V-sync",
                "Waits for the screen: no tearing. Off answers input a little sooner.",
                |ui| {
                    ui.checkbox(&mut manifest.render.vsync, "");
                },
            );
        });
        let behaviour = &mut manifest.behaviour;
        style::group(ui, "Behaviour", |ui| {
            style::row(
                ui,
                "Behind other windows",
                "Pause stops the movie and its sound until the window is back.",
                |ui| {
                    ui.selectable_value(&mut behaviour.unfocused, Unfocused::Keep, "Keep running");
                    ui.selectable_value(&mut behaviour.unfocused, Unfocused::Pause, "Pause");
                },
            );
            style::row(
                ui,
                "Esc",
                "Movie gives Esc to scripts (pause menus); the others are handled before them.",
                |ui| {
                    ui.selectable_value(&mut behaviour.escape, EscapeKey::Quit, "Quits");
                    ui.selectable_value(
                        &mut behaviour.escape,
                        EscapeKey::LeaveFullscreen,
                        "Leaves fullscreen",
                    );
                    ui.selectable_value(&mut behaviour.escape, EscapeKey::Movie, "Movie");
                },
            );
            style::row(
                ui,
                "F11 fullscreen",
                "F11 switches between the window and fullscreen.",
                |ui| {
                    ui.checkbox(&mut behaviour.fullscreen_key, "");
                },
            );
            style::row(
                ui,
                "Player keys",
                "q0player's F9 (movie only) and F4 (performance log). Off for finished apps.",
                |ui| {
                    ui.checkbox(&mut behaviour.player_hotkeys, "");
                },
            );
            style::row(
                ui,
                "Mouse cursor",
                "Hide suits games with their own cursor; idle hides it after two still seconds.",
                |ui| {
                    ui.selectable_value(&mut behaviour.cursor, CursorMode::Show, "Show");
                    ui.selectable_value(
                        &mut behaviour.cursor,
                        CursorMode::HideWhenIdle,
                        "Hide when idle",
                    );
                    ui.selectable_value(&mut behaviour.cursor, CursorMode::Hide, "Hide");
                },
            );
            style::row(
                ui,
                "One copy at a time",
                "A second launch closes at once: no two copies writing the same saves.",
                |ui| {
                    ui.checkbox(&mut behaviour.single_instance, "");
                },
            );
            style::row(
                ui,
                "Loop",
                "Starts over at the end of the timeline. Games keep it on; cartoons may not.",
                |ui| {
                    ui.checkbox(&mut behaviour.loop_playback, "");
                },
            );
            style::row(
                ui,
                "All scenes",
                "Plays the document's scenes one after another, like a film.",
                |ui| {
                    ui.checkbox(&mut behaviour.all_scenes, "");
                },
            );
        });
        let splash = &mut manifest.splash;
        style::group(ui, "Splash", |ui| {
            style::row(
                ui,
                "Splash screen",
                "Off starts the movie at once. On shows the icon and name, then fades over the running movie.",
                |ui| {
                    ui.checkbox(&mut splash.enabled, "");
                },
            );
            ui.add_enabled_ui(splash.enabled, |ui| {
                style::row(
                    ui,
                    "Length",
                    "How long it stays, the fades included.",
                    |ui| {
                        ui.add(
                            egui::Slider::new(&mut splash.duration_ms, 600..=5000).suffix(" ms"),
                        );
                    },
                );
                style::row(
                    ui,
                    "Show the icon",
                    "The app's icon above its name.",
                    |ui| {
                        ui.checkbox(&mut splash.show_icon, "");
                    },
                );
                style::row(
                    ui,
                    "Line under the name",
                    "A credit, a studio name or nothing.",
                    |ui| {
                        ui.add(egui::TextEdit::singleline(&mut splash.text).desired_width(240.0));
                    },
                );
                style::row(
                    ui,
                    "Skippable",
                    "A click, tap or key ends it early.",
                    |ui| {
                        ui.checkbox(&mut splash.skippable, "");
                    },
                );
                style::row(
                    ui,
                    "Colour",
                    "Behind the icon and name. The text turns dark on light colours.",
                    |ui| {
                        ui.color_edit_button_srgb(&mut splash.background);
                    },
                );
            });
        });
    }

    fn page_data(&mut self, ui: &mut egui::Ui) {
        let manifest = &mut self.profile.manifest;
        let folder = manifest.data_folder();
        style::group(ui, "Saves", |ui| {
            style::row(
                ui,
                "Save folder",
                "Where q0app.save! keeps values and the log goes. Empty uses the app id.",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut manifest.data.folder)
                            .hint_text(&manifest.app.id)
                            .desired_width(240.0),
                    );
                },
            );
            style::row(
                ui,
                "Portable",
                "Saves next to the app instead of the user's data folder: for USB sticks and zips.",
                |ui| {
                    ui.checkbox(&mut manifest.data.portable, "");
                },
            );
            ui.horizontal(|ui| {
                ui.add_space(178.0);
                let place = if manifest.data.portable {
                    format!("<app folder>/{folder} data")
                } else {
                    format!("<user data>/{folder}")
                };
                ui.label(
                    RichText::new(place)
                        .monospace()
                        .size(12.0)
                        .color(style::skin().dim),
                );
            });
        });
        let diagnostics = &mut manifest.diagnostics;
        style::group(ui, "Checks and logs", |ui| {
            style::row(ui, "Check on start", "Refuses a damaged download with a message instead of playing garbage. Costs a moment on big movies.", |ui| {
                ui.checkbox(&mut diagnostics.verify_on_start, "");
            });
            style::row(
                ui,
                "Log file",
                "log.txt in the save folder: q0app.log! lines and script errors, for bug reports.",
                |ui| {
                    ui.checkbox(&mut diagnostics.log_file, "");
                },
            );
            style::row(
                ui,
                "Console (Windows)",
                "A console window beside the app showing the same lines live. For testing builds.",
                |ui| {
                    ui.checkbox(&mut diagnostics.console, "");
                },
            );
            style::row(
                ui,
                "Error boxes",
                "Script errors in a message box. Useful while testing; annoying for players.",
                |ui| {
                    ui.checkbox(&mut diagnostics.error_dialog, "");
                },
            );
            style::row(
                ui,
                "Debug build",
                "app.debug reads true, so scripts can show their own debug views.",
                |ui| {
                    ui.checkbox(&mut diagnostics.debug, "");
                },
            );
        });
        style::group(ui, "q0lang", |ui| {
            ui.label(
                RichText::new("Scripts reach the app through q0.app:").color(style::skin().dim),
            );
            ui.label(
                RichText::new("import q0.app\nq0app.save! \"best\", score     -- read back as save.best\nq0app.forget! \"best\"\nq0app.open! \"https://q0id.com\"\nq0app.log! \"level\", level\napp.name  app.version  app.id  app.platform  app.packaged  app.debug  app.args")
                    .monospace()
                    .size(12.0),
            );
        });
    }

    fn page_targets(&mut self, ui: &mut egui::Ui) {
        style::group(ui, "Build for", |ui| {
            for target in Target::ALL {
                let found = runtimes::locate(target);
                let flag = match target {
                    Target::WindowsX64 => &mut self.profile.targets.windows_x64,
                    Target::WindowsArm64 => &mut self.profile.targets.windows_arm64,
                    Target::LinuxAppImage => &mut self.profile.targets.linux_appimage,
                    Target::AndroidApk => &mut self.profile.targets.android_apk,
                };
                ui.horizontal(|ui| {
                    ui.checkbox(
                        flag,
                        RichText::new(target.label())
                            .strong()
                            .color(style::skin().text),
                    );
                    match &found {
                        Some(path) => {
                            style::pill(ui, "runtime found", style::go());
                            style::path_label(ui, &path.display().to_string(), 11.0);
                        }
                        None => style::pill(ui, "runtime missing", style::bad()),
                    }
                });
                if found.is_none() {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(24.0);
                        ui.label(
                            RichText::new(target.how_to_get())
                                .size(11.0)
                                .color(style::skin().dim),
                        );
                    });
                }
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if style::button(ui, "Open runtimes folder", Tone::Plain, true).clicked() {
                    let folder = runtimes::runtimes_folder();
                    let _ = std::fs::create_dir_all(&folder);
                    open_folder(&folder);
                }
            });
        });
        style::group(ui, "Windows", |ui| {
            style::row(ui, "Icon and file details", "Writes the icon, name, version and author into the exe, so Explorer and Task Manager show the app, not q0player.", |ui| {
                ui.checkbox(&mut self.profile.windows.file_info, "");
            });
        });
        style::group(ui, "Linux", |ui| {
            style::row(
                ui,
                "Menu category",
                "Where desktop menus list the app.",
                |ui| {
                    egui::ComboBox::from_id_source("category")
                        .selected_text(&self.profile.linux.category)
                        .show_ui(ui, |ui| {
                            for category in
                                ["Game", "AudioVideo", "Graphics", "Education", "Utility"]
                            {
                                ui.selectable_value(
                                    &mut self.profile.linux.category,
                                    category.to_string(),
                                    category,
                                );
                            }
                        });
                },
            );
            ui.label(
                RichText::new("AppImages need chmod +x after downloading on Linux.")
                    .size(11.0)
                    .color(style::skin().dim),
            );
        });
        style::group(ui, "Android", |ui| {
            style::row(
                ui,
                "Leave out \"Open with\"",
                "q0player answers for .q0s files; an app should not. Keep this on.",
                |ui| {
                    ui.checkbox(&mut self.profile.android.drop_open_with, "");
                },
            );
            let key = dirs::config_dir()
                .unwrap_or_default()
                .join("q0build")
                .join("android-signing");
            style::row(ui, "Signing key", "Made on the first APK. Back the folder up: updates must be signed with the same key.", |ui| {
                style::path_label(ui, &key.display().to_string(), 11.0);
                if style::button(ui, "Open", Tone::Plain, key.is_dir()).clicked() {
                    open_folder(&key);
                }
            });
        });
    }

    fn page_output(&mut self, ui: &mut egui::Ui) {
        let default_folder = self
            .profile
            .movie
            .as_ref()
            .and_then(|movie| movie.parent())
            .map(|folder| folder.join("build").display().to_string())
            .unwrap_or_else(|| "build/ beside the movie".into());
        let example = self.profile.file_stem("windows-x64", &builder::today());
        let output = &mut self.profile.output;
        style::group(ui, "Files", |ui| {
            style::row(ui, "Folder", "Where builds are written.", |ui| {
                let text = output
                    .folder
                    .as_ref()
                    .map_or(default_folder.clone(), |path| path.display().to_string());
                style::path_label(ui, &text, 12.0);
                if style::button(ui, "Choose…", Tone::Plain, true).clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        output.folder = Some(folder);
                    }
                }
                if output.folder.is_some()
                    && style::button(ui, "Default", Tone::Plain, true).clicked()
                {
                    output.folder = None;
                }
            });
            style::row(
                ui,
                "File names",
                "{name}, {version}, {target} and {date} are filled in.",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut output.pattern)
                            .desired_width(280.0)
                            .font(egui::TextStyle::Monospace),
                    );
                },
            );
            ui.horizontal(|ui| {
                ui.add_space(178.0);
                ui.label(
                    RichText::new(format!("{example}.exe"))
                        .monospace()
                        .size(12.0)
                        .color(style::skin().dim),
                );
            });
        });
        let output = &mut self.profile.output;
        style::group(ui, "After building", |ui| {
            style::row(
                ui,
                "Check every build",
                "Reads each result back and checks the movie inside before calling it done.",
                |ui| {
                    ui.checkbox(&mut output.verify, "");
                },
            );
            style::row(ui, "Zip each build", "A .zip beside each file, ready for Discord and itch.io (keeps the AppImage runnable).", |ui| {
                ui.checkbox(&mut output.zip, "");
            });
            style::row(
                ui,
                "Checksums",
                "SHA256SUMS.txt, for players who check what they download.",
                |ui| {
                    ui.checkbox(&mut output.checksums, "");
                },
            );
            style::row(
                ui,
                "Keep the log",
                "build-log.txt with the builds, for when something needs explaining.",
                |ui| {
                    ui.checkbox(&mut output.keep_log, "");
                },
            );
            style::row(
                ui,
                "Open the folder",
                "Shows the builds when they are done.",
                |ui| {
                    ui.checkbox(&mut output.open_folder, "");
                },
            );
        });
    }

    fn page_inspector(&mut self, ui: &mut egui::Ui) {
        style::group(ui, "Inspect a movie", |ui| {
            ui.horizontal(|ui| {
                if style::button(
                    ui,
                    "This project's movie",
                    Tone::Plain,
                    self.profile.movie.is_some(),
                )
                .clicked()
                {
                    if let Some(path) = self.profile.movie.clone() {
                        self.inspector = Some(
                            inspect::load_project(&path)
                                .map(|(project, bytes)| inspect::project_report(&project, bytes)),
                        );
                    }
                }
                if style::button(ui, "Another file…", Tone::Plain, true).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("q0s movie", &["q0s", "q1s"])
                        .pick_file()
                    {
                        self.inspector = Some(
                            inspect::load_project(&path)
                                .map(|(project, bytes)| inspect::project_report(&project, bytes)),
                        );
                    }
                }
            });
        });
        match &self.inspector {
            Some(Ok(report)) => {
                style::group(ui, "At a glance", |ui| {
                    egui::Grid::new("glance")
                        .num_columns(2)
                        .spacing(vec2(24.0, 4.0))
                        .show(ui, |ui| {
                            for (label, value) in [
                                ("Name", report.name.clone()),
                                ("File", builder::size(report.file_bytes)),
                                (
                                    "Stage",
                                    format!(
                                        "{} × {} at {} fps",
                                        report.stage.0, report.stage.1, report.fps
                                    ),
                                ),
                                ("Scenes", report.scenes.join(", ")),
                                ("Symbols", report.symbols.to_string()),
                                ("Layers", report.layers.to_string()),
                                (
                                    "Longest timeline",
                                    format!("{} frames", report.longest_timeline),
                                ),
                                ("Placements", report.placements.to_string()),
                                (
                                    "Scripts",
                                    format!("{} ({} lines)", report.scripts, report.script_lines),
                                ),
                                ("Sound clips", report.sounds.to_string()),
                            ] {
                                ui.label(RichText::new(label).color(style::skin().dim));
                                ui.label(value);
                                ui.end_row();
                            }
                        });
                });
                style::group(ui, "Where the bytes go", |ui| {
                    let total: u64 = report
                        .by_kind
                        .iter()
                        .map(|(_, _, bytes)| bytes)
                        .sum::<u64>()
                        .max(1);
                    for (kind, count, bytes) in &report.by_kind {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{kind:<9}")).monospace());
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(220.0, 12.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, style::skin().field);
                            let fill = egui::Rect::from_min_size(
                                rect.min,
                                vec2(rect.width() * (*bytes as f32 / total as f32), rect.height()),
                            );
                            style::vgradient(
                                ui.painter(),
                                fill,
                                style::skin().accent,
                                Color32::from_rgb(0x1E, 0x5A, 0x92),
                            );
                            ui.label(format!("{count} · {}", builder::size(*bytes)));
                        });
                    }
                    ui.label(
                        RichText::new("Bitmaps are counted unpacked, as they sit in memory.")
                            .size(11.0)
                            .color(style::skin().dim),
                    );
                });
                style::group(ui, "Heaviest assets", |ui| {
                    for (name, kind, bytes) in &report.largest {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(builder::size(*bytes)).monospace());
                            ui.label(RichText::new(*kind).color(style::skin().dim));
                            ui.label(name);
                        });
                    }
                });
            }
            Some(Err(error)) => {
                ui.add(egui::Label::new(RichText::new(error).color(style::warn())).wrap(true));
            }
            None => {
                ui.label(RichText::new("What a movie holds: its scenes, timelines and scripts, and which assets make it heavy.").color(style::skin().dim));
            }
        }
    }

    fn page_scripts(&mut self, ui: &mut egui::Ui) {
        style::group(ui, "Check every q0lang script", |ui| {
            ui.horizontal(|ui| {
                if style::button(
                    ui,
                    "Check this project's movie",
                    Tone::Accent,
                    self.profile.movie.is_some(),
                )
                .clicked()
                {
                    if let Some(path) = self.profile.movie.clone() {
                        self.scripts = Some(
                            inspect::load_project(&path)
                                .map(|(project, _)| inspect::check_scripts(&project)),
                        );
                    }
                }
                if style::button(ui, "Another file…", Tone::Plain, true).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("q0s movie", &["q0s", "q1s"])
                        .pick_file()
                    {
                        self.scripts = Some(
                            inspect::load_project(&path)
                                .map(|(project, _)| inspect::check_scripts(&project)),
                        );
                    }
                }
            });
            ui.label(RichText::new("Symbol and frame scripts, read by the same parser the player uses, before the players find the mistakes.").size(11.0).color(style::skin().dim));
        });
        match &self.scripts {
            Some(Ok((count, issues))) if issues.is_empty() => {
                style::group(ui, "Result", |ui| {
                    ui.horizontal(|ui| {
                        style::pill(ui, "all clear", style::go());
                        ui.label(format!("{count} script(s) read without a problem"));
                    });
                });
            }
            Some(Ok((count, issues))) => {
                let report: String = issues
                    .iter()
                    .map(|issue| {
                        format!(
                            "{} line {}: {}\n    {}\n",
                            issue.place, issue.line, issue.message, issue.text
                        )
                    })
                    .collect();
                style::group(
                    ui,
                    &format!("{} problem(s) in {count} script(s)", issues.len()),
                    |ui| {
                        if style::button(ui, "Copy report", Tone::Plain, true).clicked() {
                            ui.output_mut(|output| output.copied_text = report.clone());
                        }
                        for issue in issues {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!("{} · line {}", issue.place, issue.line))
                                    .strong()
                                    .color(style::skin().text),
                            );
                            ui.label(RichText::new(&issue.message).color(style::warn()));
                            ui.label(
                                RichText::new(&issue.text)
                                    .monospace()
                                    .size(12.0)
                                    .color(style::skin().dim),
                            );
                        }
                    },
                );
            }
            Some(Err(error)) => {
                ui.add(egui::Label::new(RichText::new(error).color(style::warn())).wrap(true));
            }
            None => {}
        }
    }

    fn page_reader(&mut self, ui: &mut egui::Ui) {
        style::group(ui, "Read a built app", |ui| {
            ui.horizontal(|ui| {
                if style::button(ui, "Open an app…", Tone::Plain, true).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("QIR app", &["exe", "AppImage", "apk", "qir"])
                        .add_filter("Any file", &["*"])
                        .pick_file()
                    {
                        let result = inspect::read_built_app(&path);
                        self.reader = Some((path, result));
                    }
                }
            });
            ui.label(RichText::new("Shows what an exe, AppImage or APK made by Q0Build carries, and gets the movie back out of it.").size(11.0).color(style::skin().dim));
        });
        let mut adopt = None;
        if let Some((path, result)) = &self.reader {
            match result {
                Ok(built) => {
                    let manifest = &built.bundle.manifest;
                    style::group(
                        ui,
                        &format!(
                            "{} · {}",
                            built.kind,
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        |ui| {
                            egui::Grid::new("built")
                                .num_columns(2)
                                .spacing(vec2(24.0, 4.0))
                                .show(ui, |ui| {
                                    for (label, value) in [
                                        (
                                            "App",
                                            format!(
                                                "{} {}",
                                                manifest.app.name, manifest.app.version
                                            ),
                                        ),
                                        ("Id", manifest.app.id.clone()),
                                        ("Author", manifest.app.author.clone()),
                                        (
                                            "Window",
                                            format!(
                                                "{} × {}, {:?}",
                                                manifest.window.width,
                                                manifest.window.height,
                                                manifest.window.start
                                            ),
                                        ),
                                        ("Movie", builder::size(built.bundle.movie.len() as u64)),
                                        ("Whole file", builder::size(built.file_bytes)),
                                        ("Checksum", "matches".into()),
                                    ] {
                                        ui.label(RichText::new(label).color(style::skin().dim));
                                        ui.label(value);
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if style::button(ui, "Save the movie…", Tone::Plain, true).clicked()
                                {
                                    if let Some(out) = rfd::FileDialog::new()
                                        .add_filter("q0s movie", &["q1s"])
                                        .set_file_name(format!(
                                            "{}.q1s",
                                            q0qir::file_safe(&manifest.app.name)
                                        ))
                                        .save_file()
                                    {
                                        let _ = std::fs::write(out, &built.bundle.movie);
                                    }
                                }
                                if style::button(ui, "Use these settings", Tone::Plain, true)
                                    .clicked()
                                {
                                    adopt = Some(manifest.clone());
                                }
                            });
                        },
                    );
                }
                Err(error) => {
                    ui.add(egui::Label::new(RichText::new(error).color(style::warn())).wrap(true));
                }
            }
        }
        if let Some(manifest) = adopt {
            self.profile.manifest = manifest;
            self.say("settings taken from the app; the movie stays as chosen");
        }
    }

    fn page_icons(&mut self, ui: &mut egui::Ui) {
        style::group(ui, "The icon at every size", |ui| {
            if let Some(icon) = &self.icon {
                for background in [
                    Color32::from_rgb(0xF2, 0xF2, 0xF2),
                    Color32::from_rgb(0x20, 0x22, 0x26),
                ] {
                    egui::Frame::none()
                        .fill(background)
                        .inner_margin(egui::Margin::same(10.0))
                        .rounding(3.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (side, texture) in &icon.textures {
                                    ui.vertical(|ui| {
                                        ui.image((texture.id(), vec2(*side as f32, *side as f32)));
                                        let ink = if background.r() > 128 {
                                            Color32::BLACK
                                        } else {
                                            Color32::WHITE
                                        };
                                        ui.label(
                                            RichText::new(format!("{side}")).size(10.0).color(ink),
                                        );
                                    });
                                }
                            });
                        });
                    ui.add_space(6.0);
                }
                for note in &icon.notes {
                    ui.label(RichText::new(format!("• {note}")).color(style::warn()));
                }
            }
            ui.horizontal(|ui| {
                if style::button(ui, "Export .ico…", Tone::Plain, true).clicked() {
                    if let Ok(icon) = builder::app_icon(&self.profile) {
                        if let Some(out) = rfd::FileDialog::new()
                            .add_filter("Windows icon", &["ico"])
                            .set_file_name("app.ico")
                            .save_file()
                        {
                            let _ = std::fs::write(out, icon.ico());
                        }
                    }
                }
                if style::button(ui, "Export 512 PNG…", Tone::Plain, true).clicked() {
                    if let Ok(icon) = builder::app_icon(&self.profile) {
                        if let Some(out) = rfd::FileDialog::new()
                            .add_filter("PNG", &["png"])
                            .set_file_name("icon-512.png")
                            .save_file()
                        {
                            let big = image::imageops::resize(
                                &icon.image,
                                512,
                                512,
                                image::imageops::FilterType::Lanczos3,
                            );
                            let _ = std::fs::write(out, crate::icons::encode_png(&big));
                        }
                    }
                }
            });
            ui.label(RichText::new("Small sizes are where icons fail: check that 16 and 32 still read. Store pages want a 512 PNG.").size(11.0).color(style::skin().dim));
        });
    }
}

impl eframe::App for Q0Build {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_build();
        self.refresh_movie();
        self.refresh_icon(ctx);
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .and_then(|file| file.path.clone())
        });
        if let Some(path) = dropped {
            self.open_any(&path);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            self.save_profile();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.start_build();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
            self.test_run();
        }
        self.menu_bar(ctx);
        self.top(ctx);
        self.navigation(ctx);
        self.status_bar(ctx);
        self.console(ctx);
        self.preview(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(style::skin().sheet)
                    .inner_margin(egui::Margin::symmetric(18.0, 12.0)),
            )
            .show(ctx, |ui| {
                let title = Page::SETTINGS
                    .iter()
                    .chain(Page::TOOLS.iter())
                    .find(|(page, _, _)| *page == self.page)
                    .map_or("", |(_, _, label)| label);
                ui.label(
                    RichText::new(title)
                        .size(20.0)
                        .strong()
                        .color(style::skin().text),
                );
                ui.label(
                    RichText::new(self.page.blurb())
                        .size(12.0)
                        .color(style::skin().dim),
                );
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Project => self.page_project(ui),
                        Page::App => self.page_app(ui),
                        Page::Window => self.page_window(ui),
                        Page::Runtime => self.page_runtime(ui),
                        Page::Data => self.page_data(ui),
                        Page::Targets => self.page_targets(ui),
                        Page::Output => self.page_output(ui),
                        Page::Inspector => self.page_inspector(ui),
                        Page::Scripts => self.page_scripts(ui),
                        Page::Reader => self.page_reader(ui),
                        Page::Icons => self.page_icons(ui),
                    });
            });
        self.about_window(ctx);
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

/// `version` with one part raised and the parts after it reset.
fn bump(version: &str, part: usize) -> String {
    let mut parts: Vec<u32> = version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    parts.resize(3, 0);
    parts[part] += 1;
    for later in parts.iter_mut().skip(part + 1) {
        *later = 0;
    }
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn open_folder(folder: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(folder).spawn();
}

#[cfg(test)]
mod tests {
    use super::bump;

    #[test]
    fn versions_bump_and_reset() {
        assert_eq!(bump("1.2.3", 2), "1.2.4");
        assert_eq!(bump("1.2.3", 1), "1.3.0");
        assert_eq!(bump("1.2.3", 0), "2.0.0");
        assert_eq!(bump("1", 2), "1.0.1");
    }
}
