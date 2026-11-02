//! q0bo's speech bubbles, the chunky old buttons in them, and filling his
//! lines in with the user's own shortcut keys.

use egui::{
    pos2, text::LayoutJob, vec2, Align2, Color32, Context, CursorIcon, FontId, Id, Order, Pos2,
    Rect, Response, Sense, Shape, Stroke, TextFormat, Ui, Vec2,
};

use super::face::{INK, RED, RED_DARK};
use crate::hotkeys::{HotkeySettings, ShortcutCommand};

pub(crate) const PAPER: Color32 = Color32::from_rgb(0xFF, 0xFC, 0xF4);
const GREY: Color32 = Color32::from_rgb(0x6E, 0x66, 0x5E);
const BUTTON: Color32 = Color32::from_rgb(0xE6, 0xE1, 0xD6);

/// Letters he types out per second.
pub(crate) const TYPING: f32 = 70.0;
const TEXT_SIZE: f32 = 14.0;

/// What happened in a bubble this frame.
#[derive(Debug, Default)]
pub(crate) struct Said {
    /// A button, by its index, once the line is fully typed out.
    pub clicked: Option<usize>,
    /// The small link in its corner.
    pub corner: bool,
    /// Clicked while he was still typing: show the whole line at once.
    pub hurry: bool,
}

/// How many letters of `text` show after `typed` worth of typing.
pub(crate) fn shown(text: &str, typed: f32) -> usize {
    (typed.max(0.0) as usize).min(text.chars().count())
}

pub(crate) fn finished(text: &str, typed: f32) -> bool {
    shown(text, typed) >= text.chars().count()
}

/// The line laid out at its full size, with the part not yet typed invisible,
/// so the bubble does not grow while he talks.
fn layout(text: &str, typed: f32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let split = text
        .char_indices()
        .nth(shown(text, typed))
        .map_or(text.len(), |(index, _)| index);
    let format = TextFormat {
        font_id: FontId::monospace(TEXT_SIZE),
        color: INK,
        ..Default::default()
    };
    job.append(&text[..split], 0.0, format.clone());
    job.append(
        &text[split..],
        0.0,
        TextFormat {
            color: Color32::TRANSPARENT,
            ..format
        },
    );
    job
}

/// A bubble floating over the editor, next to q0bo standing in `speaker`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn speech(
    ctx: &Context,
    id: Id,
    speaker: Rect,
    text: &str,
    typed: f32,
    buttons: &[&str],
    primary: bool,
    corner: Option<&str>,
) -> Said {
    let screen = ctx.screen_rect();
    // The frame's margins come on top of the text's width.
    let width = (screen.width() - 60.0).clamp(160.0, 320.0);
    let room = (speaker.left() - screen.left()).max(screen.right() - speaker.right());
    let y = speaker.top() + speaker.height() * 0.42;
    let (pivot, anchor) = if room < width + 40.0 {
        // A phone has no room beside him: the bubble goes over his head.
        (Align2::CENTER_BOTTOM, pos2(screen.center().x, speaker.top() + 4.0))
    } else if speaker.center().x > screen.center().x {
        // He usually stands on the right, so the bubble goes to his left.
        (Align2::RIGHT_BOTTOM, pos2(speaker.left() + 6.0, y))
    } else {
        (Align2::LEFT_BOTTOM, pos2(speaker.right() - 6.0, y))
    };
    let mut said = Said::default();
    let done = finished(text, typed);
    let area = egui::Area::new(id)
        .order(Order::Foreground)
        .pivot(pivot)
        .fixed_pos(anchor)
        .constrain(true)
        .show(ctx, |ui| {
            frame().show(ui, |ui| {
                ui.set_width(width);
                contents(ui, text, typed, width, buttons, primary, corner, done, &mut said);
            });
        });
    let rect = area.response.rect;
    let head = pos2(speaker.center().x, speaker.top() + speaker.height() * 0.35);
    tail(&ctx.layer_painter(area.response.layer_id), rect, head);
    said
}

/// A bubble inside a panel, with its tail pointing left at q0bo.
pub(crate) fn inline(
    ui: &mut Ui,
    text: &str,
    typed: f32,
    buttons: &[&str],
    speaker: Pos2,
) -> Said {
    let mut said = Said::default();
    let done = finished(text, typed);
    let width = (ui.available_width() - 30.0).max(120.0);
    let response = frame().show(ui, |ui| {
        ui.set_width(width);
        contents(ui, text, typed, width, buttons, true, None, done, &mut said);
    });
    tail(ui.painter(), response.response.rect, speaker);
    said
}

fn frame() -> egui::Frame {
    egui::Frame::none()
        .fill(PAPER)
        .stroke(Stroke::new(2.0_f32, INK))
        .rounding(12.0)
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .shadow(egui::epaint::Shadow {
            offset: vec2(4.0, 5.0),
            blur: 0.0,
            spread: 0.0,
            color: Color32::from_black_alpha(70),
        })
}

#[allow(clippy::too_many_arguments)]
fn contents(
    ui: &mut Ui,
    text: &str,
    typed: f32,
    width: f32,
    buttons: &[&str],
    primary: bool,
    corner: Option<&str>,
    done: bool,
    said: &mut Said,
) {
    ui.visuals_mut().override_text_color = Some(INK);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("<q0bo>")
                .font(FontId::monospace(12.0))
                .color(RED)
                .strong(),
        );
        if let Some(corner) = corner {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let link = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(corner)
                                .font(FontId::monospace(11.0))
                                .color(GREY),
                        )
                        .sense(Sense::click()),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                if link.clicked() {
                    said.corner = true;
                }
            });
        }
    });
    ui.add_space(2.0);
    let body = ui
        .add(egui::Label::new(layout(text, typed, width)).sense(Sense::click()))
        .on_hover_cursor(if done {
            CursorIcon::Default
        } else {
            CursorIcon::PointingHand
        });
    if body.clicked() && !done {
        said.hurry = true;
    }
    if !buttons.is_empty() {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            for (index, label) in buttons.iter().enumerate() {
                if retro_button(ui, label, primary && index == 0).clicked() {
                    if done {
                        said.clicked = Some(index);
                    } else {
                        said.hurry = true;
                    }
                }
            }
        });
    }
}

/// The little triangle from a bubble's edge toward the speaker's head.
fn tail(painter: &egui::Painter, rect: Rect, head: Pos2) {
    // A bubble has no size until it has been laid out once.
    if rect.contains(head) || rect.width() < 60.0 || rect.height() < 40.0 {
        return;
    }
    let toward = head - rect.center();
    // Leave from the side facing the speaker, near the bottom corner.
    let (base_a, base_b, tip) = if toward.x.abs() * rect.height() > toward.y.abs() * rect.width()
    {
        let x = if toward.x > 0.0 { rect.right() } else { rect.left() };
        let y = (rect.bottom() - 26.0).max(rect.top() + 14.0);
        let tip = pos2(x + toward.x.signum() * 16.0, (y + 10.0).min(head.y.max(y - 10.0)));
        (pos2(x, y - 9.0), pos2(x, y + 9.0), tip)
    } else {
        let y = if toward.y > 0.0 { rect.bottom() } else { rect.top() };
        let x = super::within(head.x, rect.left() + 24.0, rect.right() - 24.0);
        (
            pos2(x - 9.0, y),
            pos2(x + 9.0, y),
            pos2(x + (head.x - x).clamp(-10.0, 10.0), y + toward.y.signum() * 16.0),
        )
    };
    painter.add(Shape::convex_polygon(
        vec![base_a, tip, base_b],
        PAPER,
        Stroke::NONE,
    ));
    painter.line_segment([base_a, tip], Stroke::new(2.0_f32, INK));
    painter.line_segment([tip, base_b], Stroke::new(2.0_f32, INK));
    // Open up the bubble's border where the tail joins it.
    let inward = (rect.center() - (base_a + (base_b - base_a) * 0.5)).normalized() * 1.2;
    painter.line_segment(
        [
            base_a + (base_b - base_a) * 0.1 + inward,
            base_b + (base_a - base_b) * 0.1 + inward,
        ],
        Stroke::new(3.0_f32, PAPER),
    );
}

/// A bevelled button like the ones on a 1998 homepage. The first one in a
/// bubble is red.
pub(crate) fn retro_button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let font = FontId::monospace(13.0);
    let text_colour = if primary { Color32::WHITE } else { INK };
    let galley = ui
        .painter()
        .layout_no_wrap(format!("[ {label} ]"), font, text_colour);
    let size = galley.size() + vec2(14.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let pressed = response.is_pointer_button_down_on();
        let fill = match (primary, response.hovered()) {
            (true, true) => Color32::from_rgb(0xF0, 0x3A, 0x46),
            (true, false) => RED,
            (false, true) => Color32::from_rgb(0xF4, 0xF0, 0xE8),
            (false, false) => BUTTON,
        };
        painter.rect_filled(rect, 0.0, fill);
        let (light, dark) = if primary {
            (Color32::from_rgb(0xFF, 0x8A, 0x8F), RED_DARK)
        } else {
            (Color32::WHITE, Color32::from_rgb(0x8C, 0x86, 0x7C))
        };
        let (top_left, bottom_right) = if pressed { (dark, light) } else { (light, dark) };
        let inner = rect.shrink(1.5);
        painter.line_segment(
            [inner.left_bottom(), inner.left_top()],
            Stroke::new(2.0_f32, top_left),
        );
        painter.line_segment(
            [inner.left_top(), inner.right_top()],
            Stroke::new(2.0_f32, top_left),
        );
        painter.line_segment(
            [inner.right_top(), inner.right_bottom()],
            Stroke::new(2.0_f32, bottom_right),
        );
        painter.line_segment(
            [inner.right_bottom(), inner.left_bottom()],
            Stroke::new(2.0_f32, bottom_right),
        );
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, INK));
        let nudge = if pressed { vec2(1.0, 1.0) } else { Vec2::ZERO };
        painter.galley(
            rect.center() - galley.size() * 0.5 + nudge,
            galley,
            text_colour,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Fills `{key}` names in with the user's own shortcuts, lowercased to suit
/// him, and keeps or drops `«...»` asides about keys: phones have none.
pub(crate) fn fill(text: &str, hotkeys: &HotkeySettings, touch: bool) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find(&['{', '«'][..]) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(inner) = tail.strip_prefix('«') {
            let end = inner.find('»').unwrap_or(inner.len());
            if !touch {
                out.push_str(&fill(&inner[..end], hotkeys, touch));
            } else if out.ends_with(' ') {
                out.pop();
            }
            rest = inner.get(end + '»'.len_utf8()..).unwrap_or("");
        } else if let Some(end) = tail.find('}') {
            match command(&tail[1..end]) {
                Some(command) => out.push_str(&key(hotkeys, command)),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(tail);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// The first key bound to `command`, the way he would type it.
pub(crate) fn key(hotkeys: &HotkeySettings, command: ShortcutCommand) -> String {
    hotkeys
        .shortcuts(command)
        .first()
        .map_or_else(|| "(no key)".to_string(), |chord| chord.display().to_lowercase())
}

fn command(name: &str) -> Option<ShortcutCommand> {
    use ShortcutCommand as C;
    Some(match name {
        "select" => C::ToolSelect,
        "subselect" => C::ToolSubselect,
        "lasso" => C::ToolLasso,
        "hand" => C::ToolHand,
        "pen" => C::ToolPen,
        "pencil" => C::ToolPencil,
        "brush" => C::ToolBrush,
        "eraser" => C::ToolEraser,
        "knife" => C::ToolKnife,
        "line" => C::ToolLine,
        "rect" => C::ToolRectangle,
        "oval" => C::ToolOval,
        "bucket" => C::ToolBucket,
        "eyedropper" => C::ToolEyedropper,
        "text" => C::ToolText,
        "rig" => C::ToolRig,
        "key" => C::InsertKeyframe,
        "blank" => C::InsertBlankKeyframe,
        "frame" => C::InsertFrame,
        "clearkey" => C::ClearKeyframe,
        "removeframe" => C::RemoveFrame,
        "tween" => C::MotionTween,
        "shape" => C::ShapeTween,
        "play" => C::PlaybackToggle,
        "first" => C::FirstFrame,
        "last" => C::LastFrame,
        "save" => C::SaveProject,
        "saveas" => C::SaveProjectAs,
        "undo" => C::Undo,
        "redo" => C::Redo,
        "export" => C::Export,
        "preview" => C::FastPreview,
        "previewscene" => C::FastPreviewScene,
        "script" => C::ScriptEditor,
        "symbol" => C::ConvertToSymbol,
        "zoomreset" => C::ZoomReset,
        "snap" => C::ToggleSnap,
        "new" => C::NewProject,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_filled_in_from_the_users_shortcuts() {
        let hotkeys = HotkeySettings::default();
        assert_eq!(fill("press {key} now", &hotkeys, false), "press f6 now");
        assert_eq!(fill("{save} saves", &hotkeys, false), "ctrl+s saves");
        assert_eq!(fill("a {mystery} stays", &hotkeys, false), "a {mystery} stays");
    }

    #[test]
    fn key_asides_vanish_on_phones() {
        let hotkeys = HotkeySettings::default();
        let line = "grab the rectangle «(or press {rect})» and drag";
        assert_eq!(
            fill(line, &hotkeys, false),
            "grab the rectangle (or press r) and drag"
        );
        assert_eq!(fill(line, &hotkeys, true), "grab the rectangle and drag");
    }

    #[test]
    fn typing_never_runs_past_the_line() {
        assert_eq!(shown("héllo", 3.7), 3);
        assert_eq!(shown("héllo", 99.0), 5);
        assert!(finished("héllo", 5.0));
        let job = layout("héllo", 2.0, 100.0);
        assert_eq!(job.text, "héllo");
    }
}
