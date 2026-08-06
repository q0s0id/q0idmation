use egui::{pos2, vec2, Color32, Sense, Stroke, Ui};
use q0s_format::v2::{Easing, EasingFamily, EasingMode, Tween};
use serde::{Deserialize, Serialize};

use crate::app::{Action, EditorApp};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicCurve {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicCurve {
    pub const DEFAULT: Self = Self {
        x1: 0.42,
        y1: 0.0,
        x2: 0.58,
        y2: 1.0,
    };

    pub fn easing(self) -> Easing {
        Easing::CubicBezier {
            x1: self.x1.clamp(0.0, 1.0),
            y1: self.y1.clamp(-8.0, 8.0),
            x2: self.x2.clamp(0.0, 1.0),
            y2: self.y2.clamp(-8.0, 8.0),
        }
    }

    pub fn from_easing(easing: Easing) -> Self {
        match easing {
            Easing::CubicBezier { x1, y1, x2, y2 } => Self { x1, y1, x2, y2 },
            Easing::Preset { mode, .. } => match mode {
                EasingMode::In => Self {
                    x1: 0.42,
                    y1: 0.0,
                    x2: 1.0,
                    y2: 1.0,
                },
                EasingMode::Out => Self {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 0.58,
                    y2: 1.0,
                },
                EasingMode::InOut => Self::DEFAULT,
            },
            Easing::Linear => Self {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 1.0,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PersistEasingValue(pub f32);

impl PartialEq for PersistEasingValue {
    fn eq(&self, other: &Self) -> bool {
        (self.0 - other.0).abs() < 1.0e-5
    }
}
impl Eq for PersistEasingValue {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EasingPreset {
    pub name: String,
    pub x1: PersistEasingValue,
    pub y1: PersistEasingValue,
    pub x2: PersistEasingValue,
    pub y2: PersistEasingValue,
}

impl Default for EasingPreset {
    fn default() -> Self {
        Self::from_curve("Custom easing", CubicCurve::DEFAULT)
    }
}

impl EasingPreset {
    pub fn from_curve(name: impl Into<String>, curve: CubicCurve) -> Self {
        Self {
            name: name.into(),
            x1: PersistEasingValue(curve.x1),
            y1: PersistEasingValue(curve.y1),
            x2: PersistEasingValue(curve.x2),
            y2: PersistEasingValue(curve.y2),
        }
    }

    pub fn curve(&self) -> CubicCurve {
        CubicCurve {
            x1: self.x1.0,
            y1: self.y1.0,
            x2: self.x2.0,
            y2: self.y2.0,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty() && self.curve().easing().is_valid()
    }

    pub fn sanitize(&mut self) {
        self.name = self.name.trim().chars().take(80).collect();
        self.x1.0 = finite_or(self.x1.0, CubicCurve::DEFAULT.x1).clamp(0.0, 1.0);
        self.y1.0 = finite_or(self.y1.0, CubicCurve::DEFAULT.y1).clamp(-8.0, 8.0);
        self.x2.0 = finite_or(self.x2.0, CubicCurve::DEFAULT.x2).clamp(0.0, 1.0);
        self.y2.0 = finite_or(self.y2.0, CubicCurve::DEFAULT.y2).clamp(-8.0, 8.0);
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[derive(Debug, Clone)]
pub struct EasingEditorState {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub placement_idx: usize,
    pub curve: CubicCurve,
    pub preset_name: String,
    active_handle: Option<usize>,
}

impl EasingEditorState {
    pub fn new(q0rg_id: u16, layer_id: u16, placement_idx: usize, easing: Easing) -> Self {
        Self {
            q0rg_id,
            layer_id,
            placement_idx,
            curve: CubicCurve::from_easing(easing),
            preset_name: "My easing".to_string(),
            active_handle: None,
        }
    }
}

pub fn render_tween_properties(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    tween: Tween,
) {
    ui.separator();
    ui.label(egui::RichText::new("Motion tween").strong());

    let Some(to_frame) = tween.to_frame() else {
        ui.label(
            egui::RichText::new("No tween starts at this keyframe")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        if ui.button("Create motion tween").clicked() {
            app.queue(Action::ToggleMotionTween);
        }
        return;
    };

    let easing = tween.easing();
    ui.label(format!("To frame {}", to_frame + 1));
    ui.label(
        egui::RichText::new(easing_label(easing))
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
    );
    paint_easing_preview(ui, easing, vec2(ui.available_width().max(120.0), 82.0));

    let mut requested = None;
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Built-in presets", |ui| {
            if ui.button("Linear").clicked() {
                requested = Some(Easing::Linear);
                ui.close_menu();
            }
            ui.separator();
            for mode in EasingMode::ALL {
                ui.menu_button(mode.label(), |ui| {
                    for family in EasingFamily::ALL {
                        if ui.button(family.label()).clicked() {
                            requested = Some(Easing::Preset { family, mode });
                            ui.close_menu();
                        }
                    }
                });
            }
        });

        if !app.settings.easing_presets.is_empty() {
            ui.menu_button("Library", |ui| {
                for preset in &app.settings.easing_presets {
                    if ui.button(&preset.name).clicked() {
                        requested = Some(preset.curve().easing());
                        ui.close_menu();
                    }
                }
            });
        }

        if ui.button("Edit curve...").clicked() {
            app.session.easing_editor = Some(EasingEditorState::new(
                q0rg_id,
                layer_id,
                placement_idx,
                easing,
            ));
        }
    });

    if let Some(easing) = requested {
        apply_tween_easing(app, q0rg_id, layer_id, placement_idx, easing);
    }
}

pub fn render_editor(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(mut state) = app.session.easing_editor.take() else {
        return;
    };
    let mut open = true;
    let mut close = false;
    let mut apply_curve = false;
    let mut save_settings = false;

    egui::Window::new("Easing Curve Editor")
        .open(&mut open)
        .default_width(430.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("drag the two handles; x is time, y is progress")
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );
            curve_editor(ui, &mut state);

            egui::Grid::new("easing_curve_values")
                .num_columns(4)
                .show(ui, |ui| {
                    ui.label("x1");
                    ui.add(
                        egui::DragValue::new(&mut state.curve.x1)
                            .speed(0.01)
                            .clamp_range(0.0..=1.0),
                    );
                    ui.label("y1");
                    ui.add(
                        egui::DragValue::new(&mut state.curve.y1)
                            .speed(0.01)
                            .clamp_range(-8.0..=8.0),
                    );
                    ui.end_row();
                    ui.label("x2");
                    ui.add(
                        egui::DragValue::new(&mut state.curve.x2)
                            .speed(0.01)
                            .clamp_range(0.0..=1.0),
                    );
                    ui.label("y2");
                    ui.add(
                        egui::DragValue::new(&mut state.curve.y2)
                            .speed(0.01)
                            .clamp_range(-8.0..=8.0),
                    );
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                if ui.button("Apply to tween").clicked() {
                    apply_curve = true;
                }
                if ui.button("Reset").clicked() {
                    state.curve = CubicCurve::DEFAULT;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });

            ui.separator();
            ui.label(egui::RichText::new("Preset library").strong());
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut state.preset_name);
                if ui.button("Save current").clicked() {
                    let name = state.preset_name.trim();
                    if !name.is_empty() {
                        let preset = EasingPreset::from_curve(name, state.curve);
                        if let Some(existing) = app
                            .settings
                            .easing_presets
                            .iter_mut()
                            .find(|item| item.name.eq_ignore_ascii_case(name))
                        {
                            *existing = preset;
                        } else if app.settings.easing_presets.len() < 128 {
                            app.settings.easing_presets.push(preset);
                        }
                        save_settings = true;
                    }
                }
            });

            let mut delete = None;
            for (index, preset) in app.settings.easing_presets.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let rename = ui.text_edit_singleline(&mut preset.name);
                    if rename.changed() {
                        preset.name = preset.name.chars().take(80).collect();
                        save_settings = true;
                    }
                    if ui.small_button("Load").clicked() {
                        state.curve = preset.curve();
                        state.preset_name = preset.name.clone();
                    }
                    if ui.small_button("Delete").clicked() {
                        delete = Some(index);
                    }
                });
            }
            if let Some(index) = delete {
                app.settings.easing_presets.remove(index);
                save_settings = true;
            }
        });

    if apply_curve {
        apply_tween_easing(
            app,
            state.q0rg_id,
            state.layer_id,
            state.placement_idx,
            state.curve.easing(),
        );
    }
    if save_settings {
        sanitize_library(&mut app.settings.easing_presets);
        app.settings.save();
    }
    if open && !close {
        app.session.easing_editor = Some(state);
    }
}

pub fn sanitize_library(presets: &mut Vec<EasingPreset>) {
    for preset in presets.iter_mut() {
        preset.sanitize();
    }
    presets.retain(EasingPreset::is_valid);
    let mut names = std::collections::HashSet::new();
    presets.retain(|preset| names.insert(preset.name.to_lowercase()));
    presets.truncate(128);
}

fn apply_tween_easing(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    easing: Easing,
) {
    let current = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| layer.placements.get(placement_idx))
        .map(|placement| placement.tween);
    let Some(current) = current else {
        app.session.status = "tween no longer exists".to_string();
        return;
    };
    if current.to_frame().is_none() {
        app.session.status = "this keyframe has no motion tween".to_string();
        return;
    }
    let updated = current.with_easing(easing);
    if updated == current {
        return;
    }
    app.history.snapshot(&app.state.project);
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
        .and_then(|layer| layer.placements.get_mut(placement_idx))
    {
        placement.tween = updated;
        app.state.mark_dirty();
        app.session.status = format!("tween easing: {}", easing_label(easing));
    }
}

fn easing_label(easing: Easing) -> String {
    match easing {
        Easing::Linear => "Linear".to_string(),
        Easing::Preset { family, mode } => format!("{} {}", mode.label(), family.label()),
        Easing::CubicBezier { x1, y1, x2, y2 } => {
            format!("Custom cubic ({x1:.2}, {y1:.2}, {x2:.2}, {y2:.2})")
        }
    }
}

fn paint_easing_preview(ui: &mut Ui, easing: Easing, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    let mut points = Vec::with_capacity(65);
    for step in 0..=64 {
        let t = step as f32 / 64.0;
        let y = easing.sample(t);
        points.push(pos2(
            egui::lerp(rect.left()..=rect.right(), t),
            egui::lerp(rect.bottom()..=rect.top(), y.clamp(-0.25, 1.25)),
        ));
    }
    painter.add(egui::Shape::line(
        points,
        Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
    ));
}

fn curve_editor(ui: &mut Ui, state: &mut EasingEditorState) {
    let desired = vec2(ui.available_width().max(280.0), 230.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let y_min = -1.0;
    let y_max = 2.0;
    let to_screen = |x: f32, y: f32| {
        pos2(
            egui::lerp(rect.left()..=rect.right(), x),
            egui::lerp(rect.bottom()..=rect.top(), (y - y_min) / (y_max - y_min)),
        )
    };
    let from_screen = |point: egui::Pos2| {
        let x = ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let y = y_min + ((rect.bottom() - point.y) / rect.height()) * (y_max - y_min);
        (x, y.clamp(-8.0, 8.0))
    };

    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    for index in 0..=6 {
        let x = rect.left() + rect.width() * index as f32 / 6.0;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, ui.visuals().faint_bg_color),
        );
    }
    for y in [0.0, 0.5, 1.0] {
        let sy = to_screen(0.0, y).y;
        painter.line_segment(
            [pos2(rect.left(), sy), pos2(rect.right(), sy)],
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }

    let p0 = to_screen(0.0, 0.0);
    let p1 = to_screen(state.curve.x1, state.curve.y1);
    let p2 = to_screen(state.curve.x2, state.curve.y2);
    let p3 = to_screen(1.0, 1.0);
    painter.line_segment([p0, p1], Stroke::new(1.0_f32, Color32::GRAY));
    painter.line_segment([p3, p2], Stroke::new(1.0_f32, Color32::GRAY));

    if response.drag_started() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let d1 = pointer.distance(p1);
            let d2 = pointer.distance(p2);
            state.active_handle = if d1.min(d2) <= 22.0 {
                Some(if d1 <= d2 { 0 } else { 1 })
            } else {
                None
            };
        }
    }
    if response.dragged() {
        if let (Some(handle), Some(pointer)) =
            (state.active_handle, response.interact_pointer_pos())
        {
            let (x, y) = from_screen(pointer);
            if handle == 0 {
                state.curve.x1 = x;
                state.curve.y1 = y;
            } else {
                state.curve.x2 = x;
                state.curve.y2 = y;
            }
        }
    }
    if response.drag_stopped() {
        state.active_handle = None;
    }

    let easing = state.curve.easing();
    let mut points = Vec::with_capacity(97);
    for step in 0..=96 {
        let t = step as f32 / 96.0;
        points.push(to_screen(t, easing.sample(t)));
    }
    painter.add(egui::Shape::line(
        points,
        Stroke::new(2.5_f32, ui.visuals().selection.stroke.color),
    ));
    painter.circle_filled(p1, 6.0, ui.visuals().selection.bg_fill);
    painter.circle_stroke(
        p1,
        7.0,
        Stroke::new(1.5_f32, ui.visuals().selection.stroke.color),
    );
    painter.circle_filled(p2, 6.0, ui.visuals().selection.bg_fill);
    painter.circle_stroke(
        p2,
        7.0,
        Stroke::new(1.5_f32, ui.visuals().selection.stroke.color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_sanitizer_deduplicates_and_bounds_curves() {
        let mut presets = vec![
            EasingPreset::from_curve(
                " wobble ",
                CubicCurve {
                    x1: -4.0,
                    y1: f32::NAN,
                    x2: 7.0,
                    y2: 99.0,
                },
            ),
            EasingPreset::from_curve("WOBBLE", CubicCurve::DEFAULT),
        ];
        sanitize_library(&mut presets);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "wobble");
        assert!(presets[0].curve().easing().is_valid());
    }
}
