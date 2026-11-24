//! The Create project dialog: what the stage will be, how fast and how long
//! the timeline runs, and what colour sits behind it, with the stage itself
//! shown beside the choices. It grows into each new size, a ball bounces on
//! it at the frame rate chosen, and a timeline under it plays the length.

use egui::{pos2, vec2, Align2, Color32, Context, FontId, Rect, RichText, Sense, Stroke, Ui};

use crate::app::NewProjectSpec;
use crate::ui::FitToScreen;

/// Stage sizes to pick from: name, width, height.
const SIZES: [(&str, u16, u16); 6] = [
    ("Classic", 640, 480),
    ("HD", 1280, 720),
    ("Full HD", 1920, 1080),
    ("4K", 3840, 2160),
    ("Portrait", 1080, 1920),
    ("Square", 1080, 1080),
];

/// Frame rates to pick from, with what each is for.
const RATES: [(u16, &str); 5] = [
    (12, "on twos"),
    (24, "film"),
    (25, "PAL"),
    (30, "video"),
    (60, "smooth"),
];

/// Lengths to pick from, in seconds.
const LENGTHS: [u16; 4] = [1, 2, 5, 10];

/// Backgrounds to pick from.
const BACKGROUNDS: [(&str, [u8; 3]); 5] = [
    ("White", [255, 255, 255]),
    ("Paper", [244, 238, 226]),
    ("Grey", [128, 128, 132]),
    ("Night", [26, 28, 36]),
    ("Black", [0, 0, 0]),
];

/// What the person did with the dialog this frame.
pub(crate) enum Outcome {
    Create,
    Cancel,
    Keep,
}

/// The theme colours the dialog draws with.
pub(crate) struct Colors {
    pub accent: Color32,
    pub dim: Color32,
}

/// Draws the dialog over `draft`; says whether to create, cancel or keep
/// it open.
pub(crate) fn show(ctx: &Context, draft: &mut NewProjectSpec, colors: &Colors) -> Outcome {
    let mut open = true;
    let mut outcome = Outcome::Keep;
    // It moves and stretches like any window; the choices scroll inside it
    // and the buttons stay put along its bottom, whatever its size.
    let screen = ctx.screen_rect();
    let size = vec2(
        780.0_f32.min(screen.width() - 40.0),
        640.0_f32.min(screen.height() - 80.0),
    );
    crate::dialog::Dialog::new("Create project")
        .default_pos(screen.center() - size * 0.5)
        .default_size(size)
        .min_size([420.0, 380.0])
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .vscroll(false)
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Every story starts with a stage.")
                    .strong()
                    .size(24.0),
            );
            ui.label(
                RichText::new(
                    "Pick its size, how fast and how long it plays, and what sits behind it.",
                )
                .color(colors.dim),
            );
            ui.add_space(12.0);
            // Everything but the buttons, which keep their room below.
            let body = (ui.available_height() - 56.0).max(80.0);
            egui::ScrollArea::vertical()
                .id_source("new_project_choices")
                .max_height(body)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let width = ui.available_width();
                    if width >= 640.0 {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width((width * 0.56).clamp(360.0, 560.0));
                                choices(ui, draft, colors);
                            });
                            ui.add_space(14.0);
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                preview(ui, draft, colors);
                            });
                        });
                    } else {
                        preview(ui, draft, colors);
                        ui.add_space(10.0);
                        choices(ui, draft, colors);
                    }
                });
            crate::dialog::footer(ui, |ui| {
                if crate::dialog::button(ui, "Create project", crate::dialog::Kind::Primary)
                    .on_hover_text("Enter")
                    .clicked()
                {
                    outcome = Outcome::Create;
                }
                if crate::dialog::button(ui, "Cancel", crate::dialog::Kind::Secondary).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
            let (enter, escape) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Enter),
                    input.key_pressed(egui::Key::Escape),
                )
            });
            // Enter creates, from the name field too.
            if enter {
                outcome = Outcome::Create;
            }
            if escape && !ui.ctx().wants_keyboard_input() {
                outcome = Outcome::Cancel;
            }
        });
    if !open {
        Outcome::Cancel
    } else {
        outcome
    }
}

fn section(ui: &mut Ui, title: &str, colors: &Colors, detail: &str) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(14.0));
        if !detail.is_empty() {
            ui.label(RichText::new(detail).small().color(colors.dim));
        }
    });
    ui.add_space(4.0);
}

fn choices(ui: &mut Ui, draft: &mut NewProjectSpec, colors: &Colors) {
    ui.label(RichText::new("Name").small().color(colors.dim));
    ui.add(
        egui::TextEdit::singleline(&mut draft.name)
            .hint_text("untitled")
            .font(FontId::proportional(17.0))
            .desired_width(f32::INFINITY)
            .margin(vec2(10.0, 8.0)),
    );

    section(ui, "Stage", colors, "");
    let gap = 8.0;
    let width = ui.available_width();
    let card = vec2((width - gap * 2.0) / 3.0, 58.0);
    let (grid, _) = ui.allocate_exact_size(vec2(width, card.y * 2.0 + gap), Sense::hover());
    for (index, (name, w, h)) in SIZES.iter().enumerate() {
        let rect = Rect::from_min_size(
            grid.min
                + vec2(
                    (index % 3) as f32 * (card.x + gap),
                    (index / 3) as f32 * (card.y + gap),
                ),
            card,
        );
        let chosen = draft.width == *w && draft.height == *h;
        if size_card(ui, rect, name, *w, *h, chosen, colors).clicked() {
            draft.width = *w;
            draft.height = *h;
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut draft.width)
                .clamp_range(1..=8192)
                .suffix(" px"),
        );
        if swap_button(ui, colors)
            .on_hover_text("Turn it round: width and height swap")
            .clicked()
        {
            std::mem::swap(&mut draft.width, &mut draft.height);
        }
        ui.add(
            egui::DragValue::new(&mut draft.height)
                .clamp_range(1..=8192)
                .suffix(" px"),
        );
        if !SIZES
            .iter()
            .any(|(_, w, h)| *w == draft.width && *h == draft.height)
        {
            ui.label(RichText::new("custom").small().color(colors.accent));
        }
    });

    section(ui, "Frame rate", colors, "frames a second");
    ui.horizontal_wrapped(|ui| {
        for (fps, what) in RATES {
            if chip(ui, &format!("{fps}"), what, draft.fps == fps, colors).clicked() {
                draft.fps = fps;
            }
        }
        ui.add(
            egui::DragValue::new(&mut draft.fps)
                .clamp_range(1..=240)
                .suffix(" fps"),
        );
    });

    section(
        ui,
        "Length",
        colors,
        "the first timeline; it grows as you animate",
    );
    ui.horizontal_wrapped(|ui| {
        for seconds in LENGTHS {
            let frames = seconds.saturating_mul(draft.fps.max(1));
            let label = format!("{seconds} s");
            if chip(
                ui,
                &label,
                &format!("{frames} frames"),
                draft.frame_count == frames,
                colors,
            )
            .clicked()
            {
                draft.frame_count = frames;
            }
        }
        ui.add(
            egui::DragValue::new(&mut draft.frame_count)
                .clamp_range(1..=u16::MAX)
                .suffix(" frames"),
        );
    });

    section(ui, "Background", colors, "");
    ui.horizontal_wrapped(|ui| {
        let current = [draft.background.r, draft.background.g, draft.background.b];
        for (name, rgb) in BACKGROUNDS {
            if swatch(ui, name, rgb, current == rgb, colors).clicked() {
                (draft.background.r, draft.background.g, draft.background.b) =
                    (rgb[0], rgb[1], rgb[2]);
            }
        }
        let mut picked = current;
        if crate::color_picker::color_edit_button_srgb(ui, &mut picked)
            .on_hover_text("Any colour")
            .changed()
        {
            (draft.background.r, draft.background.g, draft.background.b) =
                (picked[0], picked[1], picked[2]);
        }
    });
}

/// A stage size to pick: its shape, its name and its pixels.
fn size_card(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    w: u16,
    h: u16,
    chosen: bool,
    colors: &Colors,
) -> egui::Response {
    let response = ui
        .interact(
            rect,
            ui.id().with(("new_project_size", name)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.12);
    let pick = ctx.animate_bool_with_time(response.id.with("pick"), chosen, 0.18);
    let visuals = ui.visuals();
    let painter = ui.painter();
    let fill = crate::ui::blend(
        crate::ui::blend(
            visuals.widgets.inactive.weak_bg_fill,
            colors.accent,
            0.06 * hover,
        ),
        colors.accent,
        0.14 * pick,
    );
    let edge = crate::ui::blend(
        visuals.widgets.noninteractive.bg_stroke.color,
        colors.accent,
        (0.6 * hover).max(pick),
    );
    painter.rect(rect, 10.0, fill, Stroke::new(1.0 + pick, edge));
    let aspect = f32::from(w) / f32::from(h.max(1));
    let size = if aspect >= 1.0 {
        vec2(30.0, 30.0 / aspect)
    } else {
        vec2(30.0 * aspect, 30.0)
    } * (1.0 + 0.1 * hover);
    let glyph = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), size);
    painter.rect(
        glyph,
        2.5,
        colors.accent.gamma_multiply(0.15 + 0.25 * pick),
        Stroke::new(1.4_f32, colors.accent),
    );
    painter.text(
        pos2(rect.left() + 50.0, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.5),
        visuals.strong_text_color(),
    );
    painter.text(
        pos2(rect.left() + 50.0, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        format!("{w} × {h}"),
        FontId::proportional(10.5),
        colors.dim,
    );
    response
}

/// Two arrows passing each other: swaps width and height.
fn swap_button(ui: &mut Ui, colors: &Colors) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id, response.hovered(), 0.12);
    let ink = crate::ui::blend(colors.dim, colors.accent, hover);
    let painter = ui.painter();
    let c = rect.center();
    let stroke = Stroke::new(1.5_f32, ink);
    let shift = 2.0 * hover;
    // Right along the top, left along the bottom.
    let top = c.y - 3.5;
    painter.line_segment(
        [pos2(c.x - 7.0 + shift, top), pos2(c.x + 7.0 + shift, top)],
        stroke,
    );
    painter.line_segment(
        [
            pos2(c.x + 7.0 + shift, top),
            pos2(c.x + 4.0 + shift, top - 3.0),
        ],
        stroke,
    );
    let bottom = c.y + 3.5;
    painter.line_segment(
        [
            pos2(c.x - 7.0 - shift, bottom),
            pos2(c.x + 7.0 - shift, bottom),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(c.x - 7.0 - shift, bottom),
            pos2(c.x - 4.0 - shift, bottom + 3.0),
        ],
        stroke,
    );
    response
}

/// A pill with a big value and a small word under it.
fn chip(ui: &mut Ui, value: &str, word: &str, chosen: bool, colors: &Colors) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(62.0, 40.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.12);
    let pick = ctx.animate_bool_with_time(response.id.with("pick"), chosen, 0.18);
    let visuals = ui.visuals();
    let painter = ui.painter();
    let fill = crate::ui::blend(
        crate::ui::blend(
            visuals.widgets.inactive.weak_bg_fill,
            colors.accent,
            0.06 * hover,
        ),
        colors.accent,
        0.85 * pick,
    );
    painter.rect(
        rect,
        10.0,
        fill,
        Stroke::new(
            1.0_f32,
            crate::ui::blend(
                visuals.widgets.noninteractive.bg_stroke.color,
                colors.accent,
                hover,
            ),
        ),
    );
    let ink = crate::ui::blend(
        visuals.strong_text_color(),
        crate::theme::on_accent(colors.accent),
        pick,
    );
    painter.text(
        rect.center() - vec2(0.0, 6.0),
        Align2::CENTER_CENTER,
        value,
        FontId::proportional(14.0),
        ink,
    );
    painter.text(
        rect.center() + vec2(0.0, 10.0),
        Align2::CENTER_CENTER,
        word,
        FontId::proportional(9.5),
        ink.gamma_multiply(0.7),
    );
    response
}

/// A background colour to pick: a round swatch that rings when chosen.
fn swatch(ui: &mut Ui, name: &str, rgb: [u8; 3], chosen: bool, colors: &Colors) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let response = response
        .on_hover_text(name)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.12);
    let pick = ctx.animate_bool_with_time(response.id.with("pick"), chosen, 0.18);
    let painter = ui.painter();
    let center = rect.center();
    painter.circle_filled(
        center,
        11.0 + 1.5 * hover,
        Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
    );
    painter.circle_stroke(
        center,
        11.0 + 1.5 * hover,
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    if pick > 0.01 {
        painter.circle_stroke(
            center,
            14.5,
            Stroke::new(2.0_f32, colors.accent.gamma_multiply(pick)),
        );
    }
    response
}

/// The stage as it will be: it grows into each new size, a ball bounces
/// across it at the frame rate chosen, and under it a timeline as long as
/// the project plays its frames.
fn preview(ui: &mut Ui, draft: &NewProjectSpec, colors: &Colors) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 300.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 12.0, visuals.extreme_bg_color);

    // The stage, fitted into the box and gliding between sizes.
    let room = Rect::from_min_max(
        rect.min + vec2(16.0, 16.0),
        pos2(rect.right() - 16.0, rect.bottom() - 84.0),
    );
    let aspect = f32::from(draft.width.max(1)) / f32::from(draft.height.max(1));
    let target = if aspect >= room.width() / room.height() {
        vec2(room.width(), room.width() / aspect)
    } else {
        vec2(room.height() * aspect, room.height())
    };
    let id = ui.id().with("new_project_preview");
    let size = vec2(
        ctx.animate_value_with_time(id.with("w"), target.x, 0.22),
        ctx.animate_value_with_time(id.with("h"), target.y, 0.22),
    );
    let stage = Rect::from_center_size(room.center(), size);
    let background = Color32::from_rgb(draft.background.r, draft.background.g, draft.background.b);
    painter.rect_filled(
        stage.translate(vec2(0.0, 3.0)),
        4.0,
        Color32::from_black_alpha(70),
    );
    painter.rect_filled(stage, 4.0, background);
    // Lines on the stage in whatever reads against its colour.
    let light =
        u32::from(background.r()) + u32::from(background.g()) + u32::from(background.b()) > 384;
    let ink = if light {
        Color32::from_black_alpha(60)
    } else {
        Color32::from_white_alpha(60)
    };
    // Where titles stay safe: nine tenths of the stage.
    let safe = stage.shrink2(stage.size() * 0.05);
    dashed_rect(&painter, safe, Stroke::new(1.0_f32, ink));
    painter.line_segment(
        [
            stage.center() - vec2(6.0, 0.0),
            stage.center() + vec2(6.0, 0.0),
        ],
        Stroke::new(1.0_f32, ink),
    );
    painter.line_segment(
        [
            stage.center() - vec2(0.0, 6.0),
            stage.center() + vec2(0.0, 6.0),
        ],
        Stroke::new(1.0_f32, ink),
    );

    // The ball moves on whole frames only, so a low frame rate looks low.
    let fps = f32::from(draft.fps.max(1));
    let frames = f32::from(draft.frame_count.max(1));
    let time = ui.input(|input| input.time) as f32;
    let frame = (time * fps).floor() % frames;
    let t = frame / frames;
    let floor = stage.bottom() - stage.height() * 0.12;
    let radius = (stage.height() * 0.07).clamp(3.0, 12.0);
    let bounces = (frames / fps).clamp(1.0, 6.0).round();
    let height = (std::f32::consts::PI * (t * bounces).fract()).sin();
    let x = stage.left()
        + radius * 2.0
        + (stage.width() - radius * 4.0) * (0.5 - 0.5 * (std::f32::consts::TAU * t).cos());
    let at = pos2(x, floor - radius - height * (stage.height() * 0.55));
    painter.line_segment(
        [
            pos2(stage.left() + 8.0, floor),
            pos2(stage.right() - 8.0, floor),
        ],
        Stroke::new(1.0_f32, ink),
    );
    painter.circle_filled(at, radius, colors.accent);

    // The size, its shape and its length, under the stage.
    painter.text(
        pos2(rect.center().x, rect.bottom() - 66.0),
        Align2::CENTER_CENTER,
        format!(
            "{} × {}   ·   {}",
            draft.width,
            draft.height,
            ratio(draft.width, draft.height)
        ),
        FontId::proportional(13.0),
        visuals.strong_text_color(),
    );

    // A timeline as long as the project, a tick every second.
    let strip = Rect::from_min_max(
        pos2(rect.left() + 16.0, rect.bottom() - 46.0),
        pos2(rect.right() - 16.0, rect.bottom() - 30.0),
    );
    painter.rect_filled(strip, 5.0, visuals.widgets.inactive.weak_bg_fill);
    let seconds = (frames / fps).floor() as usize;
    if seconds <= 120 {
        for second in 1..=seconds {
            let x = strip.left() + strip.width() * (second as f32 * fps / frames).min(1.0);
            painter.line_segment(
                [pos2(x, strip.top() + 3.0), pos2(x, strip.bottom() - 3.0)],
                Stroke::new(1.0_f32, colors.dim.gamma_multiply(0.6)),
            );
        }
    }
    let head = strip.left() + strip.width() * t;
    painter.rect_filled(
        Rect::from_min_max(strip.min, pos2(head, strip.bottom())),
        5.0,
        colors.accent.gamma_multiply(0.25),
    );
    painter.line_segment(
        [
            pos2(head, strip.top() - 3.0),
            pos2(head, strip.bottom() + 3.0),
        ],
        Stroke::new(2.0_f32, colors.accent),
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - 14.0),
        Align2::CENTER_CENTER,
        format!(
            "{} frames at {} fps   ·   {:.1} s",
            draft.frame_count,
            draft.fps,
            frames / fps
        ),
        FontId::proportional(11.5),
        colors.dim,
    );
    // Drawn with every frame the screen shows, so the ball steps exactly on
    // its own frames; a timer of its own came round unevenly.
    ctx.request_repaint();
}

fn dashed_rect(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    for index in 0..4 {
        let (a, b) = (corners[index], corners[(index + 1) % 4]);
        painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 5.0, 4.0));
    }
}

/// "16:9", "4:3", "1:1", or a plain number when the sides share no small
/// ratio.
fn ratio(width: u16, height: u16) -> String {
    fn gcd(a: u16, b: u16) -> u16 {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    let divisor = gcd(width, height).max(1);
    let (w, h) = (width / divisor, height / divisor);
    if w <= 32 && h <= 32 {
        format!("{w}:{h}")
    } else {
        format!("{:.2}:1", f32::from(width) / f32::from(height.max(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_shapes_read_as_their_ratios() {
        assert_eq!(ratio(1920, 1080), "16:9");
        assert_eq!(ratio(640, 480), "4:3");
        assert_eq!(ratio(1080, 1920), "9:16");
        assert_eq!(ratio(1080, 1080), "1:1");
        assert_eq!(ratio(1000, 517), "1.93:1");
    }

    #[test]
    fn the_dialog_draws_and_waits() {
        let ctx = Context::default();
        let mut draft = NewProjectSpec::default();
        let colors = Colors {
            accent: Color32::from_rgb(90, 140, 255),
            dim: Color32::GRAY,
        };
        let mut outcome = None;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            outcome = Some(show(ctx, &mut draft, &colors));
        });
        assert!(!output.shapes.is_empty());
        assert!(matches!(outcome, Some(Outcome::Keep)));
    }
}
