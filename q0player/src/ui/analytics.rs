//! File analytics. Two flavours:
//!   * `V1` — bitmap-only legacy `.q0s`. Header info + bitmap memory cost
//!     + per-frame placement averages.
//!   * `V2` — vector `.q0s`. Header info + asset breakdown (vectors vs.
//!     bitmaps), q0rg / layer / placement counts, tween count, total
//!     anchor count (rough complexity proxy).
//!
//! Both render through one `render()` so the side panel doesn't need to
//! know which kind it has.

use std::collections::HashSet;

use q0s_format::v2::{Asset, ProjectV2};
use q0s_format::Movie;
use q0video::q0v::Q0vSpec;

use egui::{Grid, RichText, ScrollArea, Ui};

pub enum FileStats {
    V1(V1Stats),
    V2(V2Stats),
    Q0v(Q0vStats),
}

pub struct V1Stats {
    pub bytes: usize,
    pub version: u16,
    pub fps: u16,
    pub frame_count: u16,
    pub duration_sec: f32,
    pub bitmap_count: usize,
    pub total_bitmap_pixels: u64,
    pub total_bitmap_bytes: u64,
    pub placement_count: usize,
    pub avg_placements_per_frame: f32,
    pub max_placements_in_a_frame: usize,
    pub bg_rgba: (u8, u8, u8, u8),
    pub used_bitmap_count: usize,
    pub unused_bitmap_count: usize,
    pub viewport_extent: (i32, i32),
}

pub struct Q0vStats {
    pub bytes: usize,
    pub spec: Q0vSpec,
    pub duration_sec: f64,
}

pub struct V2Stats {
    pub bytes: usize,
    pub fps: u16,
    pub frame_count: u16,
    pub duration_sec: f32,
    pub stage_size: (u16, u16),
    pub q0rg_count: usize,
    pub layer_count: usize,
    pub placement_count: usize,
    pub tween_count: usize,
    pub vector_asset_count: usize,
    pub bitmap_asset_count: usize,
    pub anchor_count: usize,
    pub total_bitmap_bytes: u64,
}

impl FileStats {
    pub fn from_movie(movie: &Movie, bytes: usize) -> Self {
        let h = &movie.header;
        let frame_count = h.frame_count.max(1);
        let fps = h.fps.max(1);
        let duration_sec = frame_count as f32 / fps as f32;

        let bitmap_count = movie.bitmaps.len();
        let total_bitmap_pixels: u64 = movie
            .bitmaps
            .values()
            .map(|b| b.width as u64 * b.height as u64)
            .sum();
        let total_bitmap_bytes = total_bitmap_pixels * 4;

        let mut placement_count = 0usize;
        let mut max_placements_in_a_frame = 0usize;
        let mut used: HashSet<u16> = HashSet::new();
        let mut max_x = 0i32;
        let mut max_y = 0i32;
        for frame in &movie.placements_by_frame {
            placement_count += frame.len();
            max_placements_in_a_frame = max_placements_in_a_frame.max(frame.len());
            for pl in frame {
                used.insert(pl.bitmap_id);
                if let Some(b) = movie.bitmaps.get(&pl.bitmap_id) {
                    let dx = pl.x as i32 + (b.width as f32 * pl.scale_x) as i32;
                    let dy = pl.y as i32 + (b.height as f32 * pl.scale_y) as i32;
                    max_x = max_x.max(dx);
                    max_y = max_y.max(dy);
                }
            }
        }
        let used_bitmap_count = used.len();
        let unused_bitmap_count = bitmap_count.saturating_sub(used_bitmap_count);
        let avg = if frame_count == 0 {
            0.0
        } else {
            placement_count as f32 / frame_count as f32
        };

        FileStats::V1(V1Stats {
            bytes,
            version: h.version,
            fps,
            frame_count,
            duration_sec,
            bitmap_count,
            total_bitmap_pixels,
            total_bitmap_bytes,
            placement_count,
            avg_placements_per_frame: avg,
            max_placements_in_a_frame,
            bg_rgba: (
                movie.background.r,
                movie.background.g,
                movie.background.b,
                movie.background.a,
            ),
            used_bitmap_count,
            unused_bitmap_count,
            viewport_extent: (max_x, max_y),
        })
    }

    pub fn from_q0v(spec: Q0vSpec, bytes: usize) -> Self {
        Self::Q0v(Q0vStats {
            bytes,
            duration_sec: spec.timeline_frames as f64 / spec.fps.max(1) as f64,
            spec,
        })
    }

    pub fn from_project(project: &ProjectV2, bytes: usize) -> Self {
        let frame_count = project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == project.meta.entry_q0rg_id)
            .map(|q| q.frame_count.max(1))
            .unwrap_or(1);
        let fps = project.meta.fps.max(1);
        let duration_sec = frame_count as f32 / fps as f32;

        let mut layer_count = 0usize;
        let mut placement_count = 0usize;
        let mut tween_count = 0usize;
        for q in &project.q0rgs {
            layer_count += q.layers.len();
            for layer in &q.layers {
                placement_count += layer.placements.len();
                tween_count += layer
                    .placements
                    .iter()
                    .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
                    .count();
            }
        }

        let mut vector_asset_count = 0usize;
        let mut bitmap_asset_count = 0usize;
        let mut anchor_count = 0usize;
        let mut total_bitmap_bytes = 0u64;
        for asset in &project.assets {
            match asset {
                Asset::Vector(v) => {
                    vector_asset_count += 1;
                    anchor_count += v.paths.iter().map(|p| p.anchors.len()).sum::<usize>();
                }
                Asset::Bitmap(b) => {
                    bitmap_asset_count += 1;
                    total_bitmap_bytes += b.rgba.len() as u64;
                }
                Asset::Q0v(video) => {
                    total_bitmap_bytes += video.bytes.len() as u64;
                }
                Asset::Rig(_) => {}
            }
        }

        FileStats::V2(V2Stats {
            bytes,
            fps,
            frame_count,
            duration_sec,
            stage_size: (project.meta.stage_width, project.meta.stage_height),
            q0rg_count: project.q0rgs.len(),
            layer_count,
            placement_count,
            tween_count,
            vector_asset_count,
            bitmap_asset_count,
            anchor_count,
            total_bitmap_bytes,
        })
    }

    pub fn render(&self, ui: &mut Ui) {
        match self {
            FileStats::V1(s) => render_v1(s, ui),
            FileStats::V2(s) => render_v2(s, ui),
            FileStats::Q0v(s) => render_q0v(s, ui),
        }
    }

    pub fn viewport_hint(&self) -> Option<(u32, u32)> {
        match self {
            FileStats::V1(s) => Some((
                (s.viewport_extent.0.max(16) as u32).min(8192),
                (s.viewport_extent.1.max(16) as u32).min(8192),
            )),
            FileStats::V2(s) => Some((
                (s.stage_size.0 as u32).max(16),
                (s.stage_size.1 as u32).max(16),
            )),
            FileStats::Q0v(s) if s.spec.video => {
                Some((s.spec.width.max(16), s.spec.height.max(16)))
            }
            FileStats::Q0v(_) => Some((640, 360)),
        }
    }
}

fn render_q0v(s: &Q0vStats, ui: &mut Ui) {
    let accent = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        ui.heading("File info");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("q0v media").color(accent).small());
        });
    });
    ui.separator();
    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Grid::new("analytics_q0v")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "File size", &fmt_bytes(s.bytes as u64));
                    row(ui, "Format", "q0v v1");
                    ui.end_row();
                    section(ui, "Timing");
                    row(ui, "FPS", &s.spec.fps.to_string());
                    row(ui, "Frames", &s.spec.timeline_frames.to_string());
                    row(ui, "Duration", &format!("{:.3} s", s.duration_sec));
                    ui.end_row();
                    section(ui, "Video");
                    row(ui, "Present", if s.spec.video { "yes" } else { "no" });
                    if s.spec.video {
                        row(
                            ui,
                            "Size",
                            &format!("{}x{} px", s.spec.width, s.spec.height),
                        );
                        row(ui, "Frame payload", "indexed png");
                    }
                    ui.end_row();
                    section(ui, "Audio");
                    row(ui, "Present", if s.spec.audio { "yes" } else { "no" });
                    if s.spec.audio {
                        row(
                            ui,
                            "Sample rate",
                            &format!("{} hz", s.spec.audio_sample_rate),
                        );
                        row(ui, "Channels", &s.spec.audio_channels.to_string());
                        row(ui, "Sample format", "pcm s16le");
                    }
                });
        });
}

fn render_v1(s: &V1Stats, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("File info");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Bitmap v1").weak().small());
        });
    });
    ui.separator();
    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Grid::new("analytics_v1")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "File size", &fmt_bytes(s.bytes as u64));
                    row(ui, "Format version", &format!("v{}", s.version));
                    ui.end_row();
                    section(ui, "Timing");
                    row(ui, "FPS", &format!("{}", s.fps));
                    row(ui, "Frames", &format!("{}", s.frame_count));
                    row(ui, "Duration", &format!("{:.2} s", s.duration_sec));
                    ui.end_row();
                    section(ui, "Bitmaps");
                    row(ui, "Defined", &format!("{}", s.bitmap_count));
                    row(
                        ui,
                        "Used / unused",
                        &format!("{} / {}", s.used_bitmap_count, s.unused_bitmap_count),
                    );
                    row(ui, "Pixels (total)", &fmt_pixels(s.total_bitmap_pixels));
                    row(ui, "Memory (RGBA8)", &fmt_bytes(s.total_bitmap_bytes));
                    ui.end_row();
                    section(ui, "Placements");
                    row(ui, "Total", &format!("{}", s.placement_count));
                    row(
                        ui,
                        "Avg / frame",
                        &format!("{:.2}", s.avg_placements_per_frame),
                    );
                    row(
                        ui,
                        "Max in a frame",
                        &format!("{}", s.max_placements_in_a_frame),
                    );
                    ui.end_row();
                    section(ui, "Stage");
                    row(
                        ui,
                        "Background",
                        &format!(
                            "rgba({}, {}, {}, {})",
                            s.bg_rgba.0, s.bg_rgba.1, s.bg_rgba.2, s.bg_rgba.3
                        ),
                    );
                    row(
                        ui,
                        "Content extent",
                        &format!(
                            "{}x{} px",
                            s.viewport_extent.0.max(0),
                            s.viewport_extent.1.max(0)
                        ),
                    );
                });
        });
}

fn render_v2(s: &V2Stats, ui: &mut Ui) {
    let accent = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        ui.heading("File info");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Vector v2").color(accent).small());
        });
    });
    ui.separator();
    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Grid::new("analytics_v2")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(ui, "File size", &fmt_bytes(s.bytes as u64));
                    row(ui, "Format", "Vector v2");
                    ui.end_row();
                    section(ui, "Timing");
                    row(ui, "FPS", &format!("{}", s.fps));
                    row(ui, "Frames", &format!("{}", s.frame_count));
                    row(ui, "Duration", &format!("{:.2} s", s.duration_sec));
                    ui.end_row();
                    section(ui, "Stage");
                    row(
                        ui,
                        "Size",
                        &format!("{}x{} px", s.stage_size.0, s.stage_size.1),
                    );
                    ui.end_row();
                    section(ui, "Hierarchy");
                    row(ui, "Q0rgs (symbols)", &format!("{}", s.q0rg_count));
                    row(ui, "Layers", &format!("{}", s.layer_count));
                    row(ui, "Placements", &format!("{}", s.placement_count));
                    row(ui, "Tweens", &format!("{}", s.tween_count));
                    ui.end_row();
                    section(ui, "Assets");
                    row(ui, "Vector", &format!("{}", s.vector_asset_count));
                    row(ui, "Bitmap", &format!("{}", s.bitmap_asset_count));
                    row(ui, "Anchor points", &format!("{}", s.anchor_count));
                    row(ui, "Bitmap memory", &fmt_bytes(s.total_bitmap_bytes));
                });
        });
}

fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak().small());
    ui.label(RichText::new(value).strong().small());
    ui.end_row();
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).strong().small().underline());
    ui.label("");
    ui.end_row();
}

fn fmt_bytes(b: u64) -> String {
    const K: u64 = 1024;
    const M: u64 = 1024 * 1024;
    const G: u64 = 1024 * 1024 * 1024;
    if b >= G {
        format!("{:.2} GiB", b as f64 / G as f64)
    } else if b >= M {
        format!("{:.2} MiB", b as f64 / M as f64)
    } else if b >= K {
        format!("{:.2} KiB", b as f64 / K as f64)
    } else {
        format!("{} B", b)
    }
}

fn fmt_pixels(p: u64) -> String {
    if p >= 1_000_000 {
        format!("{:.2} M", p as f64 / 1_000_000.0)
    } else if p >= 1_000 {
        format!("{:.2} K", p as f64 / 1_000.0)
    } else {
        format!("{}", p)
    }
}
