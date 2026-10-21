//! The light editor: lamps that really light the stage. What is drawn
//! under the light layer is multiplied by the light reaching it, so a lamp
//! shows the colours under it where a white disc would only wash them out.
//! Nothing is baked: the editor, exports and q0player all light the frame
//! themselves.
//!
//! Opt-in like physics and particles: while `Settings::light_enabled` is off
//! the window and stage gizmos stay hidden. Lights a project already has
//! keep shining either way.

use egui::{Color32, Context, Painter, RichText, Stroke, Ui};
use q0s_format::lighting::{Light, LightKind, LightSetup};
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::v2::{ProjectV2, Rgba, Vec2};

use crate::app::EditorApp;
use crate::physics::{apply_edit, follow_target, has_selection_on, selection_area, Edit};
use crate::render::StageView;
use crate::state::RigMode;
use crate::ui::FitToScreen;

/// The light layer's name when one is made for a timeline.
const LIGHT_LAYER: &str = "Light";

#[derive(Default)]
pub struct LightSession {
    pub open: bool,
    pub mode: RigMode,
    pub selected: Option<u16>,
    pub filter: String,
    drag: Option<LightDrag>,
}

/// Which part of a light the pointer holds on the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grip {
    Move,
    Radius,
    Aim,
}

#[derive(Debug, Clone, Copy)]
struct LightDrag {
    light_id: u16,
    grip: Grip,
    press: Vec2,
    /// Where the light was, as stored (offset for followers).
    position: [f32; 2],
    /// Where the light's centre is on the stage.
    centre: [f32; 2],
}

// ------------------------------------------------------------------ helpers

fn rgba(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 255 }
}

/// Where a followed object's middle is on `frame`.
fn anchor(project: &ProjectV2, q0rg_id: u16, frame: u16, instance_id: u32) -> Option<[f32; 2]> {
    q0s_format::raster::runtime_instance_metrics(
        project,
        q0rg_id,
        frame,
        instance_id,
        &RuntimeSceneState::new(),
    )
    .map(|metrics| {
        [
            (metrics.left + metrics.right) * 0.5,
            (metrics.top + metrics.bottom) * 0.5,
        ]
    })
}

/// `light` as it is on `frame`: its keys played into its plain values.
fn on_frame(light: &Light, frame: u16) -> Light {
    let mut shown = light.clone();
    if !light.keys.is_empty() {
        let key = light.animated(frame);
        shown.position = key.position;
        shown.color = key.color;
        shown.intensity = key.intensity;
        shown.radius = key.radius;
        shown.direction = key.direction;
    }
    shown
}

/// Where `light`'s centre is on the stage on `frame`.
fn light_centre(project: &ProjectV2, q0rg_id: u16, light: &Light, frame: u16) -> Option<[f32; 2]> {
    let shown = on_frame(light, frame);
    let [x, y] = shown.position;
    match light.follow {
        Some(instance_id) => {
            let [ax, ay] = anchor(project, q0rg_id, frame, instance_id)?;
            Some([ax + x, ay + y])
        }
        None => Some([x, y]),
    }
}

/// Stores `light` into `q0rg_id`'s setup. A light with keys keys the
/// change on `frame`, so animated lights are edited where they are.
fn store_light(app: &mut EditorApp, edit: &Edit, q0rg_id: u16, mut light: Light, frame: u16) {
    if !light.keys.is_empty() {
        light.set_key(frame);
    }
    apply_edit(app, edit, |project| {
        let setup = project.runtime.lighting.setup_mut(q0rg_id);
        if let Some(stored) = setup.light_mut(light.light_id) {
            *stored = light;
        }
        project.runtime.lighting.sanitize();
    });
}

fn store_setup(app: &mut EditorApp, edit: &Edit, setup: LightSetup) {
    apply_edit(app, edit, |project| {
        let q0rg_id = setup.q0rg_id;
        *project.runtime.lighting.setup_mut(q0rg_id) = setup;
        project.runtime.lighting.sanitize();
    });
}

/// Gives `q0rg_id` a light layer on top if it has none, so lights shine on
/// everything drawn so far and layers added above stay unlit.
fn ensure_light_layer(project: &mut ProjectV2, q0rg_id: u16) {
    let current = project
        .runtime
        .lighting
        .setup(q0rg_id)
        .and_then(|setup| setup.layer_id);
    if let Ok(layer_id) = crate::sim_bake::effect_layer(project, q0rg_id, current, LIGHT_LAYER) {
        project.runtime.lighting.setup_mut(q0rg_id).layer_id = Some(layer_id);
    }
}

fn layer_name(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<String> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .map(|layer| layer.name.clone())
}

// ------------------------------------------------------------------ presets

struct Mood {
    name: &'static str,
    darkness: f32,
    ambient: Rgba,
}

const MOODS: [Mood; 6] = [
    Mood {
        name: "Day",
        darkness: 0.0,
        ambient: Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
    },
    Mood {
        name: "Sunset",
        darkness: 0.35,
        ambient: Rgba {
            r: 255,
            g: 130,
            b: 80,
            a: 255,
        },
    },
    Mood {
        name: "Dusk",
        darkness: 0.6,
        ambient: Rgba {
            r: 90,
            g: 70,
            b: 140,
            a: 255,
        },
    },
    Mood {
        name: "Night",
        darkness: 0.85,
        ambient: Rgba {
            r: 20,
            g: 26,
            b: 52,
            a: 255,
        },
    },
    Mood {
        name: "Underwater",
        darkness: 0.7,
        ambient: Rgba {
            r: 10,
            g: 90,
            b: 120,
            a: 255,
        },
    },
    Mood {
        name: "Cave",
        darkness: 0.97,
        ambient: Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
    },
];

struct LightPreset {
    name: &'static str,
    hint: &'static str,
    apply: fn(&mut Light),
}

/// Back to a plain lamp, keeping what makes this light this light.
fn plain(light: &mut Light) {
    let kept = light.clone();
    *light = Light::new(kept.light_id, kept.position);
    light.name = kept.name;
    light.follow = kept.follow;
    light.enabled = kept.enabled;
    light.keys = kept.keys;
}

const LIGHT_PRESETS: [LightPreset; 10] = [
    LightPreset {
        name: "Lamp",
        hint: "a warm, steady room light",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 214, 150);
            l.radius = 220.0;
            l.softness = 0.85;
            l.glow = 0.15;
        },
    },
    LightPreset {
        name: "Torch",
        hint: "orange and flickering",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 150, 60);
            l.intensity = 1.2;
            l.radius = 200.0;
            l.softness = 0.8;
            l.glow = 0.35;
            l.flicker = 0.35;
            l.flicker_speed = 10.0;
        },
    },
    LightPreset {
        name: "Candle",
        hint: "small, soft and wavering",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 190, 110);
            l.intensity = 0.9;
            l.radius = 110.0;
            l.softness = 0.9;
            l.glow = 0.4;
            l.flicker = 0.25;
            l.flicker_speed = 14.0;
        },
    },
    LightPreset {
        name: "Campfire",
        hint: "big, warm and restless",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 120, 40);
            l.intensity = 1.4;
            l.radius = 300.0;
            l.softness = 0.85;
            l.glow = 0.4;
            l.flicker = 0.4;
            l.flicker_speed = 7.0;
        },
    },
    LightPreset {
        name: "Neon",
        hint: "a hot pink sign",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 60, 200);
            l.intensity = 1.3;
            l.radius = 160.0;
            l.softness = 0.6;
            l.glow = 0.6;
        },
    },
    LightPreset {
        name: "Ice",
        hint: "a cold cyan glow",
        apply: |l| {
            plain(l);
            l.color = rgba(120, 220, 255);
            l.intensity = 1.1;
            l.radius = 180.0;
            l.softness = 0.7;
            l.glow = 0.3;
        },
    },
    LightPreset {
        name: "Moon",
        hint: "a wide, faint blue wash",
        apply: |l| {
            plain(l);
            l.color = rgba(170, 190, 255);
            l.intensity = 0.7;
            l.radius = 500.0;
            l.softness = 1.0;
            l.glow = 0.05;
        },
    },
    LightPreset {
        name: "Spotlight",
        hint: "a cone of light; drag its arrow to aim it",
        apply: |l| {
            plain(l);
            l.kind = LightKind::Spot;
            l.color = rgba(255, 245, 220);
            l.intensity = 1.4;
            l.radius = 420.0;
            l.softness = 0.5;
            l.direction = 90.0;
            l.cone = 40.0;
            l.cone_softness = 0.35;
            l.glow = 0.1;
        },
    },
    LightPreset {
        name: "Siren",
        hint: "red, blinking on and off",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 40, 40);
            l.intensity = 1.5;
            l.radius = 250.0;
            l.softness = 0.7;
            l.glow = 0.5;
            l.flicker = 1.0;
            l.flicker_speed = 3.0;
        },
    },
    LightPreset {
        name: "Sun",
        hint: "lights nearly everything",
        apply: |l| {
            plain(l);
            l.color = rgba(255, 240, 200);
            l.radius = 900.0;
            l.softness = 1.0;
            l.glow = 0.0;
        },
    },
];

/// Applies the light preset called `name`; false when there is none.
pub fn apply_preset(name: &str, light: &mut Light) -> bool {
    let Some(preset) = LIGHT_PRESETS.iter().find(|preset| preset.name == name) else {
        return false;
    };
    (preset.apply)(light);
    true
}

// ------------------------------------------------------------------- window

pub fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.settings.light_enabled || !app.session.lights.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Light")
        .open(&mut open)
        .default_size([380.0, 600.0])
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-860.0, 90.0))
        .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| render_contents(app, ui));
    if !open {
        app.session.lights.open = false;
    }
}

fn render_contents(app: &mut EditorApp, ui: &mut Ui) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    ui.horizontal(|ui| {
        ui.label("Mode");
        for mode in RigMode::ALL {
            ui.selectable_value(&mut app.session.lights.mode, mode, mode.label());
        }
    });
    let mode = app.session.lights.mode;
    ui.label(
        RichText::new(match mode {
            RigMode::Simple => "darken the scene, drop lamps on it, drag them around",
            RigMode::Pro => "spots, flicker, keyframes and which layers get lit",
        })
        .small()
        .color(dim),
    );
    ui.separator();
    render_scene(app, ui, q0rg_id, mode, dim);
    ui.separator();
    render_light_list(app, ui, q0rg_id, dim);
    let frame = app.session.current_frame;
    let Some(light) = app
        .session
        .lights
        .selected
        .and_then(|id| app.state.project.runtime.lighting.setup(q0rg_id)?.light(id))
        .cloned()
    else {
        return;
    };
    ui.separator();
    egui::ScrollArea::vertical()
        .id_source("light_setup")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut shown = on_frame(&light, frame);
            let mut edit = Edit::default();
            render_light(app, ui, &mut edit, &mut shown, mode, dim);
            if edit.follow_selection {
                if let Some(instance_id) = follow_target(app, q0rg_id) {
                    // The light keeps its place, now as an offset from the
                    // object's middle.
                    if let Some([ax, ay]) = anchor(&app.state.project, q0rg_id, frame, instance_id)
                    {
                        shown.position = [shown.position[0] - ax, shown.position[1] - ay];
                        if shown.position[0].hypot(shown.position[1]) > 400.0 {
                            shown.position = [0.0, 0.0];
                        }
                    }
                    shown.follow = Some(instance_id);
                    edit.snapshot = true;
                    edit.changed = true;
                }
            }
            if mode == RigMode::Pro {
                render_keys(app, ui, q0rg_id, &light, frame, dim);
            }
            if edit.changed || edit.snapshot {
                store_light(app, &edit, q0rg_id, shown, frame);
            }
        });
}

/// Darkness, its colour, moods and which layers get lit.
fn render_scene(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, mode: RigMode, dim: Color32) {
    let mut setup = app
        .state
        .project
        .runtime
        .lighting
        .setup(q0rg_id)
        .cloned()
        .unwrap_or_else(|| LightSetup::new(q0rg_id));
    let mut edit = Edit::default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Scene").strong());
        edit.checkbox(ui, &mut setup.enabled, "Lights on");
    });
    ui.horizontal_wrapped(|ui| {
        for mood in &MOODS {
            if ui.small_button(mood.name).clicked() {
                setup.darkness = mood.darkness;
                setup.ambient = mood.ambient;
                edit.snapshot = true;
                edit.changed = true;
            }
        }
    });
    egui::Grid::new("light_scene")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Darkness");
            edit.slider(ui, &mut setup.darkness, 0.0..=1.0, false);
            ui.end_row();
            ui.label("Dark colour");
            ui.horizontal(|ui| {
                edit.color(ui, &mut setup.ambient);
                ui.label(RichText::new("what the dark fades to").small().color(dim));
            });
            ui.end_row();
            if mode == RigMode::Pro {
                ui.label("Lights up to");
                let layers = app
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .map(|q0rg| {
                        q0rg.layers
                            .iter()
                            .rev()
                            .map(|layer| (layer.layer_id, layer.name.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let current = setup
                    .layer_id
                    .and_then(|id| layers.iter().find(|(layer_id, _)| *layer_id == id))
                    .map_or_else(|| "every layer".to_string(), |(_, name)| name.clone());
                egui::ComboBox::from_id_source("light_layer")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        edit.choice(ui, &mut setup.layer_id, None, "every layer");
                        for (layer_id, name) in &layers {
                            edit.choice(ui, &mut setup.layer_id, Some(*layer_id), name);
                        }
                    });
                ui.end_row();
            }
        });
    let note = match setup
        .layer_id
        .and_then(|id| layer_name(&app.state.project, q0rg_id, id))
    {
        Some(name) => format!(
            "Layers under \"{name}\" get lit; move layers above it to keep them as drawn (a HUD, say). \
             Hide \"{name}\" to see the scene without lights."
        ),
        None => "Every layer gets lit.".to_string(),
    };
    ui.label(RichText::new(note).small().color(dim));
    if edit.changed || edit.snapshot {
        store_setup(app, &edit, setup);
    }
}

fn render_light_list(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, dim: Color32) {
    ui.horizontal(|ui| {
        if ui
            .button("Add light")
            .on_hover_text("A lamp on the selected object, or in the middle of the stage")
            .clicked()
        {
            add_light(app, q0rg_id);
        }
        ui.add(
            egui::TextEdit::singleline(&mut app.session.lights.filter)
                .hint_text("Search lights")
                .desired_width(ui.available_width()),
        );
    });
    let lights = app
        .state
        .project
        .runtime
        .lighting
        .setup(q0rg_id)
        .map(|setup| {
            setup
                .lights
                .iter()
                .map(|light| {
                    (
                        light.light_id,
                        light.name.clone(),
                        light.color,
                        light.enabled,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if lights.is_empty() {
        ui.label(
            RichText::new("No lights on this timeline yet. Add one, then pick a look.")
                .small()
                .color(dim),
        );
        return;
    }
    if app
        .session
        .lights
        .selected
        .is_none_or(|selected| !lights.iter().any(|(id, ..)| *id == selected))
    {
        app.session.lights.selected = Some(lights[0].0);
    }
    let filter = app.session.lights.filter.trim().to_lowercase();
    let mut remove = None;
    let mut toggle = None;
    egui::ScrollArea::vertical()
        .id_source("light_list")
        .max_height(130.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let mut shown = 0;
            for (light_id, name, color, enabled) in &lights {
                if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                    continue;
                }
                shown += 1;
                ui.horizontal(|ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                    let fill = if *enabled {
                        Color32::from_rgb(color.r, color.g, color.b)
                    } else {
                        Color32::from_gray(70)
                    };
                    ui.painter().circle_filled(rect.center(), 5.5, fill);
                    if response
                        .on_hover_text(if *enabled { "Switch off" } else { "Switch on" })
                        .clicked()
                    {
                        toggle = Some(*light_id);
                    }
                    let selected = app.session.lights.selected == Some(*light_id);
                    if ui.selectable_label(selected, name).clicked() {
                        app.session.lights.selected = Some(*light_id);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(*light_id);
                        }
                    });
                });
            }
            if shown == 0 {
                ui.label(RichText::new("No light matches.").small().color(dim));
            }
        });
    if let Some(light_id) = toggle {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        if let Some(light) = app
            .state
            .project
            .runtime
            .lighting
            .setup_mut(q0rg_id)
            .light_mut(light_id)
        {
            light.enabled = !light.enabled;
        }
        app.state.mark_dirty();
    }
    if let Some(light_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        app.state
            .project
            .runtime
            .lighting
            .setup_mut(q0rg_id)
            .lights
            .retain(|light| light.light_id != light_id);
        app.state.mark_dirty();
    }
}

fn add_light(app: &mut EditorApp, q0rg_id: u16) {
    let stage = [
        f32::from(app.state.project.meta.stage_width.max(1)),
        f32::from(app.state.project.meta.stage_height.max(1)),
    ];
    let position = selection_area(app).map_or([stage[0] * 0.5, stage[1] * 0.5], |[x, y, w, h]| {
        [x + w * 0.5, y + h * 0.5]
    });
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let project = &mut app.state.project;
    let fresh = project.runtime.lighting.setup(q0rg_id).is_none();
    ensure_light_layer(project, q0rg_id);
    let setup = project.runtime.lighting.setup_mut(q0rg_id);
    if fresh {
        // A first light shows best in the dark.
        setup.darkness = 0.75;
    }
    let light_id = setup.next_light_id();
    setup.lights.push(Light::new(light_id, position));
    project.runtime.lighting.sanitize();
    app.session.lights.selected = Some(light_id);
    app.state.mark_dirty();
}

fn render_light(
    app: &EditorApp,
    ui: &mut Ui,
    edit: &mut Edit,
    light: &mut Light,
    mode: RigMode,
    dim: Color32,
) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    ui.label(RichText::new("Looks like").strong());
    ui.horizontal_wrapped(|ui| {
        for preset in &LIGHT_PRESETS {
            if ui
                .small_button(preset.name)
                .on_hover_text(preset.hint)
                .clicked()
            {
                (preset.apply)(light);
                edit.snapshot = true;
                edit.changed = true;
            }
        }
    });
    ui.add_space(4.0);
    let selected = has_selection_on(app, q0rg_id);
    egui::Grid::new(("light_settings", mode == RigMode::Pro))
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            edit.text(ui, &mut light.name, 160.0);
            ui.end_row();
            ui.label("Colour");
            edit.color(ui, &mut light.color);
            ui.end_row();
            ui.label("Brightness");
            edit.slider(ui, &mut light.intensity, 0.0..=4.0, false);
            ui.end_row();
            ui.label("Size");
            edit.slider(ui, &mut light.radius, 10.0..=2000.0, true);
            ui.end_row();
            ui.label("Softness");
            edit.slider(ui, &mut light.softness, 0.0..=1.0, false);
            ui.end_row();
            ui.label("Glow");
            edit.slider(ui, &mut light.glow, 0.0..=2.0, false);
            ui.end_row();
            ui.label("Flicker");
            edit.slider(ui, &mut light.flicker, 0.0..=1.0, false);
            ui.end_row();
            if mode == RigMode::Pro {
                ui.label("Flicker speed");
                edit.drag(ui, &mut light.flicker_speed, 0.1, 0.0..=60.0, " /s");
                ui.end_row();
                ui.label("Kind");
                ui.horizontal(|ui| {
                    edit.choice(ui, &mut light.kind, LightKind::Point, "All around");
                    edit.choice(ui, &mut light.kind, LightKind::Spot, "Spot");
                });
                ui.end_row();
                if light.kind == LightKind::Spot {
                    ui.label("Aims at");
                    edit.drag(ui, &mut light.direction, 1.0, -36_000.0..=36_000.0, "°");
                    ui.end_row();
                    ui.label("Cone");
                    ui.horizontal(|ui| {
                        edit.drag(ui, &mut light.cone, 0.5, 1.0..=360.0, "° wide");
                        edit.drag(ui, &mut light.cone_softness, 0.01, 0.0..=1.0, " soft");
                    });
                    ui.end_row();
                }
                ui.label("Position");
                ui.horizontal(|ui| {
                    edit.drag(
                        ui,
                        &mut light.position[0],
                        1.0,
                        -100_000.0..=100_000.0,
                        " x",
                    );
                    edit.drag(
                        ui,
                        &mut light.position[1],
                        1.0,
                        -100_000.0..=100_000.0,
                        " y",
                    );
                });
                ui.end_row();
            }
            ui.label("Follows");
            ui.horizontal(|ui| match light.follow {
                Some(instance_id) => {
                    let name = app
                        .state
                        .project
                        .runtime
                        .instance_names
                        .get(&q0s_format::v2::InstanceKey::new(q0rg_id, instance_id))
                        .cloned()
                        .unwrap_or_else(|| format!("object {instance_id}"));
                    ui.label(name);
                    if ui.small_button("Let go").clicked() {
                        if let Some(centre) =
                            light_centre(&app.state.project, q0rg_id, light, frame)
                        {
                            light.position = centre;
                        }
                        light.follow = None;
                        edit.snapshot = true;
                        edit.changed = true;
                    }
                }
                None => {
                    if ui
                        .add_enabled(selected, egui::Button::new("Selected object").small())
                        .on_hover_text(
                            "Ride along with the selected object, like a torch in a hand",
                        )
                        .on_disabled_hover_text("Select an object on this timeline first")
                        .clicked()
                    {
                        edit.follow_selection = true;
                    }
                    ui.label(RichText::new("nothing").small().color(dim));
                }
            });
            ui.end_row();
        });
    ui.label(
        RichText::new("drag the light on the stage to move it, its ring to resize it")
            .small()
            .color(dim),
    );
}

fn render_keys(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    light: &Light,
    frame: u16,
    dim: Color32,
) {
    ui.add_space(6.0);
    ui.label(RichText::new("Animation").strong());
    let keyed_here = light.key_at(frame).is_some();
    ui.horizontal_wrapped(|ui| {
        let mut changed = None;
        if ui
            .add_enabled(!keyed_here, egui::Button::new("Key this frame"))
            .on_hover_text("Remember position, colour, brightness, size and aim here")
            .clicked()
        {
            let mut keyed = on_frame(light, frame);
            keyed.set_key(frame);
            changed = Some(keyed);
        }
        if ui
            .add_enabled(keyed_here, egui::Button::new("Remove key"))
            .clicked()
        {
            let mut keyed = light.clone();
            keyed.keys.retain(|key| key.frame != frame);
            changed = Some(keyed);
        }
        if ui
            .add_enabled(!light.keys.is_empty(), egui::Button::new("Clear keys"))
            .on_hover_text("Hold this frame's look on every frame")
            .clicked()
        {
            let mut keyed = on_frame(light, frame);
            keyed.keys.clear();
            changed = Some(keyed);
        }
        if let Some(changed) = changed {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            if let Some(stored) = app
                .state
                .project
                .runtime
                .lighting
                .setup_mut(q0rg_id)
                .light_mut(changed.light_id)
            {
                *stored = changed;
            }
            app.state.mark_dirty();
        }
    });
    let frames = light
        .keys
        .iter()
        .map(|key| (key.frame + 1).to_string())
        .collect::<Vec<_>>();
    ui.label(
        RichText::new(if frames.is_empty() {
            "No keys: the light holds still. Key two frames to move or fade it between them; \
             with keys, edits here and on the stage key the frame you are on."
                .to_string()
        } else {
            format!("Keys on frames {}", frames.join(", "))
        })
        .small()
        .color(dim),
    );
}

// ------------------------------------------------------------- stage gizmos

/// Handles of `light` on the stage: its middle, its ring and, for a spot,
/// the arrow it aims with.
fn grips(light: &Light, centre: [f32; 2]) -> Vec<(Grip, Vec2)> {
    let shown = light;
    let mut grips = vec![
        (Grip::Move, Vec2::new(centre[0], centre[1])),
        (Grip::Radius, Vec2::new(centre[0] + shown.radius, centre[1])),
    ];
    if shown.kind == LightKind::Spot {
        let (sin, cos) = shown.direction.to_radians().sin_cos();
        let reach = shown.radius * 0.5;
        grips.push((
            Grip::Aim,
            Vec2::new(centre[0] + cos * reach, centre[1] + sin * reach),
        ));
    }
    grips
}

/// Lamps and their reach on the stage while the window is open.
pub fn paint_stage_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if !app.settings.light_enabled || !app.session.lights.open {
        return;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(setup) = app.state.project.runtime.lighting.setup(q0rg_id) else {
        return;
    };
    let accent = app.settings.theme.accent.to_color32();
    for light in &setup.lights {
        let Some(centre) = light_centre(&app.state.project, q0rg_id, light, frame) else {
            continue;
        };
        let shown = on_frame(light, frame);
        let selected = app.session.lights.selected == Some(light.light_id);
        let lamp = Color32::from_rgb(shown.color.r, shown.color.g, shown.color.b);
        let color = if selected {
            accent
        } else {
            accent.gamma_multiply(0.45)
        };
        let at = view.stage_to_screen(Vec2::new(centre[0], centre[1]));
        let scale = view.effective_scale().abs();
        // A little sun in the light's own colour.
        painter.circle(
            at,
            6.0,
            if light.enabled {
                lamp
            } else {
                Color32::from_gray(60)
            },
            Stroke::new(1.5_f32, color),
        );
        for ray in 0..8 {
            let angle = ray as f32 * std::f32::consts::TAU / 8.0;
            let (sin, cos) = angle.sin_cos();
            painter.line_segment(
                [
                    at + egui::vec2(cos, sin) * 9.0,
                    at + egui::vec2(cos, sin) * 13.0,
                ],
                Stroke::new(1.5_f32, color),
            );
        }
        if selected {
            painter.extend(egui::Shape::dashed_line(
                &(0..=48)
                    .map(|step| {
                        let angle = step as f32 * std::f32::consts::TAU / 48.0;
                        at + egui::vec2(angle.cos(), angle.sin()) * shown.radius * scale
                    })
                    .collect::<Vec<_>>(),
                Stroke::new(1.0_f32, color.gamma_multiply(0.8)),
                6.0,
                5.0,
            ));
            if shown.kind == LightKind::Spot {
                for side in [-0.5, 0.5] {
                    let angle = (shown.direction + shown.cone * side).to_radians();
                    painter.line_segment(
                        [
                            at,
                            at + egui::vec2(angle.cos(), angle.sin()) * shown.radius * scale,
                        ],
                        Stroke::new(1.0_f32, color.gamma_multiply(0.8)),
                    );
                }
            }
            for (grip, point) in grips(&shown, centre) {
                let point = view.stage_to_screen(point);
                match grip {
                    Grip::Move => {}
                    Grip::Radius => {
                        painter.rect(
                            egui::Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                            0.0,
                            color,
                            Stroke::new(1.0_f32, Color32::from_black_alpha(180)),
                        );
                    }
                    Grip::Aim => {
                        painter.arrow(at, point - at, Stroke::new(2.0_f32, color));
                        painter.circle_filled(point, 4.0, color);
                    }
                }
            }
            if light.follow.is_some() {
                let [x, y] = shown.position;
                let object = view.stage_to_screen(Vec2::new(centre[0] - x, centre[1] - y));
                painter.line_segment(
                    [object, at],
                    Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
                );
            }
        }
        painter.text(
            at + egui::vec2(12.0, -14.0),
            egui::Align2::LEFT_BOTTOM,
            &light.name,
            egui::FontId::proportional(11.0),
            color,
        );
    }
}

/// Drags the selected light on the stage. Returns whether the pointer is
/// busy with it, so the tools leave it alone.
pub fn handle_stage(
    app: &mut EditorApp,
    response: &egui::Response,
    cursor: Option<Vec2>,
    view: &StageView,
) -> bool {
    if !app.settings.light_enabled || !app.session.lights.open {
        app.session.lights.drag = None;
        return false;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(light) = app.session.lights.selected.and_then(|id| {
        app.state
            .project
            .runtime
            .lighting
            .setup(q0rg_id)?
            .light(id)
            .cloned()
    }) else {
        app.session.lights.drag = None;
        return false;
    };
    let Some(centre) = light_centre(&app.state.project, q0rg_id, &light, frame) else {
        return false;
    };
    let shown = on_frame(&light, frame);
    let near = |screen: egui::Pos2| {
        grips(&shown, centre)
            .into_iter()
            .map(|(grip, point)| (grip, view.stage_to_screen(point).distance(screen)))
            .filter(|(_, distance)| *distance <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(grip, _)| grip)
    };
    if response.drag_started_by(egui::PointerButton::Primary) {
        let press = response
            .ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some((grip, press)) = press.and_then(|press| near(press).map(|grip| (grip, press))) {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.session.lights.drag = Some(LightDrag {
                light_id: light.light_id,
                grip,
                press: view.screen_to_stage(press),
                position: shown.position,
                centre,
            });
            return true;
        }
        return false;
    }
    let Some(drag) = app.session.lights.drag else {
        if let Some(grip) = response.hover_pos().and_then(near) {
            response.ctx.set_cursor_icon(match grip {
                Grip::Move => egui::CursorIcon::Move,
                Grip::Radius => egui::CursorIcon::ResizeHorizontal,
                Grip::Aim => egui::CursorIcon::Alias,
            });
        }
        return false;
    };
    if response.drag_stopped() || !response.dragged() {
        app.session.lights.drag = None;
        return true;
    }
    let Some(cursor) = cursor else {
        return true;
    };
    let snap = response.ctx.input(|input| input.modifiers.shift);
    let mut changed = shown.clone();
    match drag.grip {
        Grip::Move => {
            changed.position = [
                drag.position[0] + cursor.x - drag.press.x,
                drag.position[1] + cursor.y - drag.press.y,
            ];
            if snap {
                changed.position = changed.position.map(f32::round);
            }
        }
        Grip::Radius => {
            let distance = (cursor.x - drag.centre[0]).hypot(cursor.y - drag.centre[1]);
            changed.radius = distance.max(1.0);
            if snap {
                changed.radius = (changed.radius / 10.0).round() * 10.0;
            }
        }
        Grip::Aim => {
            let mut degrees = (cursor.y - drag.centre[1])
                .atan2(cursor.x - drag.centre[0])
                .to_degrees();
            if snap {
                degrees = (degrees / 15.0).round() * 15.0;
            }
            changed.direction = degrees;
        }
    }
    if changed != shown {
        if !changed.keys.is_empty() {
            changed.set_key(frame);
        }
        if let Some(stored) = app
            .state
            .project
            .runtime
            .lighting
            .setup_mut(q0rg_id)
            .light_mut(drag.light_id)
        {
            *stored = changed;
        }
        app.state.project.runtime.lighting.sanitize();
        app.state.mark_dirty();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Layer, ProjectMeta, Q0rg};

    fn project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "lights".into(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                background_color: rgba(255, 255, 255),
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 24,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "art".into(),
                    explicit_keyframes: vec![0],
                    placements: Vec::new(),
                }],
            }],
        }
    }

    #[test]
    fn a_first_light_gets_a_light_layer_on_top() {
        let mut project = project();
        ensure_light_layer(&mut project, 1);
        let setup = project.runtime.lighting.setup(1).unwrap();
        let layer_id = setup.layer_id.unwrap();
        let layers = &project.q0rgs[0].layers;
        assert_eq!(layers.last().unwrap().layer_id, layer_id);
        assert_eq!(layers.last().unwrap().name, LIGHT_LAYER);
        // A second time reuses it.
        ensure_light_layer(&mut project, 1);
        assert_eq!(project.q0rgs[0].layers.len(), 2);
    }

    #[test]
    fn a_light_follows_a_plain_selected_object_as_it_moves() {
        use q0s_format::v2::{
            Asset, Placement, PlacementFx, Target, Transform2D, Tween, VectorAsset,
        };
        let mut app = EditorApp::default();
        let mut project = project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![crate::sim_bake::polygon_path(&[
                [-10.0, -10.0],
                [10.0, -10.0],
                [10.0, 10.0],
                [-10.0, 10.0],
            ])],
            fill: Some(rgba(0, 0, 0)),
            stroke: None,
        }));
        let placed = |frame: u16, x: f32, tween: Tween| Placement {
            pivot: None,
            // No instance id yet, like most drawn objects.
            instance_id: 0,
            frame,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: x,
                ty: 100.0,
                ..Transform2D::IDENTITY
            },
            tween,
            fx: PlacementFx::default(),
        };
        let layer = &mut project.q0rgs[0].layers[0];
        layer
            .placements
            .push(placed(0, 50.0, Tween::Linear { to_frame: 10 }));
        layer.ensure_explicit_keyframe(10);
        layer.push_placement(placed(10, 250.0, Tween::None));
        app.state.project = project;
        app.session.current_q0rg_id = 1;
        app.session.current_frame = 0;
        app.session.selection = crate::state::Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let instance_id = follow_target(&mut app, 1).expect("the object gets an id");
        assert_ne!(instance_id, 0);
        let setup = app.state.project.runtime.lighting.setup_mut(1);
        let mut light = Light::new(1, [0.0, -20.0]);
        light.follow = Some(instance_id);
        setup.lights.push(light);
        let at = |frame: u16| {
            q0s_format::raster::frame_lighting(
                &app.state.project,
                1,
                frame,
                &RuntimeSceneState::new(),
            )
            .unwrap()
            .lights[0]
                .position
        };
        assert_eq!(at(0), [50.0, 80.0]);
        assert_eq!(at(10), [250.0, 80.0]);
    }

    #[test]
    fn every_light_preset_shines() {
        for preset in &LIGHT_PRESETS {
            let mut light = Light::new(1, [100.0, 100.0]);
            (preset.apply)(&mut light);
            assert!(
                light.radius > 0.0 && light.intensity > 0.0,
                "{}",
                preset.name
            );
            let mut setup = LightSetup::new(1);
            setup.lights.push(light);
            let lit = setup.frame(0, 24, |_| None).unwrap();
            assert_eq!(lit.lights.len(), 1, "{}", preset.name);
        }
    }

    #[test]
    fn showing_a_keyed_light_plays_its_keys() {
        let mut light = Light::new(1, [0.0, 0.0]);
        light.set_key(0);
        light.position = [80.0, 0.0];
        light.set_key(8);
        let shown = on_frame(&light, 4);
        assert!((shown.position[0] - 40.0).abs() < 0.01);
    }
}
