//! The particle editor: emitters that spray fire, smoke, sparks, galaxies or
//! copies of a library item, baked into ordinary keyframes on a layer of
//! their own. The simulation lives in `q0physics`, shared with q0player,
//! which can also run emitters live for q0lang.
//!
//! Simple mode starts from a preset and shows the handful of settings that
//! matter most; Pro mode shows every setting the way Geometry Dash's
//! particle editor does, each with its ± variance.
//!
//! An opt-in tool like physics: while `Settings::particles_enabled` is off
//! nothing of it shows.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Context, Painter, RichText, Stroke, Ui};
use q0physics::particles::{follow_point, ParticleDraw, ParticleSystem};
use q0s_format::particles::{
    EmitterShape, ParticleEmitter, ParticleLook, ParticleMotion, ParticleSpace, Varied,
};
use q0s_format::raster::RuntimeBoundsCache;
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, BlendMode, GradientFill, GradientKind, GradientStop, LibraryItemReference, Path,
    Placement, PlacementFx, ProjectV2, Rgba, Target, TintFx, Transform2D, Tween, Vec2,
    VectorAppearance, VectorMaterial,
};

use crate::app::EditorApp;
use crate::physics::{apply_edit, follow_target, render_range, selection_area, Edit};
use crate::render::StageView;
use crate::sim_bake;
use crate::state::RigMode;
use crate::ui::FitToScreen;

/// Longest stretch one bake simulates.
const MAX_SIMULATED_FRAMES: u32 = 20_000;
/// The built-in shapes are drawn this many pixels across and scaled.
const UNIT: f32 = 10.0;

/// The pages of Pro mode, after Geometry Dash's particle editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProPage {
    #[default]
    Emitter,
    Motion,
    Looks,
}

#[derive(Default)]
pub struct ParticleSession {
    pub open: bool,
    pub mode: RigMode,
    pub page: ProPage,
    pub selected: Option<u16>,
    pub previewing: bool,
    /// Bake only the selected emitter.
    pub only_selected: bool,
    pub filter: String,
    pub(crate) message: String,
    /// The selected emitter being dragged on the stage.
    drag: Option<EmitterDrag>,
}

#[derive(Debug, Clone, Copy)]
struct EmitterDrag {
    emitter_id: u16,
    press: Vec2,
    position: [f32; 2],
}

impl ParticleSession {
    pub fn stop_preview(&mut self) {
        self.previewing = false;
    }
}

#[derive(Debug, Clone)]
pub struct EmitterTrack {
    pub emitter_id: u16,
    pub start_frame: u16,
    /// The particles of every frame from the start frame on, back to front.
    pub frames: Vec<Vec<ParticleDraw>>,
}

#[derive(Debug, Clone)]
pub struct Simulation {
    pub q0rg_id: u16,
    pub emitters: Vec<EmitterTrack>,
}

pub fn simulate(
    project: &ProjectV2,
    q0rg_id: u16,
    only: Option<u16>,
) -> Result<Simulation, String> {
    simulate_with(project, q0rg_id, only, None)
}

/// Runs the emitters of `q0rg_id` (or just `only`) over their frames.
pub fn simulate_with(
    project: &ProjectV2,
    q0rg_id: u16,
    only: Option<u16>,
    cancel: Option<&AtomicBool>,
) -> Result<Simulation, String> {
    let fps = f32::from(project.meta.fps.max(1));
    let stage = [
        f32::from(project.meta.stage_width.max(1)),
        f32::from(project.meta.stage_height.max(1)),
    ];
    let scene = RuntimeSceneState::new();
    let mut bounds = None;
    let mut emitters = Vec::new();
    for emitter in project.runtime.particles.emitters.iter().filter(|emitter| {
        emitter.q0rg_id == q0rg_id && only.is_none_or(|only| only == emitter.emitter_id)
    }) {
        let frames = u32::from(emitter.end_frame.saturating_sub(emitter.start_frame)) + 1;
        if frames > MAX_SIMULATED_FRAMES {
            return Err(format!(
                "\"{}\" runs over more than {MAX_SIMULATED_FRAMES} frames",
                emitter.name
            ));
        }
        let mut anchor_at = |frame: u16| {
            let instance_id = emitter.follow?;
            let cache = bounds.get_or_insert_with(|| RuntimeBoundsCache::new(project));
            follow_point(project, q0rg_id, frame, instance_id, &scene, Some(cache))
        };
        let mut system = ParticleSystem::new_at(emitter, stage, anchor_at(emitter.start_frame));
        let mut track = EmitterTrack {
            emitter_id: emitter.emitter_id,
            start_frame: emitter.start_frame,
            frames: Vec::with_capacity(frames as usize),
        };
        for offset in 0..frames {
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                return Err("cancelled".to_string());
            }
            if offset > 0 {
                let frame = emitter.start_frame.saturating_add(offset as u16);
                if let Some(anchor) = anchor_at(frame) {
                    system.set_anchor(anchor);
                }
                system.step(1.0 / fps);
            }
            track.frames.push(system.draws());
        }
        emitters.push(track);
    }
    if emitters.is_empty() {
        return Err("add an emitter first".to_string());
    }
    Ok(Simulation { q0rg_id, emitters })
}

/// Keys every simulated frame of every emitter onto its layer. Returns how
/// many frames were keyed.
pub fn bake(project: &mut ProjectV2, simulation: &Simulation) -> Result<usize, String> {
    let q0rg_id = simulation.q0rg_id;
    let mut ids = sim_bake::AssetIds::new(project);
    let mut keyed = 0;
    for track in &simulation.emitters {
        let Some(emitter) = project
            .runtime
            .particles
            .emitter(q0rg_id, track.emitter_id)
            .cloned()
        else {
            continue;
        };
        let layer_id = sim_bake::effect_layer(project, q0rg_id, emitter.layer_id, &emitter.name)?;
        if let Some(stored) = project
            .runtime
            .particles
            .emitter_mut(q0rg_id, emitter.emitter_id)
        {
            stored.layer_id = Some(layer_id);
        }
        let start = track.start_frame;
        let last = start + (track.frames.len().max(1) - 1) as u16;
        sim_bake::clear_range(
            project,
            q0rg_id,
            layer_id,
            start,
            last,
            sim_bake::PARTICLES_FOLDER,
        );
        let (target, built_in) = match emitter.look {
            ParticleLook::Item(target) => (target, false),
            look => (Target::Asset(shape_asset(project, &mut ids, look)?), true),
        };
        for (offset, draws) in track.frames.iter().enumerate() {
            let frame = start + offset as u16;
            let placements = draws
                .iter()
                .filter(|draw| draw.style.color.a > 0 && draw.style.size > 0.0)
                .map(|draw| placement(frame, target, built_in, draw))
                .collect();
            sim_bake::key_frame(project, q0rg_id, layer_id, frame, placements);
            keyed += 1;
        }
        sim_bake::reach_frame(project, q0rg_id, layer_id, last);
    }
    Ok(keyed)
}

fn placement(frame: u16, target: Target, built_in: bool, draw: &ParticleDraw) -> Placement {
    let style = draw.style;
    let scale = if built_in {
        style.size / UNIT
    } else {
        style.size
    };
    let opacity = f32::from(style.color.a) / 255.0;
    Placement {
        pivot: None,
        instance_id: 0,
        frame,
        target,
        transform: Transform2D {
            tx: draw.position[0],
            ty: draw.position[1],
            sx: (scale * style.stretch).max(0.001),
            sy: scale.max(0.001),
            rotation: style.rotation,
            ..Transform2D::IDENTITY
        },
        tween: Tween::None,
        fx: PlacementFx {
            opacity,
            // Built-in shapes are white and take the particle's colour.
            tint: built_in.then_some(TintFx {
                color: Rgba {
                    a: 255,
                    ..style.color
                },
                amount: 1.0,
            }),
            blend_mode: if style.additive {
                BlendMode::Add
            } else {
                BlendMode::Normal
            },
            ..PlacementFx::default()
        },
    }
}

/// The white drawing a built-in look uses, made once and shared by every
/// emitter.
fn shape_asset(
    project: &mut ProjectV2,
    ids: &mut sim_bake::AssetIds,
    look: ParticleLook,
) -> Result<u16, String> {
    let half = UNIT * 0.5;
    let (name, paths) = match look {
        ParticleLook::Circle => ("particle circle", vec![circle_path(half, false)]),
        ParticleLook::Glow => ("particle glow", vec![circle_path(half, false)]),
        ParticleLook::Ring => (
            "particle ring",
            vec![circle_path(half, false), circle_path(half * 0.72, true)],
        ),
        ParticleLook::Square => (
            "particle square",
            vec![sim_bake::polygon_path(&[
                [-half, -half],
                [half, -half],
                [half, half],
                [-half, half],
            ])],
        ),
        ParticleLook::Star => {
            let points = (0..10)
                .map(|index| {
                    let radius = if index % 2 == 0 { half } else { UNIT * 0.2 };
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                    [angle.cos() * radius, angle.sin() * radius]
                })
                .collect::<Vec<_>>();
            ("particle star", vec![sim_bake::polygon_path(&points)])
        }
        ParticleLook::Spark => (
            "particle spark",
            vec![sim_bake::polygon_path(&[
                [half, 0.0],
                [0.0, UNIT * 0.15],
                [-half, 0.0],
                [0.0, -UNIT * 0.15],
            ])],
        ),
        ParticleLook::Item(_) => unreachable!("items are library artwork"),
    };
    let existing = project.asset_names.iter().find_map(|(id, asset_name)| {
        (asset_name == name
            && project
                .runtime
                .library_item_folders
                .get(&LibraryItemReference::Asset(*id))
                .is_some_and(|folder| folder == sim_bake::PARTICLES_FOLDER))
        .then_some(*id)
    });
    if let Some(id) = existing.filter(|id| project.assets.iter().any(|asset| asset.id() == *id)) {
        return Ok(id);
    }
    let white = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    let id = sim_bake::add_drawing(
        project,
        ids,
        paths,
        white,
        name.to_string(),
        sim_bake::PARTICLES_FOLDER,
    )?;
    if look == ParticleLook::Glow {
        project.asset_appearances.insert(id, glow_appearance(half));
    }
    Ok(id)
}

/// White in the middle, fading smoothly to nothing at `radius`.
fn glow_appearance(radius: f32) -> VectorAppearance {
    let stop = |position: f32, alpha: u8| GradientStop {
        position,
        color: Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: alpha,
        },
    };
    VectorAppearance {
        material: VectorMaterial::Solid,
        erase_mask: Vec::new(),
        material_source: Vec::new(),
        clip_mask: Vec::new(),
        field_transform: Affine::IDENTITY,
        fill_gradient: Some(GradientFill {
            kind: GradientKind::Radial,
            stops: vec![
                stop(0.0, 255),
                stop(0.2, 230),
                stop(0.45, 140),
                stop(0.7, 50),
                stop(1.0, 0),
            ],
            transform: Affine::from_uniform_scale(radius),
        }),
    }
}

fn circle_path(radius: f32, reversed: bool) -> Path {
    // Four cubic arcs, the usual circle approximation. A reversed circle
    // cuts a hole into the one around it.
    let k = radius * 0.552_284_8;
    let anchor = |x: f32, y: f32, ix: f32, iy: f32, ox: f32, oy: f32| Anchor {
        point: Vec2::new(x, y),
        in_handle: Some(Vec2::new(ix, iy)),
        out_handle: Some(Vec2::new(ox, oy)),
    };
    let mut anchors = vec![
        anchor(radius, 0.0, radius, -k, radius, k),
        anchor(0.0, radius, k, radius, -k, radius),
        anchor(-radius, 0.0, -radius, k, -radius, -k),
        anchor(0.0, -radius, -k, -radius, k, -radius),
    ];
    if reversed {
        anchors.reverse();
        for anchor in &mut anchors {
            std::mem::swap(&mut anchor.in_handle, &mut anchor.out_handle);
        }
    }
    Path {
        anchors,
        closed: true,
    }
}

/// Removes what earlier bakes keyed on the emitters' layers over their frames.
pub fn clear_baked(project: &mut ProjectV2, q0rg_id: u16, only: Option<u16>) -> usize {
    let emitters = project
        .runtime
        .particles
        .emitters
        .iter()
        .filter(|emitter| {
            emitter.q0rg_id == q0rg_id && only.is_none_or(|only| only == emitter.emitter_id)
        })
        .filter_map(|emitter| Some((emitter.layer_id?, emitter.start_frame, emitter.end_frame)))
        .collect::<Vec<_>>();
    let mut removed = 0;
    for (layer_id, start, end) in emitters {
        let before = layer_keys(project, q0rg_id, layer_id);
        sim_bake::clear_range(
            project,
            q0rg_id,
            layer_id,
            start,
            end,
            sim_bake::PARTICLES_FOLDER,
        );
        removed += before.saturating_sub(layer_keys(project, q0rg_id, layer_id));
    }
    removed
}

fn layer_keys(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> usize {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map_or(0, |layer| layer.keyframe_frames().len())
}

/// What the preview job should bake for particles right now, if anything.
pub fn preview_request(app: &EditorApp) -> Option<(u16, Option<u16>)> {
    let particles = &app.session.particles;
    let q0rg_id = app.session.current_q0rg_id;
    (app.settings.particles_enabled
        && particles.open
        && particles.previewing
        && app
            .state
            .project
            .runtime
            .particles
            .emitters
            .iter()
            .any(|emitter| emitter.q0rg_id == q0rg_id))
    .then(|| {
        (
            q0rg_id,
            particles.selected.filter(|_| particles.only_selected),
        )
    })
}

pub fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.settings.particles_enabled || !app.session.particles.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Particles")
        .open(&mut open)
        .default_size([400.0, 640.0])
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-850.0, 80.0))
        .max_height((ctx.screen_rect().height() - 110.0).max(320.0))
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| render_contents(app, ui));
    if !open {
        app.session.particles.open = false;
        app.session.particles.stop_preview();
    }
}

fn render_contents(app: &mut EditorApp, ui: &mut Ui) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    ui.horizontal(|ui| {
        ui.label("Mode");
        for mode in RigMode::ALL {
            ui.selectable_value(&mut app.session.particles.mode, mode, mode.label());
        }
    });
    let mode = app.session.particles.mode;
    ui.label(
        RichText::new(match mode {
            RigMode::Simple => "pick a preset, place it, tweak a few things, bake it into keys",
            RigMode::Pro => "every setting, each with its ± variance, like a GD particle editor",
        })
        .small()
        .color(dim),
    );
    ui.separator();
    render_emitter_list(app, ui, q0rg_id, dim);
    let Some(emitter) = app
        .session
        .particles
        .selected
        .and_then(|id| app.state.project.runtime.particles.emitter(q0rg_id, id))
        .cloned()
    else {
        return;
    };
    ui.separator();
    render_bake_controls(app, ui, q0rg_id, &emitter, dim);
    ui.separator();
    if mode == RigMode::Pro {
        ui.horizontal(|ui| {
            for (page, label) in [
                (ProPage::Emitter, "Emitter"),
                (ProPage::Motion, "Motion"),
                (ProPage::Looks, "Looks"),
            ] {
                ui.selectable_value(&mut app.session.particles.page, page, label);
            }
        });
        ui.add_space(2.0);
    }
    egui::ScrollArea::vertical()
        .id_source("particles_setup")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut emitter = emitter.clone();
            let mut edit = Edit::default();
            match mode {
                RigMode::Simple => render_emitter_simple(app, ui, &mut edit, &mut emitter, dim),
                RigMode::Pro => match app.session.particles.page {
                    ProPage::Emitter => render_pro_emitter(app, ui, &mut edit, &mut emitter, dim),
                    ProPage::Motion => render_pro_motion(ui, &mut edit, &mut emitter, dim),
                    ProPage::Looks => render_pro_looks(app, ui, &mut edit, &mut emitter, dim),
                },
            }
            if edit.follow_selection {
                if let Some(instance_id) = follow_target(app, emitter.q0rg_id) {
                    // The emitter keeps its place, now as an offset from the
                    // object's middle.
                    if let Some([ax, ay]) = follow_point(
                        &app.state.project,
                        emitter.q0rg_id,
                        app.session.current_frame,
                        instance_id,
                        &RuntimeSceneState::new(),
                        None,
                    ) {
                        emitter.position = [emitter.position[0] - ax, emitter.position[1] - ay];
                        if emitter.position[0].hypot(emitter.position[1]) > 400.0 {
                            emitter.position = [0.0, 0.0];
                        }
                    }
                    emitter.follow = Some(instance_id);
                    mark(&mut edit);
                }
            }
            if edit.changed || edit.snapshot {
                store_emitter(app, &edit, emitter);
            }
        });
}

fn store_emitter(app: &mut EditorApp, edit: &Edit, emitter: ParticleEmitter) {
    apply_edit(app, edit, |project| {
        if let Some(stored) = project
            .runtime
            .particles
            .emitter_mut(emitter.q0rg_id, emitter.emitter_id)
        {
            *stored = emitter;
            stored.sanitize();
        }
    });
}

fn render_emitter_list(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, dim: Color32) {
    ui.horizontal(|ui| {
        if ui
            .button("Add emitter")
            .on_hover_text("A new emitter at the selected object, or in the middle of the stage")
            .clicked()
        {
            let stage = [
                f32::from(app.state.project.meta.stage_width.max(1)),
                f32::from(app.state.project.meta.stage_height.max(1)),
            ];
            let position = selection_area(app)
                .map_or([stage[0] * 0.5, stage[1] * 0.6], |[x, y, w, h]| {
                    [x + w * 0.5, y + h * 0.5]
                });
            let start = app.session.current_frame;
            let fps = app.state.project.meta.fps.max(1);
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            let particles = &mut app.state.project.runtime.particles;
            let emitter_id = particles.next_emitter_id(q0rg_id);
            let mut emitter = ParticleEmitter::new(q0rg_id, emitter_id, position);
            emitter.start_frame = start;
            emitter.end_frame = start
                .saturating_add(fps.saturating_mul(2) - 1)
                .min(u16::MAX - 1);
            particles.emitters.push(emitter);
            app.session.particles.selected = Some(emitter_id);
            app.state.mark_dirty();
        }
        ui.add(
            egui::TextEdit::singleline(&mut app.session.particles.filter)
                .hint_text("Search emitters")
                .desired_width(ui.available_width()),
        );
    });
    let emitters = app
        .state
        .project
        .runtime
        .particles
        .emitters
        .iter()
        .filter(|emitter| emitter.q0rg_id == q0rg_id)
        .map(|emitter| {
            (
                emitter.emitter_id,
                emitter.name.clone(),
                emitter.color_start,
                emitter.live,
            )
        })
        .collect::<Vec<_>>();
    if emitters.is_empty() {
        ui.label(
            RichText::new("No emitters on this timeline yet. Add one, then pick a preset.")
                .small()
                .color(dim),
        );
        return;
    }
    if app
        .session
        .particles
        .selected
        .is_none_or(|selected| !emitters.iter().any(|(id, _, _, _)| *id == selected))
    {
        app.session.particles.selected = Some(emitters[0].0);
    }
    let filter = app.session.particles.filter.trim().to_lowercase();
    let mut remove = None;
    egui::ScrollArea::vertical()
        .id_source("particles_list")
        .max_height(130.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let mut shown = 0;
            for (emitter_id, name, color, live) in &emitters {
                if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                    continue;
                }
                shown += 1;
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        rect.center(),
                        5.0,
                        Color32::from_rgb(color.r, color.g, color.b),
                    );
                    let selected = app.session.particles.selected == Some(*emitter_id);
                    let label = if *live {
                        format!("{name} · live")
                    } else {
                        name.clone()
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        app.session.particles.selected = Some(*emitter_id);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(*emitter_id);
                        }
                    });
                });
            }
            if shown == 0 {
                ui.label(RichText::new("No emitter matches.").small().color(dim));
            }
        });
    if let Some(emitter_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        app.state
            .project
            .runtime
            .particles
            .emitters
            .retain(|emitter| !(emitter.q0rg_id == q0rg_id && emitter.emitter_id == emitter_id));
        app.state.mark_dirty();
    }
}

fn render_bake_controls(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    emitter: &ParticleEmitter,
    dim: Color32,
) {
    let (picked, edit) = render_range(
        app,
        ui,
        q0rg_id,
        (emitter.start_frame, emitter.end_frame),
        dim,
    );
    if let Some((start, end)) = picked {
        let mut changed = emitter.clone();
        changed.start_frame = start;
        changed.end_frame = end;
        store_emitter(app, &edit, changed);
    }
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        let previewing = app.session.particles.previewing;
        if ui
            .add(egui::Button::new("Preview").selected(previewing))
            .on_hover_text(
                "Watch the particles on the stage: play or scrub. Nothing is keyed until you bake.",
            )
            .clicked()
        {
            if previewing {
                app.session.particles.stop_preview();
            } else {
                app.session.particles.previewing = true;
            }
        }
        if ui
            .button(RichText::new("Bake keys").strong())
            .on_hover_text("Key every frame of the emitters onto their own layers")
            .clicked()
        {
            bake_into_project(app, q0rg_id);
        }
        if ui
            .button("Remove keys")
            .on_hover_text("Remove what was baked over the emitters' frames")
            .clicked()
        {
            let only = app
                .session
                .particles
                .selected
                .filter(|_| app.session.particles.only_selected);
            let before = app.state.project.clone();
            let removed = clear_baked(&mut app.state.project, q0rg_id, only);
            if removed > 0 {
                app.history.snapshot_with_session(&before, &app.session);
                app.state.mark_dirty();
                app.textures.invalidate();
                report(app, format!("Removed {removed} keyframes."));
            } else {
                report(app, "No baked particles on these frames.".to_string());
            }
        }
    });
    ui.checkbox(
        &mut app.session.particles.only_selected,
        "Only the selected emitter",
    );
    let note = crate::sim_preview::status_note(app, crate::sim_preview::Part::Particles);
    let message = &app.session.particles.message;
    let message = note.as_deref().unwrap_or(if message.is_empty() {
        "Each emitter bakes onto a layer of its own, one keyframe per frame."
    } else {
        message.as_str()
    });
    ui.label(RichText::new(message).small().color(dim));
}

fn report(app: &mut EditorApp, message: String) {
    app.session.status = message.clone();
    app.session.particles.message = message;
}

fn bake_into_project(app: &mut EditorApp, q0rg_id: u16) {
    let only = app
        .session
        .particles
        .selected
        .filter(|_| app.session.particles.only_selected);
    let before = app.state.project.clone();
    let result = simulate(&app.state.project, q0rg_id, only)
        .and_then(|simulation| bake(&mut app.state.project, &simulation));
    match result {
        Ok(keyed) => {
            app.history.snapshot_with_session(&before, &app.session);
            app.state.mark_dirty();
            app.textures.invalidate();
            app.session.particles.stop_preview();
            report(
                app,
                format!("Baked {keyed} frames of particles. Undo takes them back."),
            );
        }
        Err(error) => {
            app.state.project = before;
            report(app, error);
        }
    }
}

// ---------------------------------------------------------------- presets

struct Preset {
    name: &'static str,
    hint: &'static str,
    apply: fn(&mut ParticleEmitter, [f32; 2]),
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    Rgba { r, g, b, a }
}

const NO_VARIANCE: Rgba = Rgba {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

fn v(value: f32, variance: f32) -> Varied {
    Varied::new(value, variance)
}

/// Back to plain settings, keeping what makes the emitter this emitter:
/// its name, place, frames, what it follows and whether it runs live.
fn reset(e: &mut ParticleEmitter) {
    let kept = e.clone();
    *e = ParticleEmitter::new(kept.q0rg_id, kept.emitter_id, kept.position);
    e.name = kept.name;
    e.start_frame = kept.start_frame;
    e.end_frame = kept.end_frame;
    e.follow = kept.follow;
    e.live = kept.live;
    e.layer_id = kept.layer_id;
    e.seed = kept.seed;
    e.radial_accel = Varied::fixed(0.0);
    e.tangential_accel = Varied::fixed(0.0);
    e.color_start_variance = NO_VARIANCE;
    e.color_end_variance = NO_VARIANCE;
    e.fade_in = Varied::fixed(0.0);
    e.fade_out = Varied::fixed(0.0);
    e.drag = Varied::fixed(0.0);
}

/// Across the top of the stage, for weather.
fn sky(e: &mut ParticleEmitter, stage: [f32; 2]) {
    e.follow = None;
    e.shape = EmitterShape::Box;
    e.position = [stage[0] * 0.5, -12.0];
    e.size = [stage[0] * 1.15, 8.0];
    e.angle = 0.0;
}

const PRESETS: [Preset; 14] = [
    Preset {
        name: "Fire",
        hint: "warm flames licking upwards",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = true;
            e.shape = EmitterShape::Line;
            e.size = [40.0, 0.0];
            e.rate = 90.0;
            e.max_particles = 300;
            e.lifetime = v(0.9, 0.3);
            e.direction = v(270.0, 12.0);
            e.speed = v(80.0, 30.0);
            e.gravity = [0.0, -120.0];
            e.tangential_accel = v(0.0, 40.0);
            e.drag = v(0.6, 0.0);
            e.size_start = v(34.0, 8.0);
            e.size_end = v(10.0, 4.0);
            e.color_start = rgba(255, 170, 50, 255);
            e.color_start_variance = rgba(0, 40, 20, 0);
            e.color_end = rgba(200, 40, 20, 0);
            e.fade_in = v(0.08, 0.0);
            e.fade_out = v(0.25, 0.0);
        },
    },
    Preset {
        name: "Smoke",
        hint: "slow grey puffs that spread and drift",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = false;
            e.shape = EmitterShape::Circle;
            e.size = [10.0, 0.0];
            e.rate = 18.0;
            e.lifetime = v(2.8, 0.8);
            e.direction = v(270.0, 20.0);
            e.speed = v(45.0, 15.0);
            e.gravity = [8.0, -25.0];
            e.drag = v(0.4, 0.1);
            e.size_start = v(24.0, 6.0);
            e.size_end = v(90.0, 20.0);
            e.color_start = rgba(110, 110, 115, 150);
            e.color_start_variance = rgba(15, 15, 15, 30);
            e.color_sync = true;
            e.color_end = rgba(70, 70, 75, 0);
            e.fade_in = v(0.4, 0.1);
            e.fade_out = v(1.0, 0.2);
        },
    },
    Preset {
        name: "Sparks",
        hint: "hot streaks that fall and bounce off the floor",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Spark;
            e.additive = true;
            e.shape = EmitterShape::Point;
            e.rate = 70.0;
            e.lifetime = v(0.6, 0.3);
            e.direction = v(270.0, 55.0);
            e.speed = v(380.0, 140.0);
            e.gravity = [0.0, 700.0];
            e.drag = v(0.8, 0.0);
            e.size_start = v(8.0, 2.0);
            e.size_end = v(3.0, 1.0);
            e.stretch = 1.6;
            e.face_motion = true;
            e.color_start = rgba(255, 250, 210, 255);
            e.color_end = rgba(255, 120, 20, 0);
            e.color_end_variance = rgba(0, 40, 0, 0);
            e.edge_bounce = 0.4;
            e.fade_out = v(0.1, 0.0);
        },
    },
    Preset {
        name: "Explosion",
        hint: "one big burst of fire in every direction",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = true;
            e.shape = EmitterShape::Point;
            e.rate = 0.0;
            e.burst = 140;
            e.lifetime = v(0.9, 0.4);
            e.direction = v(0.0, 180.0);
            e.speed = v(260.0, 200.0);
            e.gravity = [0.0, 80.0];
            e.drag = v(3.0, 0.5);
            e.size_start = v(36.0, 14.0);
            e.size_end = v(6.0, 4.0);
            e.color_start = rgba(255, 245, 190, 255);
            e.color_end = rgba(255, 70, 20, 0);
            e.color_end_variance = rgba(0, 40, 10, 0);
            e.fade_out = v(0.3, 0.1);
        },
    },
    Preset {
        name: "Magic",
        hint: "twinkling stars that swirl and fade",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Star;
            e.additive = true;
            e.shape = EmitterShape::Circle;
            e.size = [14.0, 0.0];
            e.rate = 50.0;
            e.lifetime = v(1.4, 0.5);
            e.direction = v(0.0, 180.0);
            e.speed = v(50.0, 30.0);
            e.gravity = [0.0, -30.0];
            e.radial_accel = v(-20.0, 0.0);
            e.tangential_accel = v(160.0, 60.0);
            e.size_start = v(14.0, 5.0);
            e.size_end = v(0.0, 0.0);
            e.spin_start = v(0.0, 180.0);
            e.spin_end = v(360.0, 180.0);
            e.color_start = rgba(170, 120, 255, 255);
            e.color_start_variance = rgba(60, 60, 0, 0);
            e.color_end = rgba(80, 220, 255, 0);
            e.fade_in = v(0.1, 0.0);
        },
    },
    Preset {
        name: "Galaxy",
        hint: "glowing dots spiralling into the middle",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = true;
            e.shape = EmitterShape::Point;
            e.motion = ParticleMotion::Radius;
            e.rate = 140.0;
            e.max_particles = 600;
            e.quick_start = true;
            e.lifetime = v(2.2, 0.6);
            e.direction = v(0.0, 180.0);
            e.start_radius = v(150.0, 30.0);
            e.end_radius = v(0.0, 0.0);
            e.orbit = v(120.0, 40.0);
            e.size_start = v(18.0, 6.0);
            e.size_end = v(4.0, 2.0);
            e.color_start = rgba(120, 170, 255, 255);
            e.color_start_variance = rgba(30, 30, 0, 0);
            e.color_end = rgba(255, 90, 200, 0);
            e.fade_in = v(0.3, 0.0);
        },
    },
    Preset {
        name: "Portal",
        hint: "a spinning ring of light",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = true;
            e.shape = EmitterShape::Point;
            e.motion = ParticleMotion::Radius;
            e.rate = 120.0;
            e.quick_start = true;
            e.lifetime = v(1.2, 0.3);
            e.direction = v(0.0, 180.0);
            e.start_radius = v(70.0, 4.0);
            e.end_radius = v(70.0, 4.0);
            e.orbit = v(260.0, 40.0);
            e.size_start = v(14.0, 4.0);
            e.size_end = v(2.0, 1.0);
            e.color_start = rgba(90, 255, 200, 255);
            e.color_end = rgba(40, 120, 255, 0);
            e.fade_in = v(0.15, 0.0);
        },
    },
    Preset {
        name: "Snow",
        hint: "soft flakes drifting down over the whole stage",
        apply: |e, stage| {
            reset(e);
            sky(e, stage);
            e.look = ParticleLook::Glow;
            e.additive = false;
            e.rate = 25.0;
            e.max_particles = 600;
            e.quick_start = true;
            e.lifetime = v((stage[1] / 40.0).clamp(3.0, 20.0), 2.0);
            e.direction = v(90.0, 20.0);
            e.speed = v(40.0, 15.0);
            e.gravity = [0.0, 8.0];
            e.drag = v(0.1, 0.0);
            e.tangential_accel = v(0.0, 15.0);
            e.size_start = v(9.0, 4.0);
            e.size_end_same = true;
            e.color_start = rgba(255, 255, 255, 230);
            e.color_start_variance = rgba(0, 0, 0, 25);
            e.color_end = rgba(255, 255, 255, 230);
            e.fade_in = v(0.5, 0.0);
            e.fade_out = v(1.0, 0.0);
        },
    },
    Preset {
        name: "Rain",
        hint: "fast streaks falling across the whole stage",
        apply: |e, stage| {
            reset(e);
            sky(e, stage);
            e.look = ParticleLook::Spark;
            e.additive = false;
            e.rate = 160.0;
            e.max_particles = 800;
            e.quick_start = true;
            e.lifetime = v((stage[1] / 650.0).clamp(0.5, 4.0), 0.2);
            e.direction = v(100.0, 3.0);
            e.speed = v(750.0, 80.0);
            e.gravity = [0.0, 400.0];
            e.size_start = v(6.0, 1.0);
            e.size_end_same = true;
            e.stretch = 3.0;
            e.face_motion = true;
            e.color_start = rgba(170, 200, 255, 180);
            e.color_end = rgba(170, 200, 255, 180);
        },
    },
    Preset {
        name: "Confetti",
        hint: "a party burst of tumbling colourful squares",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Square;
            e.additive = false;
            e.shape = EmitterShape::Point;
            e.burst = 120;
            e.rate = 30.0;
            e.emit_seconds = 0.5;
            e.lifetime = v(3.5, 1.0);
            e.direction = v(270.0, 40.0);
            e.speed = v(520.0, 180.0);
            e.gravity = [0.0, 520.0];
            e.drag = v(1.4, 0.4);
            e.size_start = v(11.0, 3.0);
            e.size_end_same = true;
            e.spin_start = v(0.0, 180.0);
            e.spin_end = v(0.0, 1080.0);
            e.color_start = rgba(255, 120, 180, 255);
            e.color_start_variance = rgba(120, 120, 120, 0);
            e.color_end = rgba(255, 120, 180, 255);
            e.color_end_variance = rgba(120, 120, 120, 0);
            e.fade_out = v(0.6, 0.0);
        },
    },
    Preset {
        name: "Bubbles",
        hint: "rings that wobble upwards and grow",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Ring;
            e.additive = false;
            e.shape = EmitterShape::Line;
            e.size = [60.0, 0.0];
            e.rate = 10.0;
            e.lifetime = v(3.5, 1.0);
            e.direction = v(270.0, 15.0);
            e.speed = v(70.0, 20.0);
            e.gravity = [0.0, -30.0];
            e.tangential_accel = v(0.0, 60.0);
            e.size_start = v(12.0, 6.0);
            e.size_end = v(22.0, 8.0);
            e.color_start = rgba(200, 240, 255, 220);
            e.color_end = rgba(200, 240, 255, 120);
            e.fade_in = v(0.2, 0.0);
            e.fade_out = v(0.3, 0.0);
        },
    },
    Preset {
        name: "Fireflies",
        hint: "little lights wandering around an area",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Glow;
            e.additive = true;
            e.shape = EmitterShape::Box;
            e.size = [300.0, 150.0];
            e.rate = 6.0;
            e.quick_start = true;
            e.lifetime = v(4.0, 1.5);
            e.direction = v(0.0, 180.0);
            e.speed = v(25.0, 15.0);
            e.gravity = [0.0, 0.0];
            e.tangential_accel = v(0.0, 40.0);
            e.size_start = v(14.0, 5.0);
            e.size_end_same = true;
            e.color_start = rgba(220, 255, 120, 255);
            e.color_start_variance = rgba(20, 0, 40, 0);
            e.color_end = rgba(220, 255, 120, 255);
            e.fade_in = v(1.0, 0.3);
            e.fade_out = v(1.2, 0.3);
        },
    },
    Preset {
        name: "Fountain",
        hint: "water shooting up and splashing down",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Circle;
            e.additive = false;
            e.shape = EmitterShape::Point;
            e.rate = 120.0;
            e.lifetime = v(1.6, 0.3);
            e.direction = v(270.0, 14.0);
            e.speed = v(520.0, 60.0);
            e.gravity = [0.0, 700.0];
            e.size_start = v(9.0, 3.0);
            e.size_end = v(5.0, 2.0);
            e.color_start = rgba(120, 200, 255, 230);
            e.color_end = rgba(60, 140, 255, 0);
            e.edge_bounce = 0.3;
        },
    },
    Preset {
        name: "Electric",
        hint: "short crackling bolts",
        apply: |e, _| {
            reset(e);
            e.look = ParticleLook::Spark;
            e.additive = true;
            e.shape = EmitterShape::Point;
            e.rate = 200.0;
            e.lifetime = v(0.15, 0.08);
            e.direction = v(0.0, 180.0);
            e.speed = v(600.0, 300.0);
            e.gravity = [0.0, 0.0];
            e.drag = v(4.0, 0.0);
            e.size_start = v(5.0, 1.0);
            e.size_end = v(1.0, 0.0);
            e.stretch = 2.5;
            e.face_motion = true;
            e.color_start = rgba(180, 240, 255, 255);
            e.color_end = rgba(90, 140, 255, 0);
        },
    },
];

/// Applies the preset called `name`; false when there is none.
pub fn apply_preset(name: &str, emitter: &mut ParticleEmitter, stage: [f32; 2]) -> bool {
    let Some(preset) = PRESETS.iter().find(|preset| preset.name == name) else {
        return false;
    };
    (preset.apply)(emitter, stage);
    true
}

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|preset| preset.name)
}

// ------------------------------------------------------------------ labels

fn look_label(look: ParticleLook) -> &'static str {
    match look {
        ParticleLook::Circle => "Dots",
        ParticleLook::Glow => "Glow",
        ParticleLook::Square => "Squares",
        ParticleLook::Star => "Stars",
        ParticleLook::Spark => "Streaks",
        ParticleLook::Ring => "Rings",
        ParticleLook::Item(_) => "Library item",
    }
}

fn shape_label(shape: EmitterShape) -> &'static str {
    match shape {
        EmitterShape::Point => "Point",
        EmitterShape::Line => "Line",
        EmitterShape::Box => "Box",
        EmitterShape::Circle => "Circle",
        EmitterShape::Ring => "Ring",
    }
}

// ------------------------------------------------------------- UI helpers

/// A value with its ± variance next to it.
fn varied(
    edit: &mut Edit,
    ui: &mut Ui,
    value: &mut Varied,
    speed: f64,
    range: RangeInclusive<f32>,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        let span = (range.end() - range.start()).abs();
        edit.drag(ui, &mut value.value, speed, range, suffix);
        ui.label("±");
        edit.drag(ui, &mut value.variance, speed, 0.0..=span, suffix);
    });
}

/// How far each colour channel may stray, either way.
fn color_variance(edit: &mut Edit, ui: &mut Ui, value: &mut Rgba) {
    ui.horizontal(|ui| {
        ui.label("±");
        for (channel, prefix) in [
            (&mut value.r, "r "),
            (&mut value.g, "g "),
            (&mut value.b, "b "),
            (&mut value.a, "a "),
        ] {
            let response = ui.add(
                egui::DragValue::new(channel)
                    .clamp_range(0..=255)
                    .prefix(prefix),
            );
            edit.note(&response);
        }
    });
}

fn mark(edit: &mut Edit) {
    edit.snapshot = true;
    edit.changed = true;
}

/// The object selected on this timeline, if it can be followed.
fn selected_instance(app: &EditorApp, q0rg_id: u16) -> Option<(u32, Target)> {
    let reference =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
            .into_iter()
            .next()?;
    if reference.q0rg_id != q0rg_id {
        return None;
    }
    let placement = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)?;
    Some((placement.instance_id, placement.target))
}

fn followed_name(app: &EditorApp, q0rg_id: u16, instance_id: u32) -> String {
    app.state
        .project
        .runtime
        .instance_names
        .get(&q0s_format::v2::InstanceKey::new(q0rg_id, instance_id))
        .cloned()
        .unwrap_or_else(|| format!("object {instance_id}"))
}

/// Where the emitter's centre is on the stage on `frame`.
fn emitter_centre(app: &EditorApp, emitter: &ParticleEmitter, frame: u16) -> [f32; 2] {
    let [x, y] = emitter.position;
    let anchor = emitter.follow.and_then(|instance_id| {
        follow_point(
            &app.state.project,
            emitter.q0rg_id,
            frame,
            instance_id,
            &RuntimeSceneState::new(),
            None,
        )
    });
    match anchor {
        Some([ax, ay]) => [ax + x, ay + y],
        None => [x, y],
    }
}

// ------------------------------------------------------- shared sections

/// Name, look and where it comes from: the same in Simple mode and on the
/// Pro Emitter page.
fn render_where(
    app: &EditorApp,
    ui: &mut Ui,
    edit: &mut Edit,
    emitter: &mut ParticleEmitter,
    dim: Color32,
) {
    let stage = [
        f32::from(app.state.project.meta.stage_width.max(1)),
        f32::from(app.state.project.meta.stage_height.max(1)),
    ];
    let selected = selected_instance(app, emitter.q0rg_id);
    let area = selection_area(app);
    egui::Grid::new("particles_where")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            edit.text(ui, &mut emitter.name, 160.0);
            ui.end_row();

            ui.label("Position");
            ui.horizontal(|ui| {
                edit.drag(
                    ui,
                    &mut emitter.position[0],
                    1.0,
                    -100_000.0..=100_000.0,
                    " x",
                );
                edit.drag(
                    ui,
                    &mut emitter.position[1],
                    1.0,
                    -100_000.0..=100_000.0,
                    " y",
                );
                if emitter.follow.is_none() {
                    if ui
                        .add_enabled(area.is_some(), egui::Button::new("Selection").small())
                        .on_hover_text("Move to the middle of the selected object")
                        .clicked()
                    {
                        if let Some([x, y, w, h]) = area {
                            emitter.position = [x + w * 0.5, y + h * 0.5];
                            mark(edit);
                        }
                    }
                    if ui.small_button("Centre").clicked() {
                        emitter.position = [stage[0] * 0.5, stage[1] * 0.5];
                        mark(edit);
                    }
                }
            });
            ui.end_row();

            ui.label("Follows");
            ui.horizontal(|ui| match emitter.follow {
                Some(instance_id) => {
                    ui.label(followed_name(app, emitter.q0rg_id, instance_id));
                    if ui
                        .small_button("Let go")
                        .on_hover_text("Stay where it is now instead")
                        .clicked()
                    {
                        emitter.position = emitter_centre(app, emitter, app.session.current_frame);
                        emitter.follow = None;
                        mark(edit);
                    }
                }
                None => {
                    if ui
                        .add_enabled(
                            selected.is_some(),
                            egui::Button::new("Selected object").small(),
                        )
                        .on_hover_text("Ride along with the selected object as it moves")
                        .on_disabled_hover_text("Select an object on this timeline first")
                        .clicked()
                    {
                        edit.follow_selection = true;
                    }
                    ui.label(RichText::new("nothing").small().color(dim));
                }
            });
            ui.end_row();
            if emitter.follow.is_some() {
                ui.label("Particles");
                ui.horizontal(|ui| {
                    edit.choice(ui, &mut emitter.space, ParticleSpace::Free, "Left behind");
                    edit.choice(
                        ui,
                        &mut emitter.space,
                        ParticleSpace::Relative,
                        "Carried along",
                    );
                });
                ui.end_row();
            }

            ui.label("Comes from");
            ui.horizontal_wrapped(|ui| {
                for shape in EmitterShape::ALL {
                    edit.choice(ui, &mut emitter.shape, shape, shape_label(shape));
                }
            });
            ui.end_row();
            match emitter.shape {
                EmitterShape::Point => {}
                EmitterShape::Line => {
                    ui.label("Length");
                    edit.drag(ui, &mut emitter.size[0], 1.0, 0.0..=100_000.0, " px");
                    ui.end_row();
                }
                EmitterShape::Box => {
                    ui.label("Size");
                    ui.horizontal(|ui| {
                        edit.drag(ui, &mut emitter.size[0], 1.0, 0.0..=100_000.0, " w");
                        edit.drag(ui, &mut emitter.size[1], 1.0, 0.0..=100_000.0, " h");
                    });
                    ui.end_row();
                }
                EmitterShape::Circle | EmitterShape::Ring => {
                    ui.label("Radius");
                    edit.drag(ui, &mut emitter.size[0], 1.0, 0.0..=100_000.0, " px");
                    ui.end_row();
                }
            }
        });
}

/// The look buttons, with library items.
fn render_look(app: &EditorApp, ui: &mut Ui, edit: &mut Edit, emitter: &mut ParticleEmitter) {
    let selected_target = selected_instance(app, emitter.q0rg_id).map(|(_, target)| target);
    ui.horizontal_wrapped(|ui| {
        for look in ParticleLook::BUILT_IN {
            if ui
                .selectable_label(emitter.look == look, look_label(look))
                .clicked()
                && emitter.look != look
            {
                if matches!(emitter.look, ParticleLook::Item(_)) {
                    // Back from a library item's scale to pixels.
                    emitter.size_start = v(16.0, 4.0);
                    emitter.size_end = v(4.0, 2.0);
                }
                emitter.look = look;
                mark(edit);
            }
        }
        let is_item = matches!(emitter.look, ParticleLook::Item(_));
        let response = ui
            .add_enabled(
                selected_target.is_some() || is_item,
                egui::SelectableLabel::new(
                    is_item,
                    if is_item {
                        "Library item ✓"
                    } else {
                        "Selected object"
                    },
                ),
            )
            .on_hover_text("Emit copies of the object selected on the stage")
            .on_disabled_hover_text("Select an object on the stage to emit copies of it");
        if response.clicked() {
            if let Some(target) = selected_target {
                if !is_item {
                    // Items are sized as a scale, not in pixels.
                    emitter.size_start = v(0.5, 0.1);
                    emitter.size_end = v(0.2, 0.05);
                }
                emitter.look = ParticleLook::Item(target);
                mark(edit);
            }
        }
    });
}

fn render_live(edit: &mut Edit, ui: &mut Ui, emitter: &mut ParticleEmitter, dim: Color32) {
    edit.checkbox(ui, &mut emitter.live, "Live in q0player");
    if emitter.live {
        ui.label(
            RichText::new(format!(
                "import q0.particles, then q0particles.start! \"{0}\", q0particles.burst! \"{0}\", 30…",
                emitter.name
            ))
            .small()
            .color(dim),
        );
    }
}

fn size_unit(emitter: &ParticleEmitter) -> (f64, RangeInclusive<f32>, &'static str) {
    if matches!(emitter.look, ParticleLook::Item(_)) {
        (0.01, 0.0..=20.0, "×")
    } else {
        (0.2, 0.0..=10_000.0, " px")
    }
}

// -------------------------------------------------------------- Simple mode

fn render_emitter_simple(
    app: &EditorApp,
    ui: &mut Ui,
    edit: &mut Edit,
    emitter: &mut ParticleEmitter,
    dim: Color32,
) {
    let stage = [
        f32::from(app.state.project.meta.stage_width.max(1)),
        f32::from(app.state.project.meta.stage_height.max(1)),
    ];
    ui.label(RichText::new("Start from").strong());
    ui.horizontal_wrapped(|ui| {
        for preset in &PRESETS {
            if ui
                .small_button(preset.name)
                .on_hover_text(preset.hint)
                .clicked()
            {
                (preset.apply)(emitter, stage);
                mark(edit);
            }
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Looks like").strong());
    render_look(app, ui, edit, emitter);
    ui.horizontal(|ui| {
        edit.checkbox(ui, &mut emitter.additive, "Glows");
        ui.label(
            RichText::new("adds light to what is behind: fire, magic, sparks")
                .small()
                .color(dim),
        );
    });
    ui.add_space(4.0);
    render_where(app, ui, edit, emitter, dim);
    ui.add_space(4.0);
    let (size_speed, size_range, size_suffix) = size_unit(emitter);
    egui::Grid::new("particles_simple")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("How many");
            edit.slider(ui, &mut emitter.rate, 0.0..=500.0, true);
            ui.end_row();
            ui.label("Burst at start");
            let response = ui.add(egui::DragValue::new(&mut emitter.burst).clamp_range(0..=5000));
            edit.note(&response);
            ui.end_row();
            ui.label("Lives for");
            edit.slider(ui, &mut emitter.lifetime.value, 0.05..=20.0, true);
            ui.end_row();
            ui.label("Speed");
            edit.slider(ui, &mut emitter.speed.value, 0.0..=2000.0, false);
            ui.end_row();
            if emitter.motion == ParticleMotion::Gravity {
                ui.label("Direction");
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut emitter.direction.value, 1.0, -360.0..=720.0, "°");
                    ui.label("spread ±");
                    edit.drag(ui, &mut emitter.direction.variance, 1.0, 0.0..=180.0, "°");
                });
                ui.end_row();
                ui.label("Falls");
                edit.slider(ui, &mut emitter.gravity[1], -1500.0..=1500.0, false);
                ui.end_row();
                ui.label("Swirl");
                edit.slider(
                    ui,
                    &mut emitter.tangential_accel.value,
                    -600.0..=600.0,
                    false,
                );
                ui.end_row();
            } else {
                ui.label("Circles");
                ui.horizontal(|ui| {
                    edit.drag(
                        ui,
                        &mut emitter.start_radius.value,
                        1.0,
                        0.0..=10_000.0,
                        " px",
                    );
                    ui.label("→");
                    edit.drag(
                        ui,
                        &mut emitter.end_radius.value,
                        1.0,
                        0.0..=10_000.0,
                        " px",
                    );
                });
                ui.end_row();
                ui.label("Spins round");
                edit.slider(ui, &mut emitter.orbit.value, -720.0..=720.0, false);
                ui.end_row();
            }
            ui.label(if matches!(emitter.look, ParticleLook::Item(_)) {
                "Scale"
            } else {
                "Size"
            });
            ui.horizontal(|ui| {
                edit.drag(
                    ui,
                    &mut emitter.size_start.value,
                    size_speed,
                    size_range.clone(),
                    size_suffix,
                );
                ui.label("→");
                ui.add_enabled_ui(!emitter.size_end_same, |ui| {
                    edit.drag(
                        ui,
                        &mut emitter.size_end.value,
                        size_speed,
                        size_range,
                        size_suffix,
                    );
                });
            });
            ui.end_row();
            ui.label("Colour");
            ui.horizontal(|ui| {
                edit.color(ui, &mut emitter.color_start);
                ui.label("→");
                edit.color(ui, &mut emitter.color_end);
            });
            ui.end_row();
            ui.label("Fades");
            ui.horizontal(|ui| {
                edit.drag(ui, &mut emitter.fade_in.value, 0.01, 0.0..=60.0, " s in");
                edit.drag(ui, &mut emitter.fade_out.value, 0.01, 0.0..=60.0, " s out");
            });
            ui.end_row();
        });
    ui.horizontal_wrapped(|ui| {
        edit.checkbox(
            ui,
            &mut emitter.quick_start,
            "Already going on the first frame",
        );
    });
    render_live(edit, ui, emitter, dim);
    if matches!(emitter.look, ParticleLook::Item(_)) {
        ui.label(
            RichText::new("library items keep their own colours; the colour's opacity fades them")
                .small()
                .color(dim),
        );
    }
}

// ----------------------------------------------------------------- Pro mode

fn render_pro_emitter(
    app: &EditorApp,
    ui: &mut Ui,
    edit: &mut Edit,
    emitter: &mut ParticleEmitter,
    dim: Color32,
) {
    render_where(app, ui, edit, emitter, dim);
    ui.add_space(4.0);
    egui::Grid::new("particles_pro_emitter")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Shape turn");
            edit.drag(ui, &mut emitter.angle, 1.0, -36_000.0..=36_000.0, "°");
            ui.end_row();
            ui.label("Emission");
            edit.drag(ui, &mut emitter.rate, 0.5, 0.0..=5000.0, " /s");
            ui.end_row();
            ui.label("Burst");
            let response = ui.add(egui::DragValue::new(&mut emitter.burst).clamp_range(0..=20_000));
            edit.note(&response);
            ui.end_row();
            ui.label("Max particles");
            let response =
                ui.add(egui::DragValue::new(&mut emitter.max_particles).clamp_range(1..=20_000));
            edit.note(&response);
            ui.end_row();
            ui.label("Duration");
            ui.horizontal(|ui| {
                edit.drag(ui, &mut emitter.emit_seconds, 0.01, 0.0..=3600.0, " s");
                ui.label(RichText::new("0 keeps going").small().color(dim));
            });
            ui.end_row();
            ui.label("Lifetime");
            varied(edit, ui, &mut emitter.lifetime, 0.01, 0.02..=600.0, " s");
            ui.end_row();
            ui.label("Quick start");
            edit.checkbox(
                ui,
                &mut emitter.quick_start,
                "already full on the first frame",
            );
            ui.end_row();
            ui.label("Seed");
            ui.horizontal(|ui| {
                let response = ui.add(egui::DragValue::new(&mut emitter.seed));
                edit.note(&response);
                if ui.small_button("New").clicked() {
                    emitter.seed = emitter
                        .seed
                        .wrapping_mul(1_664_525)
                        .wrapping_add(1_013_904_223);
                    mark(edit);
                }
            });
            ui.end_row();
        });
    ui.add_space(4.0);
    render_live(edit, ui, emitter, dim);
}

fn render_pro_motion(ui: &mut Ui, edit: &mut Edit, emitter: &mut ParticleEmitter, dim: Color32) {
    ui.horizontal(|ui| {
        ui.label("Mode");
        edit.choice(ui, &mut emitter.motion, ParticleMotion::Gravity, "Gravity");
        edit.choice(ui, &mut emitter.motion, ParticleMotion::Radius, "Radius");
    });
    ui.label(
        RichText::new(match emitter.motion {
            ParticleMotion::Gravity => "particles fly off; gravity, pulls and drag steer them",
            ParticleMotion::Radius => "particles circle the emitter from a start to an end radius",
        })
        .small()
        .color(dim),
    );
    ui.add_space(4.0);
    egui::Grid::new("particles_pro_motion")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(match emitter.motion {
                ParticleMotion::Gravity => "Angle",
                ParticleMotion::Radius => "Start angle",
            });
            varied(
                edit,
                ui,
                &mut emitter.direction,
                1.0,
                -36_000.0..=36_000.0,
                "°",
            );
            ui.end_row();
            match emitter.motion {
                ParticleMotion::Gravity => {
                    ui.label("Speed");
                    varied(
                        edit,
                        ui,
                        &mut emitter.speed,
                        1.0,
                        -100_000.0..=100_000.0,
                        " px/s",
                    );
                    ui.end_row();
                    ui.label("Gravity");
                    ui.horizontal(|ui| {
                        edit.drag(
                            ui,
                            &mut emitter.gravity[0],
                            5.0,
                            -100_000.0..=100_000.0,
                            " x",
                        );
                        edit.drag(
                            ui,
                            &mut emitter.gravity[1],
                            5.0,
                            -100_000.0..=100_000.0,
                            " y",
                        );
                    });
                    ui.end_row();
                    ui.label("Accel rad");
                    varied(
                        edit,
                        ui,
                        &mut emitter.radial_accel,
                        1.0,
                        -100_000.0..=100_000.0,
                        "",
                    );
                    ui.end_row();
                    ui.label("Accel tan");
                    varied(
                        edit,
                        ui,
                        &mut emitter.tangential_accel,
                        1.0,
                        -100_000.0..=100_000.0,
                        "",
                    );
                    ui.end_row();
                    ui.label("Friction");
                    varied(edit, ui, &mut emitter.drag, 0.01, 0.0..=100.0, " /s");
                    ui.end_row();
                    ui.label("Stage edges");
                    edit.slider(ui, &mut emitter.edge_bounce, 0.0..=1.0, false);
                    ui.end_row();
                }
                ParticleMotion::Radius => {
                    ui.label("Start radius");
                    varied(
                        edit,
                        ui,
                        &mut emitter.start_radius,
                        1.0,
                        0.0..=100_000.0,
                        " px",
                    );
                    ui.end_row();
                    ui.label("End radius");
                    varied(
                        edit,
                        ui,
                        &mut emitter.end_radius,
                        1.0,
                        0.0..=100_000.0,
                        " px",
                    );
                    ui.end_row();
                    ui.label("Rotate / sec");
                    varied(
                        edit,
                        ui,
                        &mut emitter.orbit,
                        1.0,
                        -100_000.0..=100_000.0,
                        "°",
                    );
                    ui.end_row();
                }
            }
            ui.label("Rotation");
            edit.checkbox(ui, &mut emitter.face_motion, "faces the way it flies");
            ui.end_row();
        });
    ui.label(
        RichText::new(match emitter.motion {
            ParticleMotion::Gravity => {
                "accel rad pushes away from the emitter (negative pulls in); accel tan pushes \
                 around it clockwise; stage edges: 0 lets particles leave, 1 bounces them back"
            }
            ParticleMotion::Radius => {
                "the same start and end radius keeps each particle on one circle"
            }
        })
        .small()
        .color(dim),
    );
}

fn render_pro_looks(
    app: &EditorApp,
    ui: &mut Ui,
    edit: &mut Edit,
    emitter: &mut ParticleEmitter,
    dim: Color32,
) {
    ui.label(RichText::new("Texture").strong());
    render_look(app, ui, edit, emitter);
    ui.add_space(4.0);
    let (size_speed, size_range, size_suffix) = size_unit(emitter);
    egui::Grid::new("particles_pro_looks")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Blending");
            ui.horizontal(|ui| {
                let mut additive = emitter.additive;
                edit.choice(ui, &mut additive, false, "Normal");
                edit.choice(ui, &mut additive, true, "Additive");
                emitter.additive = additive;
            });
            ui.end_row();
            ui.label("Start size");
            varied(
                edit,
                ui,
                &mut emitter.size_start,
                size_speed,
                size_range.clone(),
                size_suffix,
            );
            ui.end_row();
            ui.label("End size");
            ui.add_enabled_ui(!emitter.size_end_same, |ui| {
                varied(
                    edit,
                    ui,
                    &mut emitter.size_end,
                    size_speed,
                    size_range,
                    size_suffix,
                );
            });
            ui.end_row();
            ui.label("");
            edit.checkbox(ui, &mut emitter.size_end_same, "start size = end");
            ui.end_row();
            ui.label("Start spin");
            varied(
                edit,
                ui,
                &mut emitter.spin_start,
                1.0,
                -100_000.0..=100_000.0,
                "°",
            );
            ui.end_row();
            ui.label("End spin");
            ui.add_enabled_ui(!emitter.spin_end_same, |ui| {
                varied(
                    edit,
                    ui,
                    &mut emitter.spin_end,
                    1.0,
                    -100_000.0..=100_000.0,
                    "°",
                );
            });
            ui.end_row();
            ui.label("");
            edit.checkbox(ui, &mut emitter.spin_end_same, "start spin = end");
            ui.end_row();
            ui.label("Start colour");
            ui.vertical(|ui| {
                edit.color(ui, &mut emitter.color_start);
                color_variance(edit, ui, &mut emitter.color_start_variance);
            });
            ui.end_row();
            ui.label("End colour");
            ui.vertical(|ui| {
                edit.color(ui, &mut emitter.color_end);
                color_variance(edit, ui, &mut emitter.color_end_variance);
            });
            ui.end_row();
            ui.label("");
            edit.checkbox(ui, &mut emitter.color_sync, "one variance for r, g and b");
            ui.end_row();
            ui.label("Fade in");
            varied(edit, ui, &mut emitter.fade_in, 0.01, 0.0..=600.0, " s");
            ui.end_row();
            ui.label("Fade out");
            varied(edit, ui, &mut emitter.fade_out, 0.01, 0.0..=600.0, " s");
            ui.end_row();
            ui.label("Stretch");
            edit.drag(ui, &mut emitter.stretch, 0.05, 0.0..=20.0, "");
            ui.end_row();
            ui.label("Order");
            ui.horizontal(|ui| {
                edit.choice(ui, &mut emitter.newest_on_top, true, "Newest on top");
                edit.choice(ui, &mut emitter.newest_on_top, false, "Oldest on top");
            });
            ui.end_row();
        });
    ui.label(
        RichText::new(
            "spin is the angle at birth and at death: 0 → 720 turns twice; \
             stretch lengthens particles the faster they fly",
        )
        .small()
        .color(dim),
    );
}

// ------------------------------------------------------------ stage overlay

/// Drags the selected emitter around the stage by its middle. Returns
/// whether the pointer is busy with it, so the tools leave it alone.
pub fn handle_stage(
    app: &mut EditorApp,
    response: &egui::Response,
    cursor: Option<Vec2>,
    view: &StageView,
) -> bool {
    if !app.settings.particles_enabled || !app.session.particles.open {
        app.session.particles.drag = None;
        return false;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let Some(emitter) = app
        .session
        .particles
        .selected
        .and_then(|id| app.state.project.runtime.particles.emitter(q0rg_id, id))
        .cloned()
    else {
        app.session.particles.drag = None;
        return false;
    };
    let [cx, cy] = emitter_centre(app, &emitter, app.session.current_frame);
    let centre = view.stage_to_screen(Vec2::new(cx, cy));
    let near = |screen: egui::Pos2| screen.distance(centre) <= 10.0;
    if response.drag_started_by(egui::PointerButton::Primary) {
        let press = response
            .ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some(press) = press.filter(|press| near(*press)) {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.session.particles.drag = Some(EmitterDrag {
                emitter_id: emitter.emitter_id,
                press: view.screen_to_stage(press),
                position: emitter.position,
            });
            return true;
        }
        return false;
    }
    let Some(drag) = app.session.particles.drag else {
        if response.hover_pos().is_some_and(near) {
            response.ctx.set_cursor_icon(egui::CursorIcon::Move);
        }
        return false;
    };
    if response.drag_stopped() || !response.dragged() {
        app.session.particles.drag = None;
        return true;
    }
    if let Some(cursor) = cursor {
        let mut position = [
            drag.position[0] + cursor.x - drag.press.x,
            drag.position[1] + cursor.y - drag.press.y,
        ];
        if response.ctx.input(|input| input.modifiers.shift) {
            position = position.map(f32::round);
        }
        if let Some(stored) = app
            .state
            .project
            .runtime
            .particles
            .emitter_mut(q0rg_id, drag.emitter_id)
        {
            if stored.position != position {
                stored.position = position;
                app.state.mark_dirty();
            }
        }
    }
    true
}

/// The emitters' shapes and directions on the stage while the window is
/// open.
pub fn paint_stage_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if !app.settings.particles_enabled || !app.session.particles.open {
        return;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let accent = app.settings.theme.accent.to_color32();
    let previewing = crate::sim_preview::particles_simulation(app).is_some();
    for emitter in app
        .state
        .project
        .runtime
        .particles
        .emitters
        .iter()
        .filter(|emitter| emitter.q0rg_id == q0rg_id)
    {
        let selected = app.session.particles.selected == Some(emitter.emitter_id);
        let color = if selected {
            accent
        } else {
            accent.gamma_multiply(0.5)
        };
        let stroke = Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, color);
        let [cx, cy] = emitter_centre(app, emitter, frame);
        let to_screen = |x: f32, y: f32| {
            let (sin, cos) = emitter.angle.to_radians().sin_cos();
            view.stage_to_screen(Vec2::new(cx + cos * x - sin * y, cy + sin * x + cos * y))
        };
        let center = to_screen(0.0, 0.0);
        if emitter.follow.is_some() {
            let anchor = view.stage_to_screen(Vec2::new(
                cx - emitter.position[0],
                cy - emitter.position[1],
            ));
            painter.line_segment(
                [anchor, center],
                Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
            );
            painter.circle_filled(anchor, 3.0, color.gamma_multiply(0.6));
        }
        match emitter.shape {
            EmitterShape::Point => {
                painter.circle_stroke(center, 5.0, stroke);
            }
            EmitterShape::Line => {
                let half = emitter.size[0] * 0.5;
                painter.line_segment([to_screen(-half, 0.0), to_screen(half, 0.0)], stroke);
            }
            EmitterShape::Box => {
                let [w, h] = [emitter.size[0] * 0.5, emitter.size[1] * 0.5];
                painter.add(egui::Shape::closed_line(
                    vec![
                        to_screen(-w, -h),
                        to_screen(w, -h),
                        to_screen(w, h),
                        to_screen(-w, h),
                    ],
                    stroke,
                ));
            }
            EmitterShape::Circle | EmitterShape::Ring => {
                let edge = to_screen(emitter.size[0], 0.0);
                painter.circle_stroke(center, center.distance(edge), stroke);
            }
        }
        match emitter.motion {
            ParticleMotion::Gravity => {
                // The way particles leave, and how far they may stray from it.
                let length = 40.0;
                let spread = emitter.direction.variance;
                for (offset, width) in [(0.0, 2.0_f32), (-spread, 1.0), (spread, 1.0)] {
                    let heading = (emitter.direction.value + offset).to_radians();
                    let tip = center + egui::vec2(heading.cos(), heading.sin()) * length;
                    if offset == 0.0 {
                        painter.arrow(center, tip - center, Stroke::new(width, color));
                    } else if spread > 0.5 {
                        painter.line_segment(
                            [center, tip],
                            Stroke::new(width, color.gamma_multiply(0.6)),
                        );
                    }
                }
            }
            ParticleMotion::Radius => {
                let scale = view.effective_scale().abs();
                for radius in [emitter.start_radius.value, emitter.end_radius.value] {
                    if radius > 0.5 {
                        painter.circle_stroke(
                            center,
                            radius * scale,
                            Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
                        );
                    }
                }
            }
        }
        if selected {
            painter.rect(
                egui::Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                1.0,
                Color32::from_black_alpha(140),
                Stroke::new(1.5_f32, color),
            );
        }
        painter.text(
            center + egui::vec2(8.0, -14.0),
            egui::Align2::LEFT_BOTTOM,
            &emitter.name,
            egui::FontId::proportional(11.0),
            color,
        );
    }
    if previewing {
        crate::sim_preview::paint_banner(painter, view, "Particles preview: bake to keep them", 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Asset, Layer, ProjectMeta, Q0rg, VectorAsset};

    fn project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "particles".into(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                background_color: rgba(255, 255, 255, 255),
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
                frame_count: 48,
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

    fn baked_layer(project: &ProjectV2) -> &Layer {
        let layer_id = project.runtime.particles.emitters[0].layer_id.unwrap();
        project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .unwrap()
    }

    #[test]
    fn an_emitter_bakes_tinted_shapes_onto_its_own_layer() {
        let mut project = project();
        let mut emitter = ParticleEmitter::new(1, 1, [200.0, 150.0]);
        emitter.look = ParticleLook::Circle;
        emitter.additive = false;
        emitter.fade_in = Varied::fixed(0.0);
        emitter.end_frame = 11;
        emitter.burst = 5;
        project.runtime.particles.emitters.push(emitter);
        let simulation = simulate(&project, 1, None).unwrap();
        assert_eq!(simulation.emitters[0].frames.len(), 12);
        assert_eq!(
            simulation.emitters[0].frames[0].len(),
            5,
            "the burst shows at once"
        );
        assert_eq!(bake(&mut project, &simulation).unwrap(), 12);

        let layer = baked_layer(&project);
        assert_eq!(layer.name, "emitter1");
        assert_eq!(layer.keyframe_frames(), (0..=11).collect::<Vec<_>>());
        assert_eq!(project.assets.len(), 1, "one shared dot drawing");
        let last = layer.placements.iter().filter(|p| p.frame == 11).count();
        assert!(last > 0);
        assert!(layer.placements.iter().all(|p| p.fx.tint.is_some()));
        assert!(layer
            .placements
            .iter()
            .all(|p| p.fx.blend_mode == BlendMode::Normal));

        // Baking again reuses the dot and replaces the keys.
        let again = simulate(&project, 1, None).unwrap();
        bake(&mut project, &again).unwrap();
        assert_eq!(project.assets.len(), 1);
        assert_eq!(
            baked_layer(&project)
                .placements
                .iter()
                .filter(|p| p.frame == 11)
                .count(),
            last
        );

        assert_eq!(clear_baked(&mut project, 1, None), 11);
        assert!(
            project.assets.is_empty(),
            "the dot goes once nothing uses it"
        );
    }

    #[test]
    fn glowing_particles_add_light_through_a_soft_gradient() {
        let mut project = project();
        let mut emitter = ParticleEmitter::new(1, 1, [200.0, 150.0]);
        emitter.look = ParticleLook::Glow;
        emitter.additive = true;
        emitter.end_frame = 5;
        emitter.burst = 4;
        project.runtime.particles.emitters.push(emitter);
        let simulation = simulate(&project, 1, None).unwrap();
        bake(&mut project, &simulation).unwrap();
        let layer = baked_layer(&project);
        assert!(!layer.placements.is_empty());
        assert!(layer
            .placements
            .iter()
            .all(|p| p.fx.blend_mode == BlendMode::Add));
        let Target::Asset(glow) = layer.placements[0].target else {
            panic!("a built-in look is a drawing");
        };
        let gradient = project.asset_appearances[&glow]
            .fill_gradient
            .as_ref()
            .unwrap();
        assert_eq!(gradient.kind, GradientKind::Radial);
        assert_eq!(gradient.stops.last().unwrap().color.a, 0, "fades out");
    }

    #[test]
    fn every_preset_bakes_something_visible() {
        for preset in &PRESETS {
            let mut project = project();
            let mut emitter = ParticleEmitter::new(1, 1, [200.0, 150.0]);
            (preset.apply)(&mut emitter, [400.0, 300.0]);
            emitter.end_frame = 23;
            project.runtime.particles.emitters.push(emitter);
            let simulation = simulate(&project, 1, None).unwrap();
            let shown = simulation.emitters[0].frames[23]
                .iter()
                .filter(|draw| draw.style.color.a > 0)
                .count();
            assert!(shown > 0, "{} shows nothing after a second", preset.name);
            bake(&mut project, &simulation).unwrap();
        }
    }

    #[test]
    fn an_emitter_rides_along_with_the_object_it_follows() {
        let mut project = project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![sim_bake::polygon_path(&[
                [-10.0, -10.0],
                [10.0, -10.0],
                [10.0, 10.0],
                [-10.0, 10.0],
            ])],
            fill: Some(rgba(0, 0, 0, 255)),
            stroke: None,
        }));
        let layer = &mut project.q0rgs[0].layers[0];
        // The object slides right by 10 px a frame.
        layer.placements.push(Placement {
            pivot: None,
            instance_id: 9,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 50.0,
                ty: 100.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::Linear { to_frame: 10 },
            fx: PlacementFx::default(),
        });
        layer.ensure_explicit_keyframe(10);
        layer.push_placement(Placement {
            pivot: None,
            instance_id: 9,
            frame: 10,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 150.0,
                ty: 100.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx::default(),
        });
        let mut emitter = ParticleEmitter::new(1, 1, [0.0, -20.0]);
        emitter.follow = Some(9);
        emitter.space = ParticleSpace::Relative;
        emitter.speed = Varied::fixed(0.0);
        emitter.gravity = [0.0, 0.0];
        emitter.rate = 0.0;
        emitter.burst = 1;
        emitter.lifetime = Varied::fixed(5.0);
        emitter.end_frame = 10;
        project.runtime.particles.emitters.push(emitter);
        let simulation = simulate(&project, 1, None).unwrap();
        let first = simulation.emitters[0].frames[0][0].position;
        let last = simulation.emitters[0].frames[10][0].position;
        assert!((first[0] - 50.0).abs() < 0.5 && (first[1] - 80.0).abs() < 0.5);
        assert!((last[0] - 150.0).abs() < 0.5 && (last[1] - 80.0).abs() < 0.5);
    }

    #[test]
    fn library_items_are_emitted_at_their_own_colours() {
        let mut project = project();
        let mut emitter = ParticleEmitter::new(1, 1, [200.0, 150.0]);
        emitter.end_frame = 3;
        emitter.burst = 3;
        emitter.fade_in = Varied::fixed(0.0);
        emitter.additive = false;
        emitter.look = ParticleLook::Item(Target::Q0rg(1));
        emitter.size_start = Varied::fixed(0.5);
        emitter.size_end = Varied::fixed(0.5);
        project.runtime.particles.emitters.push(emitter);
        let simulation = simulate(&project, 1, None).unwrap();
        bake(&mut project, &simulation).unwrap();
        let first = &baked_layer(&project).placements[0];
        assert_eq!(first.target, Target::Q0rg(1));
        assert!(first.fx.tint.is_none());
        assert!((first.transform.sy - 0.5).abs() < 1.0e-6);
        assert!(project.assets.is_empty());
    }
}
