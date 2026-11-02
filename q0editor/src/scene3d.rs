//! The 3D window and 3D placement controls: the timeline's camera and 3D
//! lights, how placed objects sit in depth and turn, which clip a model
//! plays, and bones posed on the timeline.

use std::sync::Arc;

use egui::{Context, RichText, Ui};
use q0s_format::three::model::{ModelData, Trs};
use q0s_format::three::scene::{CameraPose, Light3d, Light3dKind, Scene3dSetup, Space3d, NO_CLIP};
use q0s_format::three::{ModelAsset, Quat, V3};
use q0s_format::v2::{Asset, PlacementFx, Rgba, Target};

use crate::app::EditorApp;
use crate::physics::{apply_edit, Edit};
use crate::state::{PlacementRef, Selection};
use crate::ui::FitToScreen;

#[derive(Default)]
pub struct Scene3dSession {
    pub open: bool,
    pub selected_light: Option<u16>,
    /// The bone being posed in the properties panel.
    pub pose_bone: Option<u16>,
    /// A light (and whether its aim) being dragged on the stage.
    pub drag: Option<(u16, bool)>,
    /// How many frames a physics bake runs for.
    pub bake_frames: u16,
    /// The liquid shown in the 3D window.
    pub selected_liquid: Option<u16>,
}

fn setup_of(app: &EditorApp, q0rg_id: u16) -> Scene3dSetup {
    let project = &app.state.project;
    project
        .runtime
        .scene3d
        .setup(q0rg_id)
        .cloned()
        .unwrap_or_else(|| {
            Scene3dSetup::new(q0rg_id, project.meta.stage_width, project.meta.stage_height)
        })
}

fn store_setup(app: &mut EditorApp, edit: &Edit, setup: Scene3dSetup) {
    apply_edit(app, edit, |project| {
        let (w, h) = (project.meta.stage_width, project.meta.stage_height);
        let q0rg_id = setup.q0rg_id;
        *project.runtime.scene3d.setup_mut(q0rg_id, w, h) = setup;
        project.runtime.scene3d.sanitize();
    });
    if edit.changed {
        app.textures.invalidate();
    }
}

fn drag_v3(edit: &mut Edit, ui: &mut Ui, label: &str, value: &mut V3, speed: f64) {
    ui.label(label);
    ui.horizontal(|ui| {
        for (axis, v) in [
            ("x", &mut value.x),
            ("y", &mut value.y),
            ("z", &mut value.z),
        ] {
            ui.label(RichText::new(axis).small());
            edit.drag(ui, v, speed, -100_000.0..=100_000.0, "");
        }
    });
    ui.end_row();
}

/// Moves a camera or light position around its target.
fn orbit(position: V3, target: V3, yaw: f32, pitch: f32, scale: f32) -> V3 {
    let offset = position - target;
    let turned = Quat::from_axis_angle(V3::Y, yaw.to_radians()).rotate(offset);
    let side = turned.cross(V3::Y).normalized();
    let tilted = if side == V3::ZERO {
        turned
    } else {
        Quat::from_axis_angle(side, pitch.to_radians()).rotate(turned)
    };
    target + tilted * scale
}

pub fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.session.scene3d.open {
        return;
    }
    let mut open = true;
    egui::Window::new("3D")
        .open(&mut open)
        .default_size([380.0, 620.0])
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-820.0, 100.0))
        .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_source("scene3d_window")
                .auto_shrink([false, false])
                .show(ui, |ui| render_contents(app, ui));
        });
    if !open {
        app.session.scene3d.open = false;
    }
}

fn render_contents(app: &mut EditorApp, ui: &mut Ui) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("New model")
            .on_hover_text("Start a model from a cube and open the model editor")
            .clicked()
        {
            app.queue(crate::app::Action::NewModel);
        }
        if ui
            .button("Import model…")
            .on_hover_text("FBX, glTF, GLB or OBJ")
            .clicked()
        {
            app.queue(crate::app::Action::ImportModel);
        }
    });
    ui.label(
        RichText::new("Models and anything placed in 3D share a camera and these lights. Drawings stay flat unless placed in 3D or lit below.")
            .small()
            .color(dim),
    );
    ui.separator();
    let mut setup = setup_of(app, q0rg_id);
    let mut edit = Edit::default();
    let (stage_w, stage_h) = (
        app.state.project.meta.stage_width,
        app.state.project.meta.stage_height,
    );

    // Camera.
    ui.label(RichText::new("Camera").strong());
    ui.horizontal(|ui| {
        edit.checkbox(ui, &mut setup.camera.enabled, "Own camera");
        if setup.camera.enabled {
            edit.checkbox(ui, &mut setup.camera.orthographic, "No perspective");
        }
    });
    if setup.camera.enabled {
        let mut pose = setup.camera.pose_at(frame, stage_w, stage_h);
        let keyed = setup.camera.keys.iter().any(|key| key.frame == frame);
        let before = pose;
        egui::Grid::new("scene3d_camera")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                drag_v3(&mut edit, ui, "Position", &mut pose.position, 1.0);
                drag_v3(&mut edit, ui, "Looks at", &mut pose.target, 1.0);
                ui.label("Roll");
                edit.drag(ui, &mut pose.roll, 0.5, -360.0..=360.0, "°");
                ui.end_row();
                ui.label("Zoom");
                edit.drag(ui, &mut pose.zoom, 2.0, 10.0..=20_000.0, " px");
                ui.end_row();
            });
        ui.horizontal_wrapped(|ui| {
            let mut nudge =
                |label: &str, hint: &str, yaw: f32, pitch: f32, scale: f32, edit: &mut Edit| {
                    if ui.small_button(label).on_hover_text(hint).clicked() {
                        pose.position = orbit(pose.position, pose.target, yaw, pitch, scale);
                        edit.changed = true;
                        edit.snapshot = true;
                    }
                };
            nudge("⟲", "Orbit left", -15.0, 0.0, 1.0, &mut edit);
            nudge("⟳", "Orbit right", 15.0, 0.0, 1.0, &mut edit);
            nudge("⤒", "Look from higher", 0.0, 10.0, 1.0, &mut edit);
            nudge("⤓", "Look from lower", 0.0, -10.0, 1.0, &mut edit);
            nudge("+", "Move closer", 0.0, 0.0, 0.85, &mut edit);
            nudge("−", "Move away", 0.0, 0.0, 1.0 / 0.85, &mut edit);
            if ui
                .small_button("Reset")
                .on_hover_text("Back to looking straight at the stage")
                .clicked()
            {
                pose = CameraPose::stage(stage_w, stage_h);
                edit.changed = true;
                edit.snapshot = true;
            }
        });
        if pose != before {
            setup.camera.pose = pose;
            if !setup.camera.keys.is_empty() {
                setup.camera.set_key(frame);
            }
        }
        ui.horizontal(|ui| {
            if ui
                .button(if keyed {
                    "Update key"
                } else {
                    "Key camera here"
                })
                .clicked()
            {
                setup.camera.pose = pose;
                setup.camera.set_key(frame);
                edit.changed = true;
                edit.snapshot = true;
            }
            if keyed && ui.button("Remove key").clicked() {
                setup.camera.keys.retain(|key| key.frame != frame);
                setup.camera.pose = pose;
                edit.changed = true;
                edit.snapshot = true;
            }
            ui.label(
                RichText::new(format!("{} key(s)", setup.camera.keys.len()))
                    .small()
                    .color(dim),
            );
        });
    } else {
        ui.label(
            RichText::new("The stage camera: depth 0 looks exactly as drawn.")
                .small()
                .color(dim),
        );
    }
    ui.separator();

    // Light.
    ui.label(RichText::new("Light").strong());
    egui::Grid::new("scene3d_light")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Everywhere");
            ui.horizontal(|ui| {
                edit.color(ui, &mut setup.ambient);
                edit.slider(ui, &mut setup.ambient_intensity, 0.0..=3.0, false);
            });
            ui.end_row();
        });
    ui.horizontal(|ui| {
        edit.checkbox(ui, &mut setup.fog.on, "Haze");
        if setup.fog.on {
            edit.color(ui, &mut setup.fog.color);
        }
    })
    .response
    .on_hover_text("Far 3D things fade into the haze's colour");
    if setup.fog.on {
        egui::Grid::new("scene3d_fog")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Starts at");
                edit.slider(ui, &mut setup.fog.near, 0.0..=20_000.0, true);
                ui.end_row();
                ui.label("Thick at");
                edit.slider(ui, &mut setup.fog.far, 1.0..=40_000.0, true);
                ui.end_row();
            });
        if setup.fog.far <= setup.fog.near {
            setup.fog.far = setup.fog.near + 1.0;
        }
    }
    edit.checkbox(ui, &mut setup.light_flat, "Light flat drawings too");
    edit.checkbox(
        ui,
        &mut setup.shadows,
        "Shadows (suns and spots that cast them)",
    );
    if setup.light_flat {
        let layers: Vec<(u16, String)> = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .map(|q0rg| {
                q0rg.layers
                    .iter()
                    .map(|layer| (layer.layer_id, layer.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let shown = setup
            .light_flat_layer
            .and_then(|id| {
                layers
                    .iter()
                    .find(|(l, _)| *l == id)
                    .map(|(_, n)| format!("up to {n}"))
            })
            .unwrap_or_else(|| "every layer".to_string());
        ui.horizontal(|ui| {
            ui.label("Lit");
            egui::ComboBox::from_id_source("scene3d_flat_layer")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    let response =
                        ui.selectable_value(&mut setup.light_flat_layer, None, "every layer");
                    edit.note(&response);
                    for (id, name) in &layers {
                        let response = ui.selectable_value(
                            &mut setup.light_flat_layer,
                            Some(*id),
                            format!("up to {name}"),
                        );
                        edit.note(&response);
                    }
                });
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Add");
        for (label, kind) in [
            ("Bulb", Light3dKind::Point),
            ("Spot", Light3dKind::Spot),
            ("Sun", Light3dKind::Sun),
        ] {
            if ui.button(label).clicked() {
                let (w, h) = (f32::from(stage_w), f32::from(stage_h));
                let id = setup.next_light_id();
                let (position, target) = match kind {
                    Light3dKind::Sun => (
                        V3::new(w * 0.2, -h * 0.4, -600.0),
                        V3::new(w * 0.5, h * 0.5, 0.0),
                    ),
                    _ => (
                        V3::new(w * 0.35, h * 0.3, -260.0),
                        V3::new(w * 0.5, h * 0.55, 0.0),
                    ),
                };
                let mut light = Light3d::new(id, kind, position, target);
                if kind == Light3dKind::Spot {
                    light.intensity = 1.6;
                    light.range = 1600.0;
                }
                setup.lights.push(light);
                app.session.scene3d.selected_light = Some(id);
                edit.changed = true;
                edit.snapshot = true;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Looks like");
        for (name, hint) in [
            ("Studio", "A key, a fill and a rim light"),
            ("Sunset", "Low orange sun, blue shadows"),
            ("Night", "A dark scene with one warm lamp"),
        ] {
            if ui.small_button(name).on_hover_text(hint).clicked() {
                preset(&mut setup, name, stage_w, stage_h);
                app.session.scene3d.selected_light = setup.lights.first().map(|l| l.light_id);
                edit.changed = true;
                edit.snapshot = true;
            }
        }
    });
    if setup.lights.is_empty() {
        ui.label(
            RichText::new(
                "No lights yet: models get a soft default light; drawings stay as drawn.",
            )
            .small()
            .color(dim),
        );
    }
    let mut remove = None;
    for light in &mut setup.lights {
        ui.horizontal(|ui| {
            edit.checkbox(ui, &mut light.enabled, "");
            let selected = app.session.scene3d.selected_light == Some(light.light_id);
            let kind = match light.kind {
                Light3dKind::Point => "bulb",
                Light3dKind::Spot => "spot",
                Light3dKind::Sun => "sun",
            };
            if ui
                .selectable_label(selected, format!("{} ({kind})", light.name))
                .clicked()
            {
                app.session.scene3d.selected_light =
                    if selected { None } else { Some(light.light_id) };
            }
            if ui
                .small_button("✕")
                .on_hover_text("Remove this light")
                .clicked()
            {
                remove = Some(light.light_id);
            }
        });
    }
    if let Some(id) = remove {
        setup.lights.retain(|light| light.light_id != id);
        edit.changed = true;
        edit.snapshot = true;
    }
    let selected_id = app.session.scene3d.selected_light;
    let follow_target = selected_instance(app);
    if let Some(light) = selected_id.and_then(|id| setup.light_mut(id)) {
        ui.separator();
        let key = light.animated(frame);
        let mut shown = Light3d {
            position: key.position,
            target: key.target,
            color: key.color,
            intensity: key.intensity,
            range: key.range,
            ..light.clone()
        };
        let before = shown.clone();
        egui::Grid::new("scene3d_light_props")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Name");
                edit.text(ui, &mut shown.name, 160.0);
                ui.end_row();
                ui.label("Kind");
                ui.horizontal(|ui| {
                    edit.choice(ui, &mut shown.kind, Light3dKind::Point, "Bulb");
                    edit.choice(ui, &mut shown.kind, Light3dKind::Spot, "Spot");
                    edit.choice(ui, &mut shown.kind, Light3dKind::Sun, "Sun");
                });
                ui.end_row();
                ui.label("Colour");
                edit.color(ui, &mut shown.color);
                ui.end_row();
                ui.label("Brightness");
                edit.slider(ui, &mut shown.intensity, 0.0..=8.0, false);
                ui.end_row();
                if shown.kind != Light3dKind::Sun {
                    ui.label("Reach");
                    edit.slider(ui, &mut shown.range, 0.0..=5000.0, true);
                    ui.end_row();
                }
                if shown.kind == Light3dKind::Spot {
                    ui.label("Cone");
                    edit.slider(ui, &mut shown.cone, 1.0..=170.0, false);
                    ui.end_row();
                    ui.label("Soft edge");
                    edit.slider(ui, &mut shown.softness, 0.0..=1.0, false);
                    ui.end_row();
                }
                if shown.kind != Light3dKind::Point {
                    ui.label("Shadows");
                    edit.checkbox(ui, &mut shown.shadows, "casts shadows");
                    ui.end_row();
                }
                drag_v3(
                    &mut edit,
                    ui,
                    if shown.follow.is_some() {
                        "Offset"
                    } else {
                        "Position"
                    },
                    &mut shown.position,
                    1.0,
                );
                if shown.kind != Light3dKind::Point {
                    drag_v3(&mut edit, ui, "Points at", &mut shown.target, 1.0);
                }
            });
        ui.label(
            RichText::new("Depth: less than 0 is in front of the stage, toward the camera.")
                .small()
                .color(dim),
        );
        ui.horizontal_wrapped(|ui| match shown.follow {
            Some(instance) => {
                ui.label(format!("Rides with object {instance}"));
                if ui.small_button("Let go").clicked() {
                    shown.follow = None;
                }
            }
            None => {
                if ui
                    .add_enabled(
                        follow_target.is_some(),
                        egui::Button::new("Ride with selected").small(),
                    )
                    .on_hover_text("The light moves and turns with the selected object")
                    .clicked()
                {
                    shown.follow = follow_target;
                    shown.position = V3::new(0.0, -120.0, -160.0);
                    shown.target = V3::ZERO;
                }
            }
        });
        ui.horizontal(|ui| {
            let keyed = shown.keys.iter().any(|key| key.frame == frame);
            if ui
                .button(if keyed {
                    "Update key"
                } else {
                    "Key light here"
                })
                .clicked()
            {
                shown.set_key(frame);
                edit.changed = true;
                edit.snapshot = true;
            }
            if keyed && ui.button("Remove key").clicked() {
                shown.keys.retain(|key| key.frame != frame);
                edit.changed = true;
                edit.snapshot = true;
            }
            ui.label(
                RichText::new(format!("{} key(s)", shown.keys.len()))
                    .small()
                    .color(dim),
            );
        });
        if shown != before {
            if !shown.keys.is_empty()
                && !shown
                    .keys
                    .iter()
                    .any(|key| key.frame == frame && before.keys.iter().all(|k| k.frame != frame))
            {
                shown.set_key(frame);
            }
            *light = shown;
            edit.changed = true;
        }
    }
    if edit.changed || edit.snapshot {
        store_setup(app, &edit, setup);
    }
    ui.separator();
    let mut frames = match app.session.scene3d.bake_frames {
        0 => crate::physics3d::DEFAULT_FRAMES,
        frames => frames,
    };
    crate::physics3d::world_controls(app, ui, &mut frames);
    app.session.scene3d.bake_frames = frames;
}

fn preset(setup: &mut Scene3dSetup, name: &str, w: u16, h: u16) {
    let (w, h) = (f32::from(w), f32::from(h));
    let centre = V3::new(w * 0.5, h * 0.5, 0.0);
    let color = |r, g, b| Rgba { r, g, b, a: 255 };
    setup.lights.clear();
    let mut add = |kind, position: V3, c: Rgba, intensity: f32| {
        let id = setup.next_light_id();
        let mut light = Light3d::new(id, kind, position, centre);
        light.color = c;
        light.intensity = intensity;
        light.range = 0.0;
        setup.lights.push(light);
    };
    match name {
        "Studio" => {
            add(
                Light3dKind::Sun,
                V3::new(w * 0.1, -h * 0.3, -500.0),
                color(255, 246, 232),
                0.95,
            );
            add(
                Light3dKind::Sun,
                V3::new(w * 1.1, h * 0.3, -300.0),
                color(170, 190, 255),
                0.35,
            );
            add(
                Light3dKind::Sun,
                V3::new(w * 0.6, -h * 0.6, 600.0),
                color(255, 255, 255),
                0.6,
            );
            setup.ambient = color(80, 84, 96);
            setup.ambient_intensity = 1.0;
        }
        "Sunset" => {
            add(
                Light3dKind::Sun,
                V3::new(-w * 0.5, h * 0.35, -200.0),
                color(255, 150, 80),
                1.3,
            );
            setup.ambient = color(60, 70, 130);
            setup.ambient_intensity = 0.9;
        }
        _ => {
            let id = setup.next_light_id();
            let mut lamp = Light3d::new(
                id,
                Light3dKind::Point,
                V3::new(w * 0.4, h * 0.35, -180.0),
                centre,
            );
            lamp.color = color(255, 200, 130);
            lamp.intensity = 1.8;
            lamp.range = 900.0;
            setup.lights.push(lamp);
            setup.ambient = color(18, 22, 44);
            setup.ambient_intensity = 1.0;
        }
    }
}

/// The selected placement's instance id, giving it one if it has none.
fn selected_instance(app: &EditorApp) -> Option<u32> {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection
    else {
        return None;
    };
    let placement = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == layer_id)?
        .placements
        .get(placement_idx)?;
    (placement.instance_id != 0).then_some(placement.instance_id)
}

/// The 3D part of a placement's properties. Edits `fx` like the other
/// effect controls do; bone poses go straight to the timeline's pose keys.
#[allow(clippy::too_many_arguments)]
pub(crate) fn placement_3d_properties(
    app: &mut EditorApp,
    ui: &mut Ui,
    reference: PlacementRef,
    target: Target,
    fx: &mut PlacementFx,
    current_frame: u16,
    playback_start_frame: u16,
    fx_dirty: &mut bool,
    wants_snapshot: &mut bool,
) {
    let dim = app.settings.theme.text_dim.to_color32();
    let model: Option<(u16, Arc<ModelData>)> = match target {
        Target::Asset(id) => app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Model(ModelAsset { asset_id, data }) if *asset_id == id => {
                    Some((id, data.clone()))
                }
                _ => None,
            }),
        Target::Q0rg(_) => None,
    };
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("3D").strong());
        if let Some((asset_id, _)) = &model {
            if ui.button("Edit model…").clicked() {
                app.queue(crate::app::Action::EditModel(*asset_id));
            }
        }
        if ui
            .small_button("Scene…")
            .on_hover_text("Camera and 3D lights")
            .clicked()
        {
            app.session.scene3d.open = true;
        }
    });
    let note = |response: &egui::Response, fx_dirty: &mut bool, wants_snapshot: &mut bool| {
        if response.drag_started()
            || response.gained_focus()
            || (response.changed() && !response.dragged())
        {
            *wants_snapshot = true;
        }
        if response.changed() {
            *fx_dirty = true;
        }
    };
    if model.is_none() {
        let mut placed = fx.three.is_some();
        let response = ui.checkbox(&mut placed, "Place in 3D").on_hover_text(
            "Stand this drawing in 3D as a flat card: push it back, turn it, light it",
        );
        if response.changed() {
            fx.three = placed.then(Space3d::default);
            *fx_dirty = true;
            *wants_snapshot = true;
        }
    }
    if model.is_none() && fx.three.is_none() {
        return;
    }
    let mut space = fx.three.unwrap_or_default();
    egui::Grid::new("placement_3d")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Depth");
            let response = ui.add(egui::DragValue::new(&mut space.z).speed(1.0).suffix(" px"));
            note(&response, fx_dirty, wants_snapshot);
            ui.end_row();
            ui.label("Lean back");
            let response = ui.add(
                egui::DragValue::new(&mut space.tilt_x)
                    .speed(0.5)
                    .suffix("°"),
            );
            note(&response, fx_dirty, wants_snapshot);
            ui.end_row();
            ui.label("Turn");
            let response = ui.add(
                egui::DragValue::new(&mut space.tilt_y)
                    .speed(0.5)
                    .suffix("°"),
            );
            note(&response, fx_dirty, wants_snapshot);
            ui.end_row();
            ui.label("Lit");
            let response = ui.checkbox(&mut space.lit, "by the 3D lights");
            note(&response, fx_dirty, wants_snapshot);
            ui.end_row();
        });
    if let Some((_, data)) = &model {
        ui.label(RichText::new("Animation").strong());
        egui::Grid::new("placement_3d_clip")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Clip");
                let name = |clip: u16| {
                    if clip == NO_CLIP {
                        "rest pose".to_string()
                    } else {
                        data.clips
                            .get(usize::from(clip))
                            .map_or("missing".into(), |c| c.name.clone())
                    }
                };
                egui::ComboBox::from_id_source("placement_3d_clip_combo")
                    .selected_text(name(space.clip))
                    .show_ui(ui, |ui| {
                        let response = ui.selectable_value(&mut space.clip, NO_CLIP, "rest pose");
                        note(&response, fx_dirty, wants_snapshot);
                        for (i, clip) in data.clips.iter().enumerate() {
                            let response = ui.selectable_value(
                                &mut space.clip,
                                i as u16,
                                format!("{} ({:.2}s)", clip.name, clip.duration),
                            );
                            note(&response, fx_dirty, wants_snapshot);
                        }
                    });
                ui.end_row();
                if space.clip != NO_CLIP {
                    ui.label("Start at");
                    let response = ui.add(
                        egui::DragValue::new(&mut space.clip_time)
                            .speed(0.01)
                            .suffix(" s"),
                    );
                    note(&response, fx_dirty, wants_snapshot);
                    ui.end_row();
                    ui.label("Speed");
                    let response = ui.add(
                        egui::DragValue::new(&mut space.clip_speed)
                            .speed(0.01)
                            .clamp_range(-8.0..=8.0),
                    );
                    note(&response, fx_dirty, wants_snapshot);
                    ui.end_row();
                    ui.label("Loop");
                    let response = ui.checkbox(&mut space.clip_loop, "");
                    note(&response, fx_dirty, wants_snapshot);
                    ui.end_row();
                }
            });
        if space.clip != NO_CLIP {
            ui.label(
                RichText::new("Speed 0 holds the clip still: key \"Start at\" on two keyframes to scrub through it.")
                    .small()
                    .color(dim),
            );
        }
        let elapsed = f32::from(current_frame.saturating_sub(playback_start_frame));
        pose_bones(
            app,
            ui,
            reference,
            data,
            &space,
            current_frame,
            elapsed,
            wants_snapshot,
        );
    }
    if fx.three != Some(space) && (model.is_some() || fx.three.is_some()) {
        fx.three = Some(space);
        *fx_dirty = true;
    }
    ui.add_space(4.0);
    let instance = instance_of(app, reference).unwrap_or(0);
    crate::physics3d::body_controls(app, ui, reference, instance);
}

/// Bones of a placed model, turned on the timeline.
#[allow(clippy::too_many_arguments)]
fn pose_bones(
    app: &mut EditorApp,
    ui: &mut Ui,
    reference: PlacementRef,
    data: &ModelData,
    space: &Space3d,
    frame: u16,
    elapsed: f32,
    wants_snapshot: &mut bool,
) {
    let bones: Vec<(u16, String)> = data
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.bone)
        .map(|(i, n)| (i as u16, n.name.clone()))
        .collect();
    if bones.is_empty() {
        return;
    }
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = reference.q0rg_id;
    let instance_id = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|l| l.layer_id == reference.layer_id))
        .and_then(|l| l.placements.get(reference.placement_idx))
        .map_or(0, |p| p.instance_id);
    egui::CollapsingHeader::new(RichText::new(format!("Pose ({} bones)", bones.len())).strong())
        .id_source("placement_3d_pose")
        .show(ui, |ui| {
            let selected = app
                .session
                .scene3d
                .pose_bone
                .filter(|b| bones.iter().any(|(i, _)| i == b));
            egui::ComboBox::from_id_source("placement_3d_bone")
                .selected_text(
                    selected
                        .and_then(|b| bones.iter().find(|(i, _)| *i == b))
                        .map_or("pick a bone", |(_, n)| n.as_str()),
                )
                .height(320.0)
                .show_ui(ui, |ui| {
                    for (i, name) in &bones {
                        if ui.selectable_label(selected == Some(*i), name).clicked() {
                            app.session.scene3d.pose_bone = Some(*i);
                        }
                    }
                });
            let Some(bone) = app
                .session
                .scene3d
                .pose_bone
                .filter(|b| bones.iter().any(|(i, _)| i == b))
            else {
                ui.label(
                    RichText::new("Turn bones here; each change keys this frame.")
                        .small()
                        .color(dim),
                );
                return;
            };
            // What the bone does now: the clip, with any keyed pose on top.
            let track = app
                .state
                .project
                .runtime
                .scene3d
                .pose(q0rg_id, instance_id)
                .cloned();
            let clip = (space.clip != NO_CLIP).then_some(usize::from(space.clip));
            let time = clip.and_then(|c| data.clips.get(c)).map_or(0.0, |c| {
                space.clip_seconds(elapsed, app.state.project.meta.fps, c.duration)
            });
            let mut locals = data.sample_clip(clip, time);
            if let Some(track) = &track {
                track.apply(frame, &mut locals);
            }
            let Some(local) = locals.get(usize::from(bone)).copied() else {
                return;
            };
            let mut euler = local.rotation.to_euler_deg();
            let before = euler;
            let mut changed = false;
            egui::Grid::new("placement_3d_bone_rot")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Turn x", &mut euler.x),
                        ("Turn y", &mut euler.y),
                        ("Turn z", &mut euler.z),
                    ] {
                        ui.label(label);
                        let response = ui.add(egui::DragValue::new(value).speed(0.5).suffix("°"));
                        if response.drag_started() || response.gained_focus() {
                            *wants_snapshot = true;
                        }
                        changed |= response.changed();
                        ui.end_row();
                    }
                });
            let keyed = track.as_ref().is_some_and(|t| {
                t.keys
                    .iter()
                    .any(|k| k.frame == frame && k.bones.iter().any(|(n, _)| *n == bone))
            });
            ui.horizontal(|ui| {
                if keyed && ui.small_button("Clear bone key").clicked() {
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    let track = app
                        .state
                        .project
                        .runtime
                        .scene3d
                        .pose_mut(q0rg_id, instance_id);
                    for key in &mut track.keys {
                        if key.frame == frame {
                            key.bones.retain(|(n, _)| *n != bone);
                        }
                    }
                    track.keys.retain(|key| !key.bones.is_empty());
                    app.state.project.runtime.scene3d.sanitize();
                    app.state.mark_dirty();
                }
                if ui
                    .small_button("Back to the clip")
                    .on_hover_text("Remove every pose key of this model")
                    .clicked()
                {
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    app.state
                        .project
                        .runtime
                        .scene3d
                        .poses
                        .retain(|t| !(t.q0rg_id == q0rg_id && t.instance_id == instance_id));
                    app.state.mark_dirty();
                }
            });
            if changed && euler != before {
                if *wants_snapshot {
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    *wants_snapshot = false;
                }
                let instance_id = if instance_id == 0 {
                    assign_instance_id(app, reference)
                } else {
                    instance_id
                };
                if instance_id == 0 {
                    return;
                }
                let rotation = Quat::from_euler_deg(euler.x, euler.y, euler.z);
                let track = app
                    .state
                    .project
                    .runtime
                    .scene3d
                    .pose_mut(q0rg_id, instance_id);
                let key = track.key_at_mut(frame);
                let value = Trs { rotation, ..local };
                match key.bones.iter_mut().find(|(n, _)| *n == bone) {
                    Some((_, trs)) => *trs = value,
                    None => key.bones.push((bone, value)),
                }
                app.state.mark_dirty();
                app.textures.invalidate();
            }
        });
}

/// Gives a placement (and its keyframes along the layer) a fresh instance
/// id so pose keys can find it.
/// The object id of a placement, once it has one.
pub(crate) fn instance_of(app: &EditorApp, reference: PlacementRef) -> Option<u32> {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)
        .map(|p| p.instance_id)
        .filter(|id| *id != 0)
}

pub(crate) fn assign_instance_id(app: &mut EditorApp, reference: PlacementRef) -> u32 {
    let project = &mut app.state.project;
    let next = project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .map(|p| p.instance_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let Some(layer) = project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == reference.q0rg_id)
        .and_then(|q| {
            q.layers
                .iter_mut()
                .find(|l| l.layer_id == reference.layer_id)
        })
    else {
        return 0;
    };
    let Some(target) = layer
        .placements
        .get(reference.placement_idx)
        .map(|p| p.target)
    else {
        return 0;
    };
    // Every keyframe of this object on the layer is the same object.
    for placement in layer
        .placements
        .iter_mut()
        .filter(|p| p.target == target && p.instance_id == 0)
    {
        placement.instance_id = next;
    }
    next
}

/// Adds a model to the library and places it in the middle of the stage.
pub(crate) fn add_model(app: &mut EditorApp, name: &str, data: ModelData) -> Option<u16> {
    let asset_id = app
        .state
        .project
        .assets
        .iter()
        .map(Asset::id)
        .max()
        .unwrap_or(0)
        .checked_add(1)?;
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    app.state.project.assets.push(Asset::Model(ModelAsset {
        asset_id,
        data: Arc::new(data),
    }));
    app.state
        .project
        .asset_names
        .insert(asset_id, name.to_string());
    app.state.mark_dirty();
    app.textures.invalidate();
    Some(asset_id)
}

pub(crate) fn stage_centre(app: &EditorApp) -> q0s_format::v2::Vec2 {
    q0s_format::v2::Vec2::new(
        f32::from(app.state.project.meta.stage_width) * 0.5,
        f32::from(app.state.project.meta.stage_height) * 0.5,
    )
}
