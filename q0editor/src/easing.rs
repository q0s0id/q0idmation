use egui::{pos2, vec2, Color32, Sense, Stroke, Ui};
use q0s_format::v2::{Easing, EasingFamily, EasingMode, ProjectV2, Tween};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::state::TimelineSelection;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TweenRef {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub placement_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TweenCreateError {
    NoTimelineCell,
    KeyframeSelected,
    MissingSourceKeyframe,
    MissingDestinationKeyframe,
    EmptySourceKeyframe,
    EmptyDestinationKeyframe,
    AlreadyTweened,
    NoMatchingObjects,
}

impl TweenCreateError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoTimelineCell => {
                "Select a non-keyframe timeline cell between two keyframes first."
            }
            Self::KeyframeSelected => {
                "A motion tween cannot be created on a keyframe. Select a frame between two keyframes."
            }
            Self::MissingSourceKeyframe => {
                "There is no source keyframe behind this cell. Create a non-empty keyframe first."
            }
            Self::MissingDestinationKeyframe => {
                "There is no destination keyframe ahead. Create a non-empty keyframe after this cell first."
            }
            Self::EmptySourceKeyframe => {
                "The source keyframe is empty. A motion tween needs artwork on both keyframes."
            }
            Self::EmptyDestinationKeyframe => {
                "The keyframe ahead is empty. Put the destination artwork there before creating a tween."
            }
            Self::AlreadyTweened => {
                "This frame is already part of a motion tween. Use Properties to edit or remove it."
            }
            Self::NoMatchingObjects => {
                "The two keyframes do not contain matching objects that can be tweened."
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TweenCreationPlan {
    layer_id: u16,
    source_frame: u16,
    destination_frame: u16,
    placement_indices: Vec<usize>,
}

fn selected_layer_ids(selection: TimelineSelection, visible_layer_ids: &[u16]) -> Vec<u16> {
    let Some(anchor) = visible_layer_ids
        .iter()
        .position(|layer_id| *layer_id == selection.anchor_layer_id)
    else {
        return Vec::new();
    };
    let Some(focus) = visible_layer_ids
        .iter()
        .position(|layer_id| *layer_id == selection.focus_layer_id)
    else {
        return Vec::new();
    };
    visible_layer_ids[anchor.min(focus)..=anchor.max(focus)].to_vec()
}

pub fn create_tweens_for_selection(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
    visible_layer_ids: &[u16],
) -> Result<Vec<TweenRef>, TweenCreateError> {
    let layer_ids = selected_layer_ids(selection, visible_layer_ids);
    if layer_ids.is_empty() {
        return Err(TweenCreateError::NoTimelineCell);
    }
    let first_frame = selection.anchor_frame.min(selection.focus_frame);
    let last_frame = selection.anchor_frame.max(selection.focus_frame);
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Err(TweenCreateError::NoTimelineCell);
    };

    let mut plans = Vec::<TweenCreationPlan>::new();
    for layer_id in layer_ids {
        if project.layer_is_folder(q0rg_id, layer_id) {
            continue;
        }
        let Some(layer) = q0rg.layers.iter().find(|layer| layer.layer_id == layer_id) else {
            continue;
        };
        let keyframes = layer.keyframe_frames();
        for frame in first_frame..=last_frame {
            if layer.has_keyframe(frame) {
                return Err(TweenCreateError::KeyframeSelected);
            }
            if layer.placements.iter().any(|placement| {
                placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| placement.frame < frame && frame < to_frame)
            }) {
                return Err(TweenCreateError::AlreadyTweened);
            }

            let source_frame = keyframes
                .iter()
                .copied()
                .filter(|keyframe| *keyframe < frame)
                .max()
                .ok_or(TweenCreateError::MissingSourceKeyframe)?;
            let destination_frame = keyframes
                .iter()
                .copied()
                .filter(|keyframe| *keyframe > frame)
                .min()
                .ok_or(TweenCreateError::MissingDestinationKeyframe)?;

            if plans.iter().any(|plan| {
                plan.layer_id == layer_id
                    && plan.source_frame == source_frame
                    && plan.destination_frame == destination_frame
            }) {
                continue;
            }

            let source_indices = layer
                .placements
                .iter()
                .enumerate()
                .filter(|(_, placement)| placement.frame == source_frame)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if source_indices.is_empty() {
                return Err(TweenCreateError::EmptySourceKeyframe);
            }
            if !layer
                .placements
                .iter()
                .any(|placement| placement.frame == destination_frame)
            {
                return Err(TweenCreateError::EmptyDestinationKeyframe);
            }

            let mut placement_indices = Vec::new();
            for (source_order, placement_idx) in source_indices.iter().copied().enumerate() {
                let source = &layer.placements[placement_idx];
                if source.tween.to_frame().is_some() {
                    return Err(TweenCreateError::AlreadyTweened);
                }
                let occurrence = source_indices[..source_order]
                    .iter()
                    .filter(|candidate| layer.placements[**candidate].target == source.target)
                    .count();
                let destination_exists = layer
                    .placements
                    .iter()
                    .filter(|placement| {
                        placement.frame == destination_frame && placement.target == source.target
                    })
                    .nth(occurrence)
                    .is_some();
                if destination_exists {
                    placement_indices.push(placement_idx);
                }
            }
            if placement_indices.is_empty() {
                return Err(TweenCreateError::NoMatchingObjects);
            }
            plans.push(TweenCreationPlan {
                layer_id,
                source_frame,
                destination_frame,
                placement_indices,
            });
        }
    }

    if plans.is_empty() {
        return Err(TweenCreateError::NoTimelineCell);
    }

    let q0rg = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .expect("q0rg was resolved before tween mutation");
    let mut targets = Vec::new();
    for plan in plans {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == plan.layer_id)
            .expect("selected tween layer still exists");
        for placement_idx in plan.placement_indices {
            layer.placements[placement_idx].tween = Tween::Linear {
                to_frame: plan.destination_frame,
            };
            targets.push(TweenRef {
                q0rg_id,
                layer_id: plan.layer_id,
                placement_idx,
            });
        }
    }
    targets.sort_unstable();
    targets.dedup();
    Ok(targets)
}

pub fn tween_targets_for_selection(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
    visible_layer_ids: &[u16],
) -> Vec<TweenRef> {
    let selected_layers = selected_layer_ids(selection, visible_layer_ids);
    let first_frame = selection.anchor_frame.min(selection.focus_frame);
    let last_frame = selection.anchor_frame.max(selection.focus_frame);
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for layer_id in selected_layers {
        let Some(layer) = q0rg.layers.iter().find(|layer| layer.layer_id == layer_id) else {
            continue;
        };
        for (placement_idx, placement) in layer.placements.iter().enumerate() {
            let Some(to_frame) = placement.tween.to_frame() else {
                continue;
            };
            if placement.frame <= last_frame && to_frame >= first_frame {
                targets.push(TweenRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                });
            }
        }
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

pub fn selected_tween_targets(app: &EditorApp) -> Vec<TweenRef> {
    let Some(selection) = app.session.timeline_selection else {
        return Vec::new();
    };
    let q0rg_id = app.session.current_q0rg_id;
    let visible_layer_ids = crate::panels::timeline::visible_layer_ids(&app.state.project, q0rg_id);
    tween_targets_for_selection(&app.state.project, q0rg_id, selection, &visible_layer_ids)
}

#[derive(Debug, Clone)]
pub struct EasingEditorState {
    pub targets: Vec<TweenRef>,
    pub curve: CubicCurve,
    pub preset_name: String,
    active_handle: Option<usize>,
}

impl EasingEditorState {
    pub fn new(mut targets: Vec<TweenRef>, easing: Easing) -> Self {
        targets.sort_unstable();
        targets.dedup();
        Self {
            targets,
            curve: CubicCurve::from_easing(easing),
            preset_name: "My easing".to_string(),
            active_handle: None,
        }
    }
}

fn tween_for_target(app: &EditorApp, target: TweenRef) -> Option<(u16, Tween)> {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == target.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == target.layer_id)
        })
        .and_then(|layer| layer.placements.get(target.placement_idx))
        .and_then(|placement| {
            placement
                .tween
                .to_frame()
                .map(|_| (placement.frame, placement.tween))
        })
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
    if tween.to_frame().is_none() {
        ui.label(
            egui::RichText::new(
                "Select a non-keyframe timeline cell between two keyframes to create a tween.",
            )
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
        return;
    }
    render_tween_controls(
        app,
        ui,
        &[TweenRef {
            q0rg_id,
            layer_id,
            placement_idx,
        }],
        false,
    );
}

pub fn render_selected_tween_properties(app: &mut EditorApp, ui: &mut Ui, targets: &[TweenRef]) {
    render_tween_controls(app, ui, targets, true);
}

fn render_tween_controls(
    app: &mut EditorApp,
    ui: &mut Ui,
    targets: &[TweenRef],
    timeline_selection: bool,
) {
    let records = targets
        .iter()
        .copied()
        .filter_map(|target| {
            tween_for_target(app, target).map(|(start, tween)| (target, start, tween))
        })
        .collect::<Vec<_>>();
    if records.is_empty() {
        return;
    }

    let mut spans = std::collections::BTreeSet::new();
    for (target, start, tween) in &records {
        if let Some(end) = tween.to_frame() {
            spans.insert((target.layer_id, *start, end));
        }
    }
    if timeline_selection {
        ui.label(
            egui::RichText::new(if spans.len() == 1 {
                "1 motion tween".to_string()
            } else {
                format!("{} motion tweens", spans.len())
            })
            .strong(),
        );
        ui.label(
            egui::RichText::new(format!("{} animated object track(s)", records.len()))
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }

    let first_easing = records[0].2.easing();
    let common_easing = records
        .iter()
        .all(|(_, _, tween)| tween.easing() == first_easing)
        .then_some(first_easing);
    match common_easing {
        Some(easing) => {
            ui.label(
                egui::RichText::new(easing_label(easing))
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );
            paint_easing_preview(ui, easing, vec2(ui.available_width().max(120.0), 82.0));
        }
        None => {
            ui.label(
                egui::RichText::new(
                    "Mixed easing — a new preset will be applied to all selected tweens",
                )
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
            );
        }
    }

    let target_refs = records
        .iter()
        .map(|(target, _, _)| *target)
        .collect::<Vec<_>>();
    let mut requested = None;
    let mut remove = false;
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
                target_refs.clone(),
                common_easing.unwrap_or(first_easing),
            ));
        }
        if ui
            .button(if spans.len() == 1 {
                "Remove tween"
            } else {
                "Remove tweens"
            })
            .clicked()
        {
            remove = true;
        }
    });

    if let Some(easing) = requested {
        apply_tween_easing(app, &target_refs, easing);
    }
    if remove {
        remove_tweens(app, &target_refs);
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

    let title = if state.targets.len() == 1 {
        "Easing Curve Editor".to_string()
    } else {
        format!("Easing Curve Editor — {} tweens", state.targets.len())
    };
    egui::Window::new(title)
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
                let apply_label = if state.targets.len() == 1 {
                    "Apply to tween".to_string()
                } else {
                    format!("Apply to {} tweens", state.targets.len())
                };
                if ui.button(apply_label).clicked() {
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
        apply_tween_easing(app, &state.targets, state.curve.easing());
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

fn apply_tween_easing(app: &mut EditorApp, targets: &[TweenRef], easing: Easing) {
    let mut changed = targets
        .iter()
        .copied()
        .filter(|target| {
            tween_for_target(app, *target)
                .is_some_and(|(_, tween)| tween.with_easing(easing) != tween)
        })
        .collect::<Vec<_>>();
    changed.sort_unstable();
    changed.dedup();
    if changed.is_empty() {
        return;
    }

    app.history.snapshot(&app.state.project);
    let mut updated_count = 0usize;
    for target in changed {
        let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == target.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == target.layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(target.placement_idx))
        else {
            continue;
        };
        if placement.tween.to_frame().is_some() {
            placement.tween = placement.tween.with_easing(easing);
            updated_count += 1;
        }
    }
    if updated_count > 0 {
        app.state.mark_dirty();
        app.session.status = if updated_count == 1 {
            format!("tween easing: {}", easing_label(easing))
        } else {
            format!("updated easing on {updated_count} tween tracks")
        };
    }
}

fn remove_tweens(app: &mut EditorApp, targets: &[TweenRef]) {
    let mut removable = targets
        .iter()
        .copied()
        .filter(|target| tween_for_target(app, *target).is_some())
        .collect::<Vec<_>>();
    removable.sort_unstable();
    removable.dedup();
    if removable.is_empty() {
        return;
    }

    app.history.snapshot(&app.state.project);
    let mut removed = 0usize;
    for target in removable {
        let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == target.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == target.layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(target.placement_idx))
        else {
            continue;
        };
        if placement.tween.to_frame().is_some() {
            placement.tween = Tween::None;
            removed += 1;
        }
    }
    if removed > 0 {
        app.state.mark_dirty();
        app.session.status = if removed == 1 {
            "removed motion tween".to_string()
        } else {
            format!("removed {removed} motion tween tracks")
        };
    }
}

pub fn render_warning(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(message) = app.session.tween_warning.clone() else {
        return;
    };
    let mut open = true;
    let mut dismiss = false;
    egui::Window::new("Motion tween not created")
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(8.0);
            if ui.button("OK").clicked() {
                dismiss = true;
            }
        });
    if dismiss || !open {
        app.session.tween_warning = None;
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

    fn timeline_project() -> ProjectV2 {
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.q0rgs[0].layers[0].placements = vec![
            q0s_format::v2::Placement {
                frame: 0,
                target: q0s_format::v2::Target::Asset(77),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: Tween::None,
            },
            q0s_format::v2::Placement {
                frame: 6,
                target: q0s_format::v2::Target::Asset(77),
                transform: q0s_format::v2::Transform2D {
                    tx: 60.0,
                    ..q0s_format::v2::Transform2D::IDENTITY
                },
                tween: Tween::None,
            },
        ];
        project
    }

    #[test]
    fn non_keyframe_between_two_content_keys_creates_tween() {
        let mut project = timeline_project();
        let targets =
            create_tweens_for_selection(&mut project, 1, TimelineSelection::single(1, 3), &[1])
                .expect("interior frame must create tween");

        assert_eq!(targets.len(), 1);
        assert_eq!(
            project.q0rgs[0].layers[0].placements[0].tween,
            Tween::Linear { to_frame: 6 }
        );
        assert_eq!(project.q0rgs[0].layers[0].placements.len(), 2);
    }

    #[test]
    fn keyframe_selection_is_rejected_without_mutation() {
        let mut project = timeline_project();
        let before = project.clone();
        assert_eq!(
            create_tweens_for_selection(&mut project, 1, TimelineSelection::single(1, 0), &[1],),
            Err(TweenCreateError::KeyframeSelected)
        );
        assert_eq!(project, before);
    }

    #[test]
    fn empty_destination_keyframe_is_rejected_without_auto_keying() {
        let mut project = timeline_project();
        project.q0rgs[0].layers[0].placements.pop();
        project.q0rgs[0].layers[0].explicit_keyframes = vec![6];
        let before = project.clone();

        assert_eq!(
            create_tweens_for_selection(&mut project, 1, TimelineSelection::single(1, 3), &[1],),
            Err(TweenCreateError::EmptyDestinationKeyframe)
        );
        assert_eq!(project, before);
    }

    #[test]
    fn missing_destination_keyframe_is_rejected_without_extending_timeline() {
        let mut project = timeline_project();
        project.q0rgs[0].layers[0].placements.pop();
        let before = project.clone();

        assert_eq!(
            create_tweens_for_selection(&mut project, 1, TimelineSelection::single(1, 3), &[1],),
            Err(TweenCreateError::MissingDestinationKeyframe)
        );
        assert_eq!(project, before);
    }

    #[test]
    fn selecting_any_cell_of_tween_finds_its_settings_target() {
        let mut project = timeline_project();
        project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 6 };

        for frame in [0, 3, 6] {
            let targets =
                tween_targets_for_selection(&project, 1, TimelineSelection::single(1, frame), &[1]);
            assert_eq!(targets.len(), 1, "frame {frame} must resolve tween");
            assert_eq!(targets[0].placement_idx, 0);
        }
    }

    #[test]
    fn rectangular_selection_collects_multiple_tweens() {
        let mut project = timeline_project();
        project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 6 };
        project.q0rgs[0].layers.push(q0s_format::v2::Layer {
            layer_id: 2,
            name: "second".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                q0s_format::v2::Placement {
                    frame: 0,
                    target: q0s_format::v2::Target::Asset(88),
                    transform: q0s_format::v2::Transform2D::IDENTITY,
                    tween: Tween::Linear { to_frame: 6 },
                },
                q0s_format::v2::Placement {
                    frame: 6,
                    target: q0s_format::v2::Target::Asset(88),
                    transform: q0s_format::v2::Transform2D::IDENTITY,
                    tween: Tween::None,
                },
            ],
        });
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 2,
            focus_frame: 4,
        };
        let targets = tween_targets_for_selection(&project, 1, selection, &[1, 2]);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].layer_id, 1);
        assert_eq!(targets[1].layer_id, 2);
    }

    #[test]
    fn applying_easing_updates_multiple_tweens_as_one_undo_step() {
        let mut app = EditorApp::default();
        app.state.project = timeline_project();
        app.state.project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 6 };
        app.state.project.q0rgs[0]
            .layers
            .push(q0s_format::v2::Layer {
                layer_id: 2,
                name: "second".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![
                    q0s_format::v2::Placement {
                        frame: 0,
                        target: q0s_format::v2::Target::Asset(88),
                        transform: q0s_format::v2::Transform2D::IDENTITY,
                        tween: Tween::Linear { to_frame: 6 },
                    },
                    q0s_format::v2::Placement {
                        frame: 6,
                        target: q0s_format::v2::Target::Asset(88),
                        transform: q0s_format::v2::Transform2D::IDENTITY,
                        tween: Tween::None,
                    },
                ],
            });
        let targets = vec![
            TweenRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            },
            TweenRef {
                q0rg_id: 1,
                layer_id: 2,
                placement_idx: 0,
            },
        ];
        let easing = Easing::Preset {
            family: EasingFamily::Bounce,
            mode: EasingMode::InOut,
        };

        apply_tween_easing(&mut app, &targets, easing);

        assert_eq!(
            app.state.project.q0rgs[0].layers[0].placements[0]
                .tween
                .easing(),
            easing
        );
        assert_eq!(
            app.state.project.q0rgs[0].layers[1].placements[0]
                .tween
                .easing(),
            easing
        );
        assert!(app.history.can_undo());
        assert!(app.state.dirty);
    }

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
