//! What a tween's settings offer beyond its curve: easing picked from a
//! grid of curves or with one Animate-style strength slider, which way and
//! how many times the object turns, and its motion path drawn on stage.

use egui::{pos2, vec2, Color32, Sense, Stroke, Ui};
use q0s_format::transform::Affine;
use q0s_format::v2::{Easing, EasingFamily, EasingMode, Layer, Tween};

use crate::app::EditorApp;
use crate::easing::TweenRef;
use crate::render::StageView;

/// The curves offered at a click, in the order shown.
const QUICK_EASES: [(&str, Easing); 12] = [
    ("Linear", Easing::Linear),
    (
        "Ease in",
        Easing::Preset {
            family: EasingFamily::Sine,
            mode: EasingMode::In,
        },
    ),
    (
        "Ease out",
        Easing::Preset {
            family: EasingFamily::Sine,
            mode: EasingMode::Out,
        },
    ),
    (
        "Ease in and out",
        Easing::Preset {
            family: EasingFamily::Sine,
            mode: EasingMode::InOut,
        },
    ),
    (
        "Strong ease in",
        Easing::Preset {
            family: EasingFamily::Cubic,
            mode: EasingMode::In,
        },
    ),
    (
        "Strong ease out",
        Easing::Preset {
            family: EasingFamily::Cubic,
            mode: EasingMode::Out,
        },
    ),
    (
        "Strong ease in and out",
        Easing::Preset {
            family: EasingFamily::Cubic,
            mode: EasingMode::InOut,
        },
    ),
    (
        "Anticipate: backs up before going",
        Easing::Preset {
            family: EasingFamily::Back,
            mode: EasingMode::In,
        },
    ),
    (
        "Overshoot: goes past and settles back",
        Easing::Preset {
            family: EasingFamily::Back,
            mode: EasingMode::Out,
        },
    ),
    (
        "Elastic: springs into place",
        Easing::Preset {
            family: EasingFamily::Elastic,
            mode: EasingMode::Out,
        },
    ),
    (
        "Bounce: lands and bounces",
        Easing::Preset {
            family: EasingFamily::Bounce,
            mode: EasingMode::Out,
        },
    ),
    ("Hold: jumps at the end", Easing::Hold),
];

/// Animate's classic ease, -100 (all ease in) to +100 (all ease out), as a
/// cubic curve: at 100 it is a quadratic ease out, at -100 an ease in.
pub(crate) fn ease_strength_curve(strength: f32) -> Easing {
    let k = (strength / 100.0).clamp(-1.0, 1.0);
    if k.abs() < 0.005 {
        return Easing::Linear;
    }
    Easing::CubicBezier {
        x1: 1.0 / 3.0,
        y1: (1.0 + k) / 3.0,
        x2: 2.0 / 3.0,
        y2: (2.0 + k) / 3.0,
    }
}

/// The strength `easing` was made with by [`ease_strength_curve`], if it was.
pub(crate) fn ease_strength_of(easing: Easing) -> Option<f32> {
    match easing {
        Easing::Linear => Some(0.0),
        Easing::CubicBezier { x1, y1, x2, y2 }
            if (x1 - 1.0 / 3.0).abs() < 1e-3
                && (x2 - 2.0 / 3.0).abs() < 1e-3
                && ((y1 - 1.0 / 3.0) - (y2 - 2.0 / 3.0)).abs() < 1e-3 =>
        {
            Some(((y1 - 1.0 / 3.0) * 300.0).round())
        }
        _ => None,
    }
}

/// Which way, and how far, a motion tween turns its object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Spin {
    /// The least turning that reaches the end angle.
    Shortest,
    /// Clockwise to the end angle, and this many whole turns more.
    Clockwise(u32),
    /// Counter-clockwise to the end angle, and this many whole turns more.
    CounterClockwise(u32),
}

/// The turn from `from` to `to` (radians) that `spin` asks for. It always
/// ends at the same angle as `to`, a whole number of turns away at most.
pub(crate) fn spun_turn(from: f32, to: f32, spin: Spin) -> f32 {
    use std::f32::consts::{PI, TAU};
    let base = (to - from).rem_euclid(TAU);
    let base = if (TAU - base).abs() < 1e-5 { 0.0 } else { base };
    match spin {
        Spin::Shortest => {
            if base > PI {
                base - TAU
            } else {
                base
            }
        }
        Spin::Clockwise(turns) => base + TAU * turns as f32,
        Spin::CounterClockwise(turns) => {
            (if base > 1e-5 { base - TAU } else { 0.0 }) - TAU * turns as f32
        }
    }
}

/// Which placement on `layer` the motion tween of `source_index` goes to:
/// the same object, matched in order, on the keyframe it tweens to.
pub(crate) fn tween_destination(layer: &Layer, source_index: usize) -> Option<usize> {
    let source = layer.placements.get(source_index)?;
    let to_frame = source.tween.to_frame()?;
    let occurrence = layer.placements[..source_index]
        .iter()
        .filter(|placement| placement.frame == source.frame && placement.target == source.target)
        .count();
    layer
        .placements
        .iter()
        .enumerate()
        .filter(|(_, placement)| placement.frame == to_frame && placement.target == source.target)
        .nth(occurrence)
        .map(|(index, _)| index)
}

fn layer_of(app: &EditorApp, target: TweenRef) -> Option<&Layer> {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == target.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == target.layer_id)
}

/// The turn (radians) each motion tween in `targets` makes now.
fn tween_turns(app: &EditorApp, targets: &[TweenRef]) -> Vec<f32> {
    targets
        .iter()
        .filter(|target| target.camera_frame.is_none())
        .filter_map(|target| {
            let layer = layer_of(app, *target)?;
            let source = layer.placements.get(target.placement_idx)?;
            if !source.tween.is_motion() {
                return None;
            }
            let destination = layer
                .placements
                .get(tween_destination(layer, target.placement_idx)?)?;
            Some(destination.transform.rotation - source.transform.rotation)
        })
        .collect()
}

/// Turn every motion tween in `targets` the way `spin` says, by moving its
/// end keyframe's angle by whole turns: the end looks just as it did.
pub(crate) fn spin_tweens(app: &mut EditorApp, targets: &[TweenRef], spin: Spin) {
    let mut changes = Vec::new();
    for target in targets
        .iter()
        .filter(|target| target.camera_frame.is_none())
    {
        let Some(layer) = layer_of(app, *target) else {
            continue;
        };
        let Some(source) = layer.placements.get(target.placement_idx) else {
            continue;
        };
        if !source.tween.is_motion() {
            continue;
        }
        let Some(destination_index) = tween_destination(layer, target.placement_idx) else {
            continue;
        };
        let from = source.transform.rotation;
        let to = layer.placements[destination_index].transform.rotation;
        let turned = from + spun_turn(from, to, spin);
        if (turned - to).abs() > 1e-5 {
            changes.push((target.q0rg_id, target.layer_id, destination_index, turned));
        }
    }
    if changes.is_empty() {
        app.session.status = "the tween already turns that way".to_string();
        return;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    for (q0rg_id, layer_id, index, rotation) in changes {
        if let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(index))
        {
            placement.transform.rotation = rotation;
        }
    }
    app.state.mark_dirty();
    app.textures.invalidate();
    app.session.status = match spin {
        Spin::Shortest => "the tween turns the shortest way".to_string(),
        Spin::Clockwise(0) => "the tween turns clockwise".to_string(),
        Spin::CounterClockwise(0) => "the tween turns counter-clockwise".to_string(),
        Spin::Clockwise(turns) => format!("the tween turns clockwise, {turns} extra turn(s)"),
        Spin::CounterClockwise(turns) => {
            format!("the tween turns counter-clockwise, {turns} extra turn(s)")
        }
    };
}

fn describe_turn(turn: f32) -> String {
    let degrees = turn.to_degrees();
    if degrees.abs() < 0.05 {
        "No turning".to_string()
    } else if degrees > 0.0 {
        format!("Turns {degrees:.0}\u{b0} clockwise")
    } else {
        format!("Turns {:.0}\u{b0} counter-clockwise", -degrees)
    }
}

fn paint_curve(painter: &egui::Painter, rect: egui::Rect, easing: Easing, color: Color32) {
    let inner = rect.shrink2(vec2(4.0, 6.0));
    let points: Vec<_> = (0..=32)
        .map(|step| {
            let t = step as f32 / 32.0;
            pos2(
                egui::lerp(inner.left()..=inner.right(), t),
                egui::lerp(
                    inner.bottom()..=inner.top(),
                    easing.sample(t).clamp(-0.3, 1.3),
                ),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.6_f32, color)));
}

/// The extra rows of a tween's settings: how long it is, a grid of curves,
/// the ease slider and, for motion tweens, which way it turns. Returns the
/// easing picked, for the caller to apply with its own undo step.
pub(crate) fn render_tween_extras(
    app: &mut EditorApp,
    ui: &mut Ui,
    records: &[(TweenRef, u16, Tween)],
    common_easing: Option<Easing>,
) -> Option<Easing> {
    let dim = app.settings.theme.text_dim.to_color32();
    let accent = app.settings.theme.accent.to_color32();
    let text = app.settings.theme.text.to_color32();
    if let [(_, start, tween)] = records {
        if let Some(end) = tween.to_frame() {
            ui.label(
                egui::RichText::new(format!(
                    "Frames {} to {} \u{b7} {} frames",
                    start + 1,
                    end + 1,
                    end.saturating_sub(*start)
                ))
                .small()
                .color(dim),
            );
        }
    }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (label, easing) in QUICK_EASES {
            let (rect, response) = ui.allocate_exact_size(vec2(46.0, 32.0), Sense::click());
            let current = common_easing == Some(easing);
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                3.0,
                if current {
                    accent.gamma_multiply(0.25)
                } else if response.hovered() {
                    accent.gamma_multiply(0.12)
                } else {
                    Color32::TRANSPARENT
                },
            );
            painter.rect_stroke(
                rect,
                3.0,
                Stroke::new(
                    1.0_f32,
                    if current {
                        accent
                    } else {
                        dim.gamma_multiply(0.6)
                    },
                ),
            );
            paint_curve(&painter, rect, easing, if current { accent } else { text });
            if response.on_hover_text(label).clicked() {
                picked = Some(easing);
            }
        }
    });
    let strength = common_easing.and_then(ease_strength_of);
    ui.horizontal(|ui| {
        ui.label("Ease");
        let mut value = strength.unwrap_or(0.0);
        let slider = ui
            .add(egui::Slider::new(&mut value, -100.0..=100.0).step_by(1.0))
            .on_hover_text("Animate's ease: below zero it starts slow, above zero it ends slow");
        if strength.is_none() {
            ui.label(egui::RichText::new("(custom curve)").small().color(dim));
        }
        if slider.changed() {
            picked = Some(ease_strength_curve(value));
        }
    });

    let targets: Vec<TweenRef> = records.iter().map(|(target, _, _)| *target).collect();
    let turns = tween_turns(app, &targets);
    if !turns.is_empty() {
        let first = turns[0];
        let same = turns.iter().all(|turn| (turn - first).abs() < 1e-4);
        ui.horizontal_wrapped(|ui| {
            ui.label(if same {
                describe_turn(first)
            } else {
                "Turning: mixed".to_string()
            });
        });
        let extra = if same {
            (first.abs() / std::f32::consts::TAU).floor() as u32
        } else {
            0
        };
        let mut spin = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Shortest")
                .on_hover_text("Turn the least way round to the end angle")
                .clicked()
            {
                spin = Some(Spin::Shortest);
            }
            if ui
                .button("\u{21bb} Clockwise")
                .on_hover_text("Turn clockwise to the end angle")
                .clicked()
            {
                spin = Some(Spin::Clockwise(extra));
            }
            if ui
                .button("\u{21ba} Counter")
                .on_hover_text("Turn counter-clockwise to the end angle")
                .clicked()
            {
                spin = Some(Spin::CounterClockwise(extra));
            }
            let mut more = extra;
            let response = ui
                .add(
                    egui::DragValue::new(&mut more)
                        .clamp_range(0..=50)
                        .prefix("+")
                        .suffix(" turns"),
                )
                .on_hover_text("Whole turns more on the way, in the same direction");
            if response.changed() {
                spin = Some(if same && first < 0.0 {
                    Spin::CounterClockwise(more)
                } else {
                    Spin::Clockwise(more)
                });
            }
        });
        if let Some(spin) = spin {
            spin_tweens(app, &targets, spin);
        }
    }
    picked
}

/// Where the object of motion tween `source_index` on `layer` is at each
/// frame of the tween: the point it turns about, in the symbol's space.
fn motion_path(
    app: &EditorApp,
    layer: &Layer,
    source_index: usize,
) -> Vec<(u16, q0s_format::v2::Vec2)> {
    let Some(source) = layer.placements.get(source_index) else {
        return Vec::new();
    };
    let Some(to_frame) = source.tween.to_frame() else {
        return Vec::new();
    };
    let anchor = source
        .pivot
        .or_else(|| q0s_format::motion_tween::default_pivot(&app.state.project, source.target))
        .unwrap_or(q0s_format::v2::Vec2::new(0.0, 0.0));
    let timeline = q0s_format::raster::LayerTimeline::new(layer);
    (source.frame..=to_frame.min(source.frame.saturating_add(600)))
        .filter_map(|frame| {
            let state = if frame == to_frame {
                tween_destination(layer, source_index)
                    .and_then(|index| layer.placements.get(index))
                    .map(|placement| placement.transform)
            } else {
                q0s_format::raster::active_placement_states_with(layer, frame, &timeline)
                    .into_iter()
                    .find(|state| state.index == source_index)
                    .map(|state| state.transform)
            }?;
            Some((frame, Affine::from_transform(state).apply(anchor)))
        })
        .collect()
}

/// The motion paths of the tweens the playhead is in on the current layer:
/// a dot for every frame, the current one ringed, so how a tween moves and
/// eases can be seen before it plays.
pub(crate) fn draw_motion_paths(app: &EditorApp, painter: &egui::Painter, view: &StageView) {
    if app.session.playing {
        return;
    }
    let frame = app.session.current_frame;
    let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == app.session.current_q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == app.session.current_layer_id)
        })
    else {
        return;
    };
    if !app
        .state
        .project
        .layer_is_visible(app.session.current_q0rg_id, layer.layer_id)
    {
        return;
    }
    let accent = app.settings.theme.accent.to_color32();
    let sources: Vec<usize> = layer
        .placements
        .iter()
        .enumerate()
        .filter(|(_, placement)| {
            placement.tween.is_motion()
                && placement.frame <= frame
                && placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| frame <= to_frame)
        })
        .map(|(index, _)| index)
        .take(16)
        .collect();
    for source in sources {
        let path = motion_path(app, layer, source);
        if path.len() < 2 {
            continue;
        }
        let screen: Vec<_> = path
            .iter()
            .map(|(_, point)| view.stage_to_screen(*point))
            .collect();
        painter.add(egui::Shape::line(
            screen.clone(),
            Stroke::new(1.0_f32, accent.gamma_multiply(0.55)),
        ));
        let last = path.len() - 1;
        for (index, ((at, _), point)) in path.iter().zip(&screen).enumerate() {
            let key = index == 0 || index == last;
            painter.circle_filled(
                *point,
                if key { 3.5 } else { 2.0 },
                if key {
                    accent
                } else {
                    accent.gamma_multiply(0.8)
                },
            );
            if *at == frame {
                painter.circle_stroke(*point, 6.0, Stroke::new(1.5_f32, accent));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{PI, TAU};

    #[test]
    fn the_ease_slider_round_trips_and_eases() {
        for strength in [-100.0, -40.0, 25.0, 100.0] {
            let curve = ease_strength_curve(strength);
            assert_eq!(ease_strength_of(curve), Some(strength));
            assert!(curve.is_valid());
        }
        assert_eq!(ease_strength_curve(0.0), Easing::Linear);
        // Ease out is ahead at the middle, ease in behind.
        assert!(ease_strength_curve(100.0).sample(0.5) > 0.6);
        assert!(ease_strength_curve(-100.0).sample(0.5) < 0.4);
    }

    #[test]
    fn spins_end_where_the_tween_ended() {
        let (from, to) = (0.2_f32, 0.2 + 1.5 * PI);
        for spin in [
            Spin::Shortest,
            Spin::Clockwise(0),
            Spin::Clockwise(2),
            Spin::CounterClockwise(0),
            Spin::CounterClockwise(1),
        ] {
            let turn = spun_turn(from, to, spin);
            let off = (from + turn - to).rem_euclid(TAU);
            assert!(off < 1e-4 || TAU - off < 1e-4, "{spin:?} ends elsewhere");
        }
        assert!((spun_turn(from, to, Spin::Shortest) + 0.5 * PI).abs() < 1e-4);
        assert!((spun_turn(from, to, Spin::Clockwise(1)) - 3.5 * PI).abs() < 1e-4);
        assert!((spun_turn(from, to, Spin::CounterClockwise(0)) + 0.5 * PI).abs() < 1e-4);
        assert!((spun_turn(0.0, 0.0, Spin::Clockwise(1)) - TAU).abs() < 1e-4);
    }
}
