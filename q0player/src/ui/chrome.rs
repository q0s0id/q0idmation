//! Painted parts of the player's chrome: the seek bar and icon buttons of
//! the transport, recent-file cards, panel headers and the drop zone shown
//! before a movie is open. They are drawn with the painter, so they look the
//! same whatever font a platform has.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    pos2, vec2, Color32, FontId, Galley, Pos2, Rect, Response, RichText, Rounding, Sense, Shape,
    Stroke, Ui, Vec2,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    StepBack,
    StepForward,
    Restart,
    Loop,
    Fullscreen,
    Drop,
}

/// A frame label shown on the seek bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub frame: usize,
    pub name: String,
}

fn accent(ui: &Ui) -> Color32 {
    ui.visuals().selection.bg_fill
}

/// `a` moved `t` of the way to `b`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// White or black, whichever reads better on `fill`.
fn ink_on(fill: Color32) -> Color32 {
    let luma =
        0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
    if luma > 160.0 {
        Color32::from_gray(20)
    } else {
        Color32::WHITE
    }
}

/// `m:ss.cc` for a frame, or `h:mm:ss` once a movie runs past an hour.
pub fn format_time(frame: usize, fps: u16) -> String {
    let hundredths = frame.saturating_mul(100) / usize::from(fps.max(1));
    let seconds = hundredths / 100;
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{:02}", seconds % 60)
    } else {
        format!("{minutes}:{:02}.{:02}", seconds % 60, hundredths % 100)
    }
}

/// A square button showing `icon`. `on` lights it, for a toggle that is set.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, on: bool, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    if ui.is_rect_visible(rect) {
        let hovered = enabled && response.hovered();
        let pressed = enabled && response.is_pointer_button_down_on();
        let accent = accent(ui);
        let visuals = ui.visuals();
        if on || hovered {
            let fill = if on {
                accent.gamma_multiply(if hovered { 0.35 } else { 0.22 })
            } else {
                visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.7)
            };
            ui.painter()
                .rect_filled(rect, Rounding::same(size * 0.28), fill);
        }
        let color = if !enabled {
            visuals.weak_text_color().gamma_multiply(0.5)
        } else if on {
            accent
        } else if hovered {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        let inset = size * if pressed { 0.3 } else { 0.27 };
        paint_icon(ui.painter(), rect.shrink(inset), icon, color);
    }
    response
}

/// The round accent button that plays and pauses.
pub fn play_button(ui: &mut Ui, playing: bool, size: f32, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    if ui.is_rect_visible(rect) {
        let accent = accent(ui);
        let pressed = ui
            .ctx()
            .animate_bool(response.id, enabled && response.is_pointer_button_down_on());
        let fill = if !enabled {
            ui.visuals().widgets.inactive.bg_fill
        } else if response.hovered() {
            mix(accent, Color32::WHITE, 0.12)
        } else {
            accent
        };
        let radius = size * 0.5 * (1.0 - 0.06 * pressed);
        ui.painter().circle_filled(rect.center(), radius, fill);
        let ink = if enabled {
            ink_on(fill)
        } else {
            ui.visuals().weak_text_color()
        };
        let icon = if playing { Icon::Pause } else { Icon::Play };
        let glyph = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.4));
        paint_icon(ui.painter(), glyph, icon, ink);
    }
    response
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let (w, h) = (rect.width(), rect.height());
    let at = |x: f32, y: f32| pos2(rect.left() + w * x, rect.top() + h * y);
    let fill = |points: Vec<Pos2>| Shape::convex_polygon(points, color, Stroke::NONE);
    let line = Stroke::new((w * 0.13).max(1.4), color);
    match icon {
        Icon::Play => {
            // Nudged right so the triangle looks centred, not its box.
            painter.add(fill(vec![at(0.18, 0.04), at(0.98, 0.5), at(0.18, 0.96)]));
        }
        Icon::Pause => {
            for x in [0.14, 0.6] {
                painter.rect_filled(
                    Rect::from_min_max(at(x, 0.06), at(x + 0.26, 0.94)),
                    Rounding::same(w * 0.06),
                    color,
                );
            }
        }
        Icon::StepBack => {
            painter.rect_filled(
                Rect::from_min_max(at(0.08, 0.1), at(0.24, 0.9)),
                Rounding::same(w * 0.04),
                color,
            );
            painter.add(fill(vec![at(0.92, 0.1), at(0.92, 0.9), at(0.28, 0.5)]));
        }
        Icon::StepForward => {
            painter.add(fill(vec![at(0.08, 0.1), at(0.72, 0.5), at(0.08, 0.9)]));
            painter.rect_filled(
                Rect::from_min_max(at(0.76, 0.1), at(0.92, 0.9)),
                Rounding::same(w * 0.04),
                color,
            );
        }
        Icon::Restart => {
            let centre = rect.center();
            let radius = w * 0.4;
            let arc = |from: f32, to: f32| {
                (0..=24)
                    .map(|i| {
                        let angle = (from + (to - from) * i as f32 / 24.0).to_radians();
                        centre + vec2(angle.cos(), angle.sin()) * radius
                    })
                    .collect::<Vec<_>>()
            };
            let points = arc(-60.0, 230.0);
            painter.add(Shape::line(points, line));
            arrow_head(painter, centre, radius, -60.0, -1.0, w, color);
        }
        Icon::Loop => {
            let centre = rect.center();
            let radius = w * 0.4;
            for (from, to, tip, turn) in [(200.0, 340.0, 340.0, 1.0), (20.0, 160.0, 160.0, 1.0)] {
                let points = (0..=16)
                    .map(|i| {
                        let angle = (from + (to - from) * i as f32 / 16.0_f32).to_radians();
                        centre + vec2(angle.cos(), angle.sin()) * radius
                    })
                    .collect::<Vec<_>>();
                painter.add(Shape::line(points, line));
                arrow_head(painter, centre, radius, tip, turn, w, color);
            }
        }
        Icon::Fullscreen => {
            let arm = 0.32;
            for (cx, cy, sx, sy) in [
                (0.06, 0.06, 1.0, 1.0),
                (0.94, 0.06, -1.0, 1.0),
                (0.06, 0.94, 1.0, -1.0),
                (0.94, 0.94, -1.0, -1.0),
            ] {
                painter.add(Shape::line(
                    vec![at(cx + sx * arm, cy), at(cx, cy), at(cx, cy + sy * arm)],
                    line,
                ));
            }
        }
        Icon::Drop => {
            painter.add(Shape::line(
                vec![
                    at(0.06, 0.55),
                    at(0.06, 0.94),
                    at(0.94, 0.94),
                    at(0.94, 0.55),
                ],
                line,
            ));
            painter.line_segment([at(0.5, 0.04), at(0.5, 0.62)], line);
            painter.add(fill(vec![at(0.28, 0.48), at(0.72, 0.48), at(0.5, 0.74)]));
        }
    }
}

/// An arrow head at `angle` (degrees) on a circle, pointing along it.
fn arrow_head(
    painter: &egui::Painter,
    centre: Pos2,
    radius: f32,
    angle: f32,
    turn: f32,
    size: f32,
    color: Color32,
) {
    let a = angle.to_radians();
    let tip = centre + vec2(a.cos(), a.sin()) * radius;
    let along = vec2(-a.sin(), a.cos()) * turn;
    let out = vec2(a.cos(), a.sin());
    let length = size * 0.26;
    let points = vec![
        tip + along * length * 0.6,
        tip - along * length * 0.4 + out * length * 0.55,
        tip - along * length * 0.4 - out * length * 0.55,
    ];
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// The timeline: shows where playback is, and the frame someone clicks or
/// drags to. Frame labels show as ticks. Also says whether it is held down.
pub fn seek_bar(
    ui: &mut Ui,
    current: usize,
    total: usize,
    fps: u16,
    markers: &[Marker],
    enabled: bool,
    tall: bool,
) -> (Option<usize>, bool) {
    let height = if tall { 30.0 } else { 18.0 };
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), sense);
    let last = total.saturating_sub(1);
    let span = last.max(1) as f32;
    let active = enabled && (response.hovered() || response.dragged());
    let grow = ui.ctx().animate_bool(response.id, active || tall);
    let thickness = 4.0 + 2.0 * grow;
    let knob = 7.0 * grow;
    let track = Rect::from_center_size(
        rect.center(),
        vec2(rect.width() - 2.0 * knob.max(7.0), thickness),
    );
    let x_of = |frame: usize| track.left() + track.width() * (frame.min(last) as f32 / span);
    let frame_at =
        |x: f32| (((x - track.left()) / track.width()).clamp(0.0, 1.0) * span).round() as usize;

    let accent = accent(ui);
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect.expand(2.0));
    let round = Rounding::same(thickness * 0.5);
    painter.rect_filled(
        track,
        round,
        mix(visuals.extreme_bg_color, visuals.text_color(), 0.12),
    );
    let hover = response.hover_pos().filter(|_| enabled);
    if let Some(pos) = hover {
        // Where a click would land, lighter than what has played.
        let x = x_of(frame_at(pos.x));
        let ghost = Rect::from_min_max(track.min, pos2(x, track.max.y));
        painter.rect_filled(ghost, round, visuals.text_color().gamma_multiply(0.18));
    }
    if total > 1 || current > 0 {
        let played = Rect::from_min_max(track.min, pos2(x_of(current), track.max.y));
        painter.rect_filled(
            played,
            round,
            if enabled {
                accent
            } else {
                visuals.weak_text_color()
            },
        );
    }
    for marker in markers {
        let x = x_of(marker.frame);
        painter.line_segment(
            [pos2(x, track.top() - 4.0), pos2(x, track.bottom() + 4.0)],
            Stroke::new(1.5_f32, visuals.strong_text_color().gamma_multiply(0.7)),
        );
    }
    if enabled && knob > 0.5 {
        let centre = pos2(x_of(current), track.center().y);
        painter.circle_filled(centre, knob, mix(accent, Color32::WHITE, 0.15));
        painter.circle_stroke(
            centre,
            knob,
            Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
        );
    }

    let held = enabled && response.is_pointer_button_down_on();
    let mut target = None;
    if enabled && (response.dragged() || response.clicked() || response.drag_started()) {
        if let Some(pos) = response.interact_pointer_pos() {
            let frame = frame_at(pos.x);
            if frame != current {
                target = Some(frame);
            }
        }
    }
    if let Some(pos) = hover {
        let frame = frame_at(pos.x);
        let near = markers
            .iter()
            .filter(|marker| (x_of(marker.frame) - pos.x).abs() <= 5.0)
            .min_by(|a, b| {
                (x_of(a.frame) - pos.x)
                    .abs()
                    .total_cmp(&(x_of(b.frame) - pos.x).abs())
            });
        response.on_hover_ui_at_pointer(|ui| {
            if let Some(marker) = near {
                ui.label(RichText::new(&marker.name).strong());
            }
            ui.label(format!(
                "{}  ·  frame {}",
                format_time(frame, fps),
                frame + 1
            ));
        });
    }
    (target, held)
}

/// `text` on one line, cut with an ellipsis at `width`.
fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts(|fonts| fonts.layout_job(job))
}

/// What happened to a recent-file card.
pub struct CardResponse {
    pub open: bool,
    pub forget: bool,
}

/// One recent movie: its name, the folder it is in and a badge for its kind.
/// The open movie is marked. A cross to forget it shows on hover.
pub fn recent_card(
    ui: &mut Ui,
    name: &str,
    folder: &str,
    kind: &str,
    current: bool,
    tall: bool,
) -> (Response, CardResponse) {
    let height = if tall { 50.0 } else { 40.0 };
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let forget_rect = Rect::from_center_size(
        pos2(rect.right() - 16.0, rect.center().y),
        Vec2::splat(if tall { 30.0 } else { 20.0 }),
    );
    let forget = ui.interact(forget_rect, response.id.with("forget"), Sense::click());
    let hovered = response.hovered() || forget.hovered();
    if ui.is_rect_visible(rect) {
        let accent = accent(ui);
        let visuals = ui.visuals().clone();
        let painter = ui.painter();
        let rounding = Rounding::same(5.0);
        if current {
            painter.rect_filled(rect, rounding, accent.gamma_multiply(0.16));
            painter.rect_filled(
                Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(3.0, rect.height() - 12.0)),
                Rounding::same(1.5),
                accent,
            );
        } else if hovered {
            painter.rect_filled(
                rect,
                rounding,
                visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.55),
            );
        }
        let badge = Rect::from_min_size(
            pos2(rect.left() + 10.0, rect.center().y - 9.0),
            vec2(30.0, 18.0),
        );
        painter.rect_filled(badge, Rounding::same(4.0), visuals.extreme_bg_color);
        painter.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            kind,
            FontId::monospace(9.5),
            if current {
                accent
            } else {
                visuals.weak_text_color()
            },
        );
        let left = badge.right() + 9.0;
        let width = rect.right() - left - if hovered { 30.0 } else { 8.0 };
        let name = one_line(
            ui,
            name,
            FontId::proportional(13.5),
            visuals.strong_text_color(),
            width,
        );
        let folder = one_line(
            ui,
            folder,
            FontId::proportional(11.0),
            visuals.weak_text_color(),
            width,
        );
        let gap = 2.0;
        let top = rect.center().y - (name.size().y + gap + folder.size().y) * 0.5;
        painter.galley(pos2(left, top), name.clone(), visuals.strong_text_color());
        painter.galley(
            pos2(left, top + name.size().y + gap),
            folder,
            visuals.weak_text_color(),
        );
        if hovered {
            let color = if forget.hovered() {
                visuals.strong_text_color()
            } else {
                visuals.weak_text_color()
            };
            let cross = forget_rect.shrink(forget_rect.width() * 0.32);
            let stroke = Stroke::new(1.5_f32, color);
            painter.line_segment([cross.left_top(), cross.right_bottom()], stroke);
            painter.line_segment([cross.right_top(), cross.left_bottom()], stroke);
        }
    }
    let result = CardResponse {
        open: response.clicked() && !forget.clicked(),
        forget: forget.clicked(),
    };
    (
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
        result,
    )
}

/// A panel's title row: a small capitalised title, and whatever `right`
/// adds at the far end.
pub fn panel_header(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title.to_uppercase())
                .size(11.5)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(4.0);
}

/// "q0player", with its 0 in the accent colour.
pub fn wordmark(ui: &Ui, size: f32) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    let text = ui.visuals().strong_text_color();
    for (part, color) in [("q", text), ("0", accent(ui)), ("player", text)] {
        job.append(
            part,
            0.0,
            TextFormat {
                font_id: FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
    }
    ui.fonts(|fonts| fonts.layout_job(job))
}

/// The drop zone shown before a movie is open. Lights up while files are
/// dragged over the window; returns whether its Open button was pressed.
pub fn drop_zone(ui: &mut Ui, dragging: bool, touch: bool) -> bool {
    let width = ui.available_width().min(460.0);
    let height = if touch { 150.0 } else { 170.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let accent = accent(ui);
    let visuals = ui.visuals().clone();
    let glow = ui
        .ctx()
        .animate_bool(ui.id().with("drop_zone_glow"), dragging);
    let border = mix(visuals.weak_text_color().gamma_multiply(0.6), accent, glow);
    ui.painter().rect_filled(
        rect,
        Rounding::same(10.0),
        mix(visuals.extreme_bg_color, accent, 0.12 * glow),
    );
    let r = rect.shrink(1.0);
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    ui.painter().extend(Shape::dashed_line(
        &corners,
        Stroke::new(1.5_f32, border),
        7.0,
        5.0,
    ));
    let icon = Rect::from_center_size(pos2(rect.center().x, rect.top() + 38.0), Vec2::splat(30.0));
    paint_icon(ui.painter(), icon, Icon::Drop, border);
    let words = if touch {
        "Open a .q0s movie or .q0v clip"
    } else if dragging {
        "Let go to play it"
    } else {
        "Drop a .q0s movie or .q0v clip here"
    };
    ui.painter().text(
        pos2(rect.center().x, rect.top() + 76.0),
        egui::Align2::CENTER_CENTER,
        words,
        FontId::proportional(14.0),
        visuals.text_color(),
    );
    let button = Rect::from_center_size(
        pos2(
            rect.center().x,
            rect.bottom() - if touch { 36.0 } else { 40.0 },
        ),
        vec2(
            if touch { 200.0 } else { 150.0 },
            if touch { 44.0 } else { 32.0 },
        ),
    );
    let ink = ink_on(accent);
    let open = egui::Button::new(RichText::new("Open a movie…").color(ink).strong())
        .fill(accent)
        .rounding(Rounding::same(button.height() * 0.5));
    ui.put(button, open).clicked()
}

/// A soft shadow under the stage, lifting it off the backdrop.
pub fn stage_shadow(painter: &egui::Painter, stage: Rect) {
    let shadow = egui::epaint::Shadow {
        offset: vec2(0.0, 6.0),
        blur: 28.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };
    painter.add(Shape::mesh(shadow.tessellate(stage, Rounding::same(2.0))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_minutes_seconds_and_hundredths() {
        assert_eq!(format_time(0, 30), "0:00.00");
        assert_eq!(format_time(102, 30), "0:03.40");
        assert_eq!(format_time(1273, 30), "0:42.43");
        assert_eq!(format_time(30 * 125, 30), "2:05.00");
        assert_eq!(format_time(24 * 3725, 24), "1:02:05");
        assert_eq!(format_time(10, 0), "0:10.00", "no fps counts as one");
    }

    #[test]
    fn ink_stays_readable_on_light_and_dark_accents() {
        assert_eq!(ink_on(Color32::from_rgb(200, 16, 46)), Color32::WHITE);
        assert_ne!(ink_on(Color32::from_rgb(250, 240, 120)), Color32::WHITE);
    }
}
