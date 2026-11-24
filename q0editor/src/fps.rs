//! The frame rate counter: how many frames the editor really drew in the
//! last second, and how long it worked on each, measured as they happen.
//!
//! The editor draws only when something changes, so an idle editor draws
//! nothing and says so; the counter never asks for frames itself, which
//! would change what it measures.

use std::collections::VecDeque;
use std::time::Duration;

use egui::{pos2, vec2, Color32, Context, FontId, Id, LayerId, Order, Rect, Stroke};

/// How far back the counter looks.
const WINDOW: f64 = 1.0;
/// A gap this long since the frame before means the editor was idle.
const IDLE_GAP: f64 = 0.5;

#[derive(Clone, Default)]
struct Meter {
    /// When each frame of the last second began, and how long it took to
    /// work out, in milliseconds.
    frames: VecDeque<(f64, f32)>,
}

fn meter_id() -> Id {
    Id::new("q0editor.fps_meter")
}

/// Counts a frame that just took `work` to work out.
pub(crate) fn record(ctx: &Context, work: Duration) {
    let now = ctx.input(|input| input.time);
    let mut meter: Meter = ctx
        .data(|data| data.get_temp(meter_id()))
        .unwrap_or_default();
    meter.frames.push_back((now, work.as_secs_f32() * 1000.0));
    while meter
        .frames
        .front()
        .is_some_and(|(time, _)| now - time > WINDOW)
    {
        meter.frames.pop_front();
    }
    ctx.data_mut(|data| data.insert_temp(meter_id(), meter));
}

/// What the counter reads: frames a second (None while idle) and the
/// average work per frame in milliseconds.
pub(crate) fn reading(ctx: &Context) -> (Option<f32>, f32) {
    let meter: Meter = ctx
        .data(|data| data.get_temp(meter_id()))
        .unwrap_or_default();
    let work = if meter.frames.is_empty() {
        0.0
    } else {
        meter.frames.iter().map(|(_, ms)| ms).sum::<f32>() / meter.frames.len() as f32
    };
    let mut times = meter.frames.iter().map(|(time, _)| *time).rev();
    let (Some(last), Some(before)) = (times.next(), times.next()) else {
        return (None, work);
    };
    if last - before > IDLE_GAP {
        return (None, work);
    }
    let first = meter.frames.front().map_or(last, |(time, _)| *time);
    let span = (last - first) as f32;
    let fps = (span > 0.0).then(|| (meter.frames.len() - 1) as f32 / span);
    (fps, work)
}

/// The counter in the top right corner of `area`, with the last second's
/// frames as a strip of bars under it.
pub(crate) fn paint(ctx: &Context, area: Rect) {
    let (fps, work) = reading(ctx);
    let meter: Meter = ctx
        .data(|data| data.get_temp(meter_id()))
        .unwrap_or_default();
    let painter = ctx
        .layer_painter(LayerId::new(Order::Foreground, Id::new("q0editor.fps")))
        .with_clip_rect(area);
    let color = match fps {
        Some(fps) if fps >= 55.0 => Color32::from_rgb(110, 220, 120),
        Some(fps) if fps >= 30.0 => Color32::from_rgb(235, 200, 90),
        Some(_) => Color32::from_rgb(240, 100, 90),
        None => Color32::from_gray(170),
    };
    let text = match fps {
        Some(fps) => format!("{fps:.0} fps  {work:.1} ms"),
        None => format!("idle  {work:.1} ms"),
    };
    let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), color);
    let size = vec2(galley.size().x.max(96.0) + 16.0, galley.size().y + 22.0);
    let card = Rect::from_min_size(pos2(area.right() - size.x - 10.0, area.top() + 10.0), size);
    painter.rect_filled(card, 6.0, Color32::from_black_alpha(170));
    painter.rect_stroke(
        card,
        6.0,
        Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
    );
    painter.galley(card.min + vec2(8.0, 5.0), galley, color);
    // Each frame's time to the next, as a bar: taller is slower; a sixtieth
    // of a second reaches the middle line.
    let strip = Rect::from_min_max(
        pos2(card.left() + 8.0, card.bottom() - 13.0),
        pos2(card.right() - 8.0, card.bottom() - 5.0),
    );
    painter.hline(
        strip.x_range(),
        strip.center().y,
        Stroke::new(1.0_f32, Color32::from_white_alpha(30)),
    );
    let gaps: Vec<f64> = meter
        .frames
        .iter()
        .zip(meter.frames.iter().skip(1))
        .map(|((a, _), (b, _))| b - a)
        .collect();
    let width = strip.width() / 60.0;
    for (index, gap) in gaps.iter().rev().take(60).enumerate() {
        let x = strip.right() - (index as f32 + 1.0) * width;
        let height = ((*gap as f32) * 60.0 * 0.5 * strip.height()).clamp(1.0, strip.height());
        let slow = *gap > 1.0 / 30.0;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, strip.bottom() - height),
                pos2(x + width * 0.7, strip.bottom()),
            ),
            0.0,
            if slow {
                Color32::from_rgb(240, 100, 90)
            } else {
                color.gamma_multiply(0.8)
            },
        );
    }
}

const PLAYBACK_ID: &str = "q0editor.playback_rate";

/// How many timeline frames a second playback really shows, measured over
/// the last moment of playing; None while stopped or just started.
pub(crate) fn playback_rate(ctx: &Context, playing: bool, frame: u16) -> Option<f32> {
    let id = Id::new(PLAYBACK_ID);
    let now = ctx.input(|input| input.time);
    let mut seen: VecDeque<(f64, u16)> = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
    if !playing {
        if !seen.is_empty() {
            ctx.data_mut(|data| data.remove::<VecDeque<(f64, u16)>>(id));
        }
        return None;
    }
    if seen.back().is_none_or(|(_, last)| *last != frame) {
        seen.push_back((now, frame));
    }
    while seen.front().is_some_and(|(time, _)| now - time > 1.5) {
        seen.pop_front();
    }
    let rate = match (seen.front(), seen.back()) {
        (Some((first, _)), Some((last, _))) if seen.len() >= 3 && last - first > 0.3 => {
            Some((seen.len() - 1) as f32 / (last - first) as f32)
        }
        _ => None,
    };
    ctx.data_mut(|data| data.insert_temp(id, seen));
    rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counter_counts_frames_as_they_come() {
        let ctx = Context::default();
        for frame in 0..31 {
            ctx.begin_frame(egui::RawInput {
                time: Some(frame as f64 / 30.0),
                ..Default::default()
            });
            record(&ctx, Duration::from_millis(4));
            let _ = ctx.end_frame();
        }
        let (fps, work) = reading(&ctx);
        let fps = fps.expect("drawing, not idle");
        assert!((fps - 30.0).abs() < 1.0, "read {fps}");
        assert!((work - 4.0).abs() < 0.5);
        // A long pause, then one frame: idle, not a slow rate.
        ctx.begin_frame(egui::RawInput {
            time: Some(5.0),
            ..Default::default()
        });
        record(&ctx, Duration::from_millis(4));
        let _ = ctx.end_frame();
        assert_eq!(reading(&ctx).0, None);
    }
}
