use std::path::Path;

use egui::{Color32, RichText};
use q0s_format::v2::ProjectV2;

use crate::app::EditorApp;
use crate::q0enc::{ExportFormat, ImageCodec, JobState, Q0EncState};

pub fn render(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.q0enc.open {
        return;
    }

    app.q0enc.reconcile_with_project(&app.state.project);
    let file_path = app.state.file_path.clone();
    let mut open = app.q0enc.open;
    let mut browse_requested = false;
    let mut enqueue_requested = false;
    let mut export_now_requested = false;
    let mut start_queue_requested = false;
    let mut clear_finished_requested = false;
    let mut cancel_ids = Vec::new();
    let previous_sequence_create_subfolder = app.q0enc.sequence_create_subfolder;

    egui::Window::new("q0enc")
        .open(&mut open)
        .default_size([780.0, 660.0])
        .min_size([640.0, 500.0])
        .max_width(860.0)
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            let project = &app.state.project;
            let state = &mut app.q0enc;
            let theme = &app.settings.theme;

            ui.horizontal(|ui| {
                ui.heading("q0enc");
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} × {}  •  {} fps  •  {} frame(s)",
                        project.meta.stage_width,
                        project.meta.stage_height,
                        project.meta.fps,
                        entry_frame_count(project)
                    ))
                    .color(theme.text_dim.to_color32()),
                );
            });
            ui.add_space(5.0);
            ui.separator();

            let body_height = (ui.available_height() - 205.0).max(235.0);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(154.0, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label(
                            RichText::new("format")
                                .strong()
                                .color(theme.text.to_color32()),
                        );
                        ui.add_space(3.0);
                        for format in ExportFormat::ALL {
                            let response = ui
                                .selectable_label(state.selected_format == format, format.label());
                            if response.clicked() {
                                state.select_format(format, file_path.as_deref());
                            }
                        }
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                            ui.label(
                                RichText::new("q0enc final export workshop")
                                    .small()
                                    .color(theme.text_dim.to_color32()),
                            );
                        });
                    },
                );

                ui.separator();

                ui.allocate_ui_with_layout(
                    egui::vec2((ui.available_width() - 8.0).max(380.0), body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_source("q0enc_format_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                render_format_settings(
                                    ui,
                                    state,
                                    project,
                                    theme.text_dim.to_color32(),
                                );
                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(output_label(state.selected_format)).strong(),
                                );
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut state.output_text)
                                            .desired_width(f32::INFINITY),
                                    );
                                    if ui
                                        .add_enabled(true, egui::Button::new("browse..."))
                                        .clicked()
                                    {
                                        browse_requested = true;
                                    }
                                });

                                let validation = state.current_validation(project);
                                if let Err(message) = &validation {
                                    ui.label(RichText::new(message).small().color(
                                        if state.selected_format.implemented() {
                                            ctx.style().visuals.warn_fg_color
                                        } else {
                                            theme.text_dim.to_color32()
                                        },
                                    ));
                                }
                                ui.add_space(5.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            validation.is_ok(),
                                            egui::Button::new("add to queue"),
                                        )
                                        .clicked()
                                    {
                                        enqueue_requested = true;
                                    }
                                    if ui
                                        .add_enabled(
                                            validation.is_ok(),
                                            egui::Button::new("export now"),
                                        )
                                        .clicked()
                                    {
                                        export_now_requested = true;
                                    }
                                });
                            });
                    },
                );
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("export queue").strong());
                let pending = state
                    .queue
                    .iter()
                    .filter(|job| matches!(job.state, JobState::Pending))
                    .count();
                ui.label(
                    RichText::new(format!("{} job(s), {} pending", state.queue.len(), pending))
                        .small()
                        .color(theme.text_dim.to_color32()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            pending > 0 && !state.queue_running,
                            egui::Button::new("export queue"),
                        )
                        .clicked()
                    {
                        start_queue_requested = true;
                    }
                    if ui
                        .add_enabled(
                            state.queue.iter().any(|job| job.state.is_finished()),
                            egui::Button::new("clear finished"),
                        )
                        .clicked()
                    {
                        clear_finished_requested = true;
                    }
                });
            });

            egui::ScrollArea::vertical()
                .id_source("q0enc_queue_scroll")
                .max_height(142.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if state.queue.is_empty() {
                        ui.label(
                            RichText::new("the queue is empty")
                                .small()
                                .color(theme.text_dim.to_color32()),
                        );
                    }
                    for job in &state.queue {
                        ui.horizontal(|ui| {
                            let state_color = match &job.state {
                                JobState::Completed => Color32::from_rgb(110, 190, 125),
                                JobState::Failed(_) => ctx.style().visuals.error_fg_color,
                                JobState::Running => theme.accent.to_color32(),
                                _ => theme.text_dim.to_color32(),
                            };
                            ui.label(
                                RichText::new(format!(
                                    "#{:03}  {:<12}  {}",
                                    job.id,
                                    job.options.format.label(),
                                    job.state.label()
                                ))
                                .monospace()
                                .color(state_color),
                            );
                            ui.label(
                                RichText::new(job.output_path.display().to_string())
                                    .small()
                                    .color(theme.text_dim.to_color32()),
                            );
                            if matches!(job.state, JobState::Pending | JobState::Running)
                                && ui.small_button("cancel").clicked()
                            {
                                cancel_ids.push(job.id);
                            }
                        });
                        if matches!(job.state, JobState::Running) {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::ProgressBar::new(job.progress())
                                        .desired_width((ui.available_width() - 160.0).max(120.0))
                                        .show_percentage(),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} / {}  {}",
                                        job.completed_units, job.total_units, job.phase
                                    ))
                                    .small()
                                    .color(theme.text_dim.to_color32()),
                                );
                            });
                        } else if let JobState::Failed(error) = &job.state {
                            ui.label(
                                RichText::new(error)
                                    .small()
                                    .color(ctx.style().visuals.error_fg_color),
                            );
                        }
                    }
                });
        });

    app.q0enc.open = open;

    if browse_requested {
        if let Some(path) = browse_output(&app.q0enc, file_path.as_deref()) {
            let directory = if app.q0enc.selected_format == ExportFormat::PngSequence {
                path.clone()
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            app.q0enc.output_text = path.display().to_string();
            app.q0enc.remember_directory(directory.clone());
            app.settings.last_export_directory = Some(directory);
            app.settings.save();
        }
    }
    if app.q0enc.sequence_create_subfolder != previous_sequence_create_subfolder {
        app.settings.png_sequence_create_subfolder = app.q0enc.sequence_create_subfolder;
        app.settings.save();
    }
    for id in cancel_ids {
        app.q0enc.cancel_job(id);
    }
    if clear_finished_requested {
        app.q0enc.clear_finished();
    }
    if enqueue_requested || export_now_requested {
        match app.q0enc.enqueue(&app.state.project) {
            Ok(id) => {
                app.session.status = format!("q0enc queued export job #{id}");
                if export_now_requested {
                    app.q0enc.start_queue();
                }
            }
            Err(error) => app.session.status = format!("q0enc queue failed: {error}"),
        }
    }
    if start_queue_requested {
        app.q0enc.start_queue();
        app.session.status = "q0enc export queue started".to_string();
    }

    if app.q0enc.queue_running {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

fn render_format_settings(
    ui: &mut egui::Ui,
    state: &mut Q0EncState,
    project: &ProjectV2,
    dim: Color32,
) {
    ui.heading(state.selected_format.label());
    ui.add_space(3.0);
    match state.selected_format {
        ExportFormat::Q0s => {
            ui.label("runtime build for q0player and q0lang games.");
            ui.label(
                RichText::new(
                    "exports the complete vector project, preserves q0rgs and scripts, then verifies the written file by parsing it back.",
                )
                .color(dim),
            );
            ui.add_space(8.0);
            egui::Grid::new("q0enc_q0s_summary")
                .num_columns(2)
                .spacing([16.0, 5.0])
                .show(ui, |ui| {
                    ui.label("entry q0rg");
                    ui.label(entry_q0rg_name(project));
                    ui.end_row();
                    ui.label("assets");
                    ui.label(project.assets.len().to_string());
                    ui.end_row();
                    ui.label("symbols");
                    ui.label(project.q0rgs.len().to_string());
                    ui.end_row();
                    ui.label("verification");
                    ui.label("serialize → parse-back → atomic write → reread");
                    ui.end_row();
                });
        }
        ExportFormat::Q0v => {
            ui.label("embedded media container for q0player content.");
            ui.label(
                RichText::new(
                    "q0v is a timeline/display-object asset. it may carry video, audio, or both; both streams share one rational media clock.",
                )
                .color(dim),
            );
            ui.add_space(8.0);
            render_frame_source(ui, state, project, false);
            ui.add_space(8.0);
            ui.label(RichText::new("streams").strong());
            ui.checkbox(&mut state.q0v_streams.video, "video stream");
            ui.checkbox(&mut state.q0v_streams.audio, "audio stream");
            if state.q0v_streams.video {
                ui.checkbox(&mut state.transparent, "transparent video background");
            }
            if !state.q0v_streams.video && !state.q0v_streams.audio {
                ui.label(
                    RichText::new("q0v must contain at least one stream")
                        .color(ui.style().visuals.warn_fg_color),
                );
            }
            let clock = state.current_options().media_clock(project);
            ui.label(
                RichText::new(format!(
                    "shared clock: {} fps video, {} hz audio, {} ticks/sec",
                    clock.fps,
                    clock.audio_sample_rate,
                    crate::q0enc::MEDIA_TICKS_PER_SECOND
                ))
                .small()
                .color(dim),
            );
            if state.q0v_streams.audio {
                ui.label(
                    RichText::new(
                        "the container carries real pcm audio and seeks it with video. until audio clips land on the q0e timeline, this project exports duration-correct silence.",
                    )
                    .small()
                    .color(dim),
                );
            }
        }
        ExportFormat::Mp4 => {
            ui.label("final h.264 video for publishing, sharing and editing.");
            render_frame_source(ui, state, project, false);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("bitrate");
                let mut mbps = state.mp4_bitrate as f32 / 1_000_000.0;
                if ui
                    .add(egui::Slider::new(&mut mbps, 0.5..=50.0).suffix(" mbps"))
                    .changed()
                {
                    state.mp4_bitrate = (mbps * 1_000_000.0).round() as u32;
                }
            });
            let encoded_width = crate::q0enc::h264_encoded_dimension(state.width);
            let encoded_height = crate::q0enc::h264_encoded_dimension(state.height);
            if encoded_width != state.width || encoded_height != state.height {
                ui.label(
                    RichText::new(format!(
                        "h.264 will pad the encoded frame to {encoded_width} × {encoded_height} without scaling"
                    ))
                    .small()
                    .color(ui.style().visuals.warn_fg_color),
                );
            }
            ui.label(
                RichText::new(
                    "encoded natively through windows media foundation. the current q1s model has no audio clips yet, so this mp4 is video-only.",
                )
                .small()
                .color(dim),
            );
        }
        ExportFormat::PngSequence => {
            ui.label("one verified, lossless png per rendered frame.");
            render_frame_source(ui, state, project, false);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("filename prefix");
                ui.text_edit_singleline(&mut state.sequence_prefix);
                ui.label("_0001.png");
            });
            ui.checkbox(&mut state.transparent, "transparent background");
            ui.checkbox(
                &mut state.sequence_create_subfolder,
                "create a subfolder inside the selected folder",
            );
            if state.sequence_create_subfolder {
                ui.horizontal(|ui| {
                    ui.label("subfolder");
                    ui.text_edit_singleline(&mut state.sequence_subfolder);
                });
            }
            ui.label(
                RichText::new(
                    "all frames are staged and decode-checked first. without a subfolder, existing unrelated files stay untouched and filename conflicts are refused.",
                )
                .small()
                .color(dim),
            );
        }
        ExportFormat::Gif => {
            ui.label("small palette-based animated previews.");
            render_frame_source(ui, state, project, false);
            ui.add_space(8.0);
            ui.checkbox(&mut state.gif_loop, "loop forever");
            ui.checkbox(&mut state.transparent, "transparent background");
            ui.horizontal(|ui| {
                ui.label("quantizer speed");
                ui.add(egui::Slider::new(&mut state.gif_speed, 1..=30));
            });
            ui.label(
                RichText::new(
                    "lower speed values spend more cpu on palette quality. timing uses variable gif delays so 24/30/60 fps do not accumulate drift.",
                )
                .small()
                .color(dim),
            );
        }
        ExportFormat::Image => {
            ui.label("one offscreen-rendered still image.");
            render_frame_source(ui, state, project, true);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("codec");
                egui::ComboBox::from_id_source("q0enc_image_codec")
                    .selected_text(state.image_codec.label())
                    .show_ui(ui, |ui| {
                        for codec in ImageCodec::ALL {
                            if ui
                                .selectable_label(state.image_codec == codec, codec.label())
                                .clicked()
                            {
                                state.select_image_codec(codec);
                            }
                        }
                    });
            });
            if state.image_codec == ImageCodec::Jpeg {
                ui.horizontal(|ui| {
                    ui.label("jpeg quality");
                    ui.add(egui::Slider::new(&mut state.jpeg_quality, 1..=100));
                });
                ui.label(
                    RichText::new("jpeg is always composited onto white; it has no alpha channel.")
                        .small()
                        .color(dim),
                );
            } else {
                ui.checkbox(&mut state.transparent, "transparent background");
            }
            ui.label(
                RichText::new(
                    "the encoded file is decoded back and dimension-checked before atomic replacement.",
                )
                .small()
                .color(dim),
            );
        }
    }
}

fn render_frame_source(
    ui: &mut egui::Ui,
    state: &mut Q0EncState,
    project: &ProjectV2,
    single_frame: bool,
) {
    ui.add_space(6.0);
    ui.label(RichText::new("source").strong());
    egui::ComboBox::from_id_source("q0enc_source_q0rg")
        .selected_text(q0rg_label(project, state.source_q0rg_id))
        .show_ui(ui, |ui| {
            for q0rg in &project.q0rgs {
                if ui
                    .selectable_value(
                        &mut state.source_q0rg_id,
                        q0rg.q0rg_id,
                        format!("{}  [q0rg {}]", q0rg.name, q0rg.q0rg_id),
                    )
                    .changed()
                {
                    state.first_frame = 0;
                    state.last_frame = crate::q0enc::source_last_frame(project, q0rg.q0rg_id);
                }
            }
        });

    let max_frame = crate::q0enc::source_last_frame(project, state.source_q0rg_id);
    if single_frame {
        state.entire_timeline = false;
        state.last_frame = state.first_frame;
        ui.horizontal(|ui| {
            let mut frame = u32::from(state.first_frame) + 1;
            ui.label("frame");
            if ui
                .add(egui::DragValue::new(&mut frame).clamp_range(1..=u32::from(max_frame) + 1))
                .changed()
            {
                state.first_frame = frame.saturating_sub(1) as u16;
                state.last_frame = state.first_frame;
            }
        });
    } else {
        ui.checkbox(&mut state.entire_timeline, "entire timeline");
        ui.add_enabled_ui(!state.entire_timeline, |ui| {
            ui.horizontal(|ui| {
                let mut first = u32::from(state.first_frame) + 1;
                let mut last = u32::from(state.last_frame) + 1;
                ui.label("frames");
                if ui
                    .add(egui::DragValue::new(&mut first).clamp_range(1..=u32::from(max_frame) + 1))
                    .changed()
                {
                    state.first_frame = first.saturating_sub(1) as u16;
                    state.last_frame = state.last_frame.max(state.first_frame);
                }
                ui.label("to");
                if ui
                    .add(egui::DragValue::new(&mut last).clamp_range(1..=u32::from(max_frame) + 1))
                    .changed()
                {
                    state.last_frame = last.saturating_sub(1) as u16;
                    state.first_frame = state.first_frame.min(state.last_frame);
                }
            });
        });
    }

    ui.add_space(6.0);
    ui.label(RichText::new("render").strong());
    ui.horizontal(|ui| {
        ui.label("size");
        ui.add(egui::DragValue::new(&mut state.width).clamp_range(1..=16_384));
        ui.label("×");
        ui.add(egui::DragValue::new(&mut state.height).clamp_range(1..=16_384));
    });
    ui.horizontal(|ui| {
        ui.label("antialiasing");
        egui::ComboBox::from_id_source("q0enc_supersampling")
            .selected_text(format!("{}x", state.supersampling))
            .show_ui(ui, |ui| {
                for factor in [1_u8, 2, 4] {
                    ui.selectable_value(&mut state.supersampling, factor, format!("{factor}x"));
                }
            });
    });
}

fn browse_output(state: &Q0EncState, suggested: Option<&Path>) -> Option<std::path::PathBuf> {
    let current = Path::new(state.output_text.trim());
    let preferred = state
        .last_export_directory
        .as_deref()
        .or_else(|| {
            (state.selected_format == ExportFormat::PngSequence && current.is_dir())
                .then_some(current)
        })
        .or_else(|| current.parent())
        .or_else(|| suggested.and_then(Path::parent));
    let mut dialog = rfd::FileDialog::new();
    if let Some(directory) = preferred.filter(|path| !path.as_os_str().is_empty()) {
        dialog = dialog.set_directory(directory);
    }
    if state.selected_format == ExportFormat::PngSequence {
        return dialog.pick_folder();
    }
    if let Some(name) = current.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(name);
    }
    match state.selected_format {
        ExportFormat::Q0s => dialog.add_filter("q0s movie", &["q0s"]).save_file(),
        ExportFormat::Mp4 => dialog.add_filter("mp4 video", &["mp4"]).save_file(),
        ExportFormat::Q0v => dialog.add_filter("q0v media", &["q0v"]).save_file(),
        ExportFormat::Gif => dialog.add_filter("gif animation", &["gif"]).save_file(),
        ExportFormat::Image => match state.image_codec {
            ImageCodec::Png => dialog.add_filter("png image", &["png"]).save_file(),
            ImageCodec::Jpeg => dialog
                .add_filter("jpeg image", &["jpg", "jpeg"])
                .save_file(),
            ImageCodec::WebP => dialog.add_filter("webp image", &["webp"]).save_file(),
        },
        ExportFormat::PngSequence => unreachable!("handled by folder picker"),
    }
}

fn output_label(format: ExportFormat) -> &'static str {
    if format == ExportFormat::PngSequence {
        "selected folder"
    } else {
        "output"
    }
}

fn q0rg_label(project: &ProjectV2, id: u16) -> String {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == id)
        .map(|q0rg| format!("{}  [q0rg {}]", q0rg.name, q0rg.q0rg_id))
        .unwrap_or_else(|| format!("missing q0rg {id}"))
}

fn entry_q0rg_name(project: &ProjectV2) -> String {
    q0rg_label(project, project.meta.entry_q0rg_id)
}

fn entry_frame_count(project: &ProjectV2) -> u16 {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == project.meta.entry_q0rg_id)
        .map(|q0rg| q0rg.frame_count.max(1))
        .unwrap_or(1)
}

#[allow(dead_code)]
fn path_has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}
