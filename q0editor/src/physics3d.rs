//! 3D physics in the editor: which objects fall or stand still in 3D, and
//! baking their falls into a key on every frame, so they play the same in
//! every renderer and export.

use egui::{RichText, Ui};
use q0physics::liquid3d::{Obstacle3, Settings3, Tank3};
use q0physics::rigid3d::{simulate, Body3, Shape3};
use q0s_format::three::draw::model_root;
use q0s_format::three::model::mesh_matrix;
use q0s_format::three::scene::{Body3d, BodyShape3d, Liquid3d, Physics3d, Scene3dSetup, Space3d};
use q0s_format::three::{Quat, V3};
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, ProjectV2, Target, Transform2D, Tween, Vec2 as StageVec};

use crate::app::EditorApp;
use crate::physics::Edit;
use crate::state::PlacementRef;

/// Frames baked when nothing else is asked for: two seconds at 24 fps.
pub(crate) const DEFAULT_FRAMES: u16 = 48;

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

fn store(app: &mut EditorApp, edit: &Edit, q0rg_id: u16, physics: Physics3d) {
    crate::physics::apply_edit(app, edit, |project| {
        let (w, h) = (project.meta.stage_width, project.meta.stage_height);
        project.runtime.scene3d.setup_mut(q0rg_id, w, h).physics = physics;
    });
}

/// The body controls for one placed object, in its 3D properties.
pub(crate) fn body_controls(
    app: &mut EditorApp,
    ui: &mut Ui,
    reference: PlacementRef,
    instance_id: u32,
) {
    let q0rg_id = reference.q0rg_id;
    let mut physics = setup_of(app, q0rg_id).physics;
    let mut edit = Edit::default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Physics").strong());
        let mut has = instance_id != 0 && physics.body(instance_id).is_some();
        let response = ui
            .checkbox(&mut has, "has a body")
            .on_hover_text("It falls, bumps and lands in 3D when the physics is baked");
        edit.note(&response);
        if response.changed() {
            if has {
                let id = if instance_id == 0 {
                    crate::scene3d::assign_instance_id(app, reference)
                } else {
                    instance_id
                };
                if id != 0 {
                    physics.bodies.push(Body3d::new(id));
                }
            } else {
                physics
                    .bodies
                    .retain(|body| body.instance_id != instance_id);
            }
        }
    });
    let instance_id = crate::scene3d::instance_of(app, reference).unwrap_or(instance_id);
    if let Some(body) = physics.body_mut(instance_id) {
        egui::Grid::new("body3d")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("It");
                ui.horizontal(|ui| {
                    edit.choice(ui, &mut body.falls, true, "falls");
                    edit.choice(ui, &mut body.falls, false, "stays put");
                });
                ui.end_row();
                ui.label("Shape");
                ui.horizontal(|ui| {
                    edit.choice(ui, &mut body.shape, BodyShape3d::Box, "box");
                    edit.choice(ui, &mut body.shape, BodyShape3d::Ball, "ball");
                });
                ui.end_row();
                ui.label("Bounce");
                edit.slider(ui, &mut body.bounce, 0.0..=1.0, false);
                ui.end_row();
                ui.label("Grip");
                edit.slider(ui, &mut body.friction, 0.0..=2.0, false);
                ui.end_row();
                if body.falls {
                    ui.label("Weight");
                    edit.slider(ui, &mut body.weight, 0.05..=50.0, true);
                    ui.end_row();
                    ui.label("Push");
                    ui.horizontal(|ui| {
                        for (axis, value) in [
                            ("x", &mut body.push.x),
                            ("y", &mut body.push.y),
                            ("z", &mut body.push.z),
                        ] {
                            ui.label(RichText::new(axis).small());
                            edit.drag(ui, value, 5.0, -20_000.0..=20_000.0, "");
                        }
                    });
                    ui.end_row();
                    ui.label("Spin");
                    ui.horizontal(|ui| {
                        for (axis, value) in [
                            ("x", &mut body.spin.x),
                            ("y", &mut body.spin.y),
                            ("z", &mut body.spin.z),
                        ] {
                            ui.label(RichText::new(axis).small());
                            edit.drag(ui, value, 0.05, -60.0..=60.0, "");
                        }
                    });
                    ui.end_row();
                }
            });
    }
    if edit.changed || edit.snapshot {
        store(app, &edit, q0rg_id, physics);
    }
}

/// Gravity, the floor and the bake, in the 3D window.
pub(crate) fn world_controls(app: &mut EditorApp, ui: &mut Ui, frames: &mut u16) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    let mut physics = setup_of(app, q0rg_id).physics;
    let mut edit = Edit::default();
    ui.label(RichText::new("Physics").strong());
    egui::Grid::new("physics3d_world")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Gravity");
            edit.slider(ui, &mut physics.gravity, -3000.0..=3000.0, false);
            ui.end_row();
            ui.label("Floor");
            ui.horizontal(|ui| {
                edit.checkbox(ui, &mut physics.floor, "at the stage's bottom");
                if physics.floor {
                    edit.drag(
                        ui,
                        &mut physics.floor_offset,
                        1.0,
                        -20_000.0..=20_000.0,
                        " px lower",
                    );
                }
            });
            ui.end_row();
        });
    let bodies = physics.bodies.len();
    let falling = physics.bodies.iter().filter(|body| body.falls).count();
    ui.label(
        RichText::new(if bodies == 0 {
            "Give objects a body in their 3D properties: then they fall here.".to_string()
        } else {
            format!("{bodies} bodies, {falling} of them falling")
        })
        .small()
        .color(dim),
    );
    ui.horizontal(|ui| {
        ui.label("Frames");
        ui.add(egui::DragValue::new(frames).clamp_range(1..=2400));
        if ui
            .add_enabled(falling > 0, egui::Button::new("Bake falls"))
            .on_hover_text(
                "Simulate from this frame and key every falling body on every frame after it",
            )
            .clicked()
        {
            match bake(app, *frames) {
                Ok(keys) => app.session.status = format!("baked {keys} keys of 3D physics"),
                Err(error) => app.session.status = error,
            }
        }
    });
    if edit.changed || edit.snapshot {
        store(app, &edit, q0rg_id, physics);
    }
    ui.add_space(6.0);
    liquid_controls(app, ui, *frames);
}

fn store_liquids(app: &mut EditorApp, edit: &Edit, q0rg_id: u16, liquids: Vec<Liquid3d>) {
    crate::physics::apply_edit(app, edit, |project| {
        let (w, h) = (project.meta.stage_width, project.meta.stage_height);
        project.runtime.scene3d.setup_mut(q0rg_id, w, h).liquids = liquids;
    });
    app.textures.invalidate();
}

/// The liquids of the timeline: pour one, shape it, bake it.
fn liquid_controls(app: &mut EditorApp, ui: &mut Ui, frames: u16) {
    let dim = app.settings.theme.text_dim.to_color32();
    let q0rg_id = app.session.current_q0rg_id;
    let mut liquids = setup_of(app, q0rg_id).liquids;
    let mut edit = Edit::default();
    let mut bake_now = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Liquids").strong());
        if ui
            .small_button("+ Pour a liquid")
            .on_hover_text("A block of liquid over the middle of the stage, let go on this frame")
            .clicked()
        {
            let (w, h) = (
                f32::from(app.state.project.meta.stage_width),
                f32::from(app.state.project.meta.stage_height),
            );
            let id = liquids.iter().map(|l| l.liquid_id).max().unwrap_or(0) + 1;
            let mut liquid = Liquid3d::new(
                id,
                app.session.current_layer_id,
                V3::new(w * 0.4, h * 0.1, -w * 0.08),
                V3::new(w * 0.6, h * 0.3, w * 0.08),
            );
            liquid.start_frame = app.session.current_frame;
            liquids.push(liquid);
            app.session.scene3d.selected_liquid = Some(id);
            edit.changed = true;
            edit.snapshot = true;
        }
    });
    if liquids.is_empty() {
        ui.label(
            RichText::new("Water, paint, slime: pour a block of it and bake how it splashes.")
                .small()
                .color(dim),
        );
        if edit.changed || edit.snapshot {
            store_liquids(app, &edit, q0rg_id, liquids);
        }
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for liquid in &liquids {
            let chosen = app.session.scene3d.selected_liquid == Some(liquid.liquid_id);
            if ui.selectable_label(chosen, &liquid.name).clicked() {
                app.session.scene3d.selected_liquid = Some(liquid.liquid_id);
            }
        }
    });
    let chosen = app.session.scene3d.selected_liquid;
    let mut remove = None;
    if let Some(liquid) = liquids.iter_mut().find(|l| Some(l.liquid_id) == chosen) {
        egui::Grid::new("liquid3d")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Name");
                edit.text(ui, &mut liquid.name, 140.0);
                ui.end_row();
                ui.label("Looks");
                ui.horizontal(|ui| {
                    edit.color(ui, &mut liquid.color);
                    edit.choice(ui, &mut liquid.clear, true, "clear");
                    edit.choice(ui, &mut liquid.clear, false, "thick");
                });
                ui.end_row();
                for (label, corner) in [
                    ("Block from", &mut liquid.from),
                    ("Block to", &mut liquid.to),
                ] {
                    ui.label(label);
                    ui.horizontal(|ui| {
                        for (axis, value) in [
                            ("x", &mut corner.x),
                            ("y", &mut corner.y),
                            ("z", &mut corner.z),
                        ] {
                            ui.label(RichText::new(axis).small());
                            edit.drag(ui, value, 1.0, -20_000.0..=20_000.0, "");
                        }
                    });
                    ui.end_row();
                }
                ui.label("Drops apart");
                edit.slider(ui, &mut liquid.spacing, 6.0..=40.0, false);
                ui.end_row();
                ui.label("Splash");
                ui.horizontal(|ui| {
                    for (axis, value) in [
                        ("x", &mut liquid.splash.x),
                        ("y", &mut liquid.splash.y),
                        ("z", &mut liquid.splash.z),
                    ] {
                        ui.label(RichText::new(axis).small());
                        edit.drag(ui, value, 5.0, -10_000.0..=10_000.0, "");
                    }
                });
                ui.end_row();
                ui.label("Thickness");
                edit.slider(ui, &mut liquid.thickness, 0.0..=1.0, false);
                ui.end_row();
                ui.label("Cling");
                edit.slider(ui, &mut liquid.cling, 0.0..=1.0, false);
                ui.end_row();
                ui.label("Tank");
                edit.checkbox(ui, &mut liquid.tank, "walls round the stage keep it in");
                ui.end_row();
            });
        let drops = q0physics::liquid3d::fill(
            [liquid.from.x, liquid.from.y, liquid.from.z],
            [liquid.to.x, liquid.to.y, liquid.to.z],
            liquid.spacing,
        )
        .len();
        let baked = if liquid.frames.is_empty() {
            "not baked yet".to_string()
        } else {
            format!(
                "{} frames baked from frame {}",
                liquid.frames.len(),
                liquid.start_frame + 1
            )
        };
        ui.label(
            RichText::new(format!("{drops} drops, {baked}"))
                .small()
                .color(dim),
        );
        ui.horizontal(|ui| {
            if ui
                .button("Bake liquid")
                .on_hover_text(
                    "Let it go on this frame and keep where its drops are on every frame after",
                )
                .clicked()
            {
                bake_now = Some(liquid.liquid_id);
            }
            if ui.button("Remove").clicked() {
                remove = Some(liquid.liquid_id);
            }
        });
    }
    if let Some(id) = remove {
        liquids.retain(|l| l.liquid_id != id);
        app.session.scene3d.selected_liquid = None;
        edit.changed = true;
        edit.snapshot = true;
    }
    if edit.changed || edit.snapshot {
        store_liquids(app, &edit, q0rg_id, liquids);
    }
    if let Some(id) = bake_now {
        match bake_liquid(app, id, frames) {
            Ok(drops) => {
                app.session.status = format!("baked a liquid of {drops} drops over {frames} frames")
            }
            Err(error) => app.session.status = error,
        }
    }
}

/// Lets liquid `liquid_id` go on the current frame and keeps where its
/// drops are for `frames` frames. Returns how many drops it has.
pub fn bake_liquid(app: &mut EditorApp, liquid_id: u16, frames: u16) -> Result<usize, String> {
    let q0rg_id = app.session.current_q0rg_id;
    let start = app.session.current_frame;
    let setup = setup_of(app, q0rg_id);
    let liquid = setup
        .liquids
        .iter()
        .find(|l| l.liquid_id == liquid_id)
        .ok_or("that liquid is gone")?
        .clone();
    let project = &app.state.project;
    // Bodies stand where they are on this frame for the liquid to run over.
    let mut obstacles = Vec::new();
    for body in &setup.physics.bodies {
        let Some(reference) = find(project, q0rg_id, body.instance_id, start) else {
            continue;
        };
        let Some(object) = crate::tools3d::object_state(app, reference) else {
            continue;
        };
        let Some(target) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get(reference.placement_idx))
            .map(|placement| placement.target)
        else {
            continue;
        };
        let Some((shape, offset)) =
            shape_of(project, target, &object.transform, object.pivot, body.shape)
        else {
            continue;
        };
        let q = crate::tools3d::orientation_of(&object.transform, &object.space);
        let rotation = [q.x, q.y, q.z, q.w];
        let centre = object.centre + q.rotate(V3::new(offset[0], offset[1], offset[2]));
        let centre = [centre.x, centre.y, centre.z];
        obstacles.push(match shape {
            Shape3::Box(half) => Obstacle3::Box {
                centre,
                half,
                rotation,
            },
            Shape3::Ball(radius) => Obstacle3::Ball { centre, radius },
        });
    }
    let (w, h) = (
        f32::from(project.meta.stage_width),
        f32::from(project.meta.stage_height),
    );
    let physics = &setup.physics;
    let tank = Tank3 {
        floor: physics.floor.then_some(h + physics.floor_offset),
        walls: liquid.tank.then_some([0.0, w, -w * 0.5, w * 0.5]),
    };
    let drops = q0physics::liquid3d::fill(
        [liquid.from.x, liquid.from.y, liquid.from.z],
        [liquid.to.x, liquid.to.y, liquid.to.z],
        liquid.spacing,
    );
    let count = drops.len();
    let fps = f32::from(project.meta.fps.max(1));
    let baked = q0physics::liquid3d::simulate(
        drops,
        [liquid.splash.x, liquid.splash.y, liquid.splash.z],
        Settings3 {
            spacing: liquid.spacing,
            thickness: liquid.thickness,
            cling: liquid.cling,
        },
        physics.gravity,
        tank,
        &obstacles,
        fps,
        usize::from(frames),
    );
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let project = &mut app.state.project;
    let (sw, sh) = (project.meta.stage_width, project.meta.stage_height);
    let layer = liquid.layer_id;
    if let Some(stored) = project
        .runtime
        .scene3d
        .setup_mut(q0rg_id, sw, sh)
        .liquids
        .iter_mut()
        .find(|l| l.liquid_id == liquid_id)
    {
        stored.start_frame = start;
        stored.frames = baked;
    }
    crate::sim_bake::reach_frame(project, q0rg_id, layer, start.saturating_add(frames));
    app.state.mark_dirty();
    app.textures.invalidate();
    Ok(count)
}

/// The placement of `instance_id` showing on `frame`.
fn find(project: &ProjectV2, q0rg_id: u16, instance_id: u32, frame: u16) -> Option<PlacementRef> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    q0rg.layers.iter().find_map(|layer| {
        crate::render::active_placements_at(layer, frame)
            .into_iter()
            .find(|(index, _)| layer.placements[*index].instance_id == instance_id)
            .map(|(placement_idx, _)| PlacementRef {
                q0rg_id,
                layer_id: layer.layer_id,
                placement_idx,
            })
    })
}

/// The box an object takes up in its own coordinates, as min and max.
fn local_bounds(project: &ProjectV2, target: Target) -> Option<(V3, V3)> {
    if let Target::Asset(id) = target {
        if let Some(model) = project.assets.iter().find_map(|asset| match asset {
            Asset::Model(model) if model.asset_id == id => Some(model),
            _ => None,
        }) {
            let root = model_root(&q0s_format::three::Mat4::IDENTITY, &model.data);
            let world = model.data.world_matrices(&model.data.rest_locals());
            let mut out: Option<(V3, V3)> = None;
            for mesh in model.data.meshes.iter().filter(|mesh| !mesh.hidden) {
                let m = root * mesh_matrix(mesh, &world);
                for p in &mesh.positions {
                    let p = m.transform_point(*p);
                    out = Some(match out {
                        Some((lo, hi)) => (
                            V3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)),
                            V3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)),
                        ),
                        None => (p, p),
                    });
                }
            }
            return out;
        }
    }
    let (x0, y0, x1, y1) = crate::render::target_local_bbox(project, target, None)?;
    // A flat card is a thin slab.
    Some((V3::new(x0, y0, -1.0), V3::new(x1, y1, 1.0)))
}

/// A body's shape and where it sits about the object's pivot, sized as the
/// object shows.
fn shape_of(
    project: &ProjectV2,
    target: Target,
    transform: &Transform2D,
    pivot: StageVec,
    kind: BodyShape3d,
) -> Option<(Shape3, [f32; 3])> {
    let (lo, hi) = local_bounds(project, target)?;
    let depth = (transform.sx * transform.sy).abs().sqrt();
    let scale = V3::new(transform.sx.abs(), transform.sy.abs(), depth);
    let half = V3::new(
        (hi.x - lo.x) * 0.5 * scale.x,
        (hi.y - lo.y) * 0.5 * scale.y,
        (hi.z - lo.z) * 0.5 * scale.z,
    );
    let middle = (lo + hi) * 0.5;
    let offset = [
        (middle.x - pivot.x) * transform.sx,
        (middle.y - pivot.y) * transform.sy,
        middle.z * depth,
    ];
    let shape = match kind {
        BodyShape3d::Box => Shape3::Box([half.x, half.y, half.z]),
        BodyShape3d::Ball => Shape3::Ball(half.x.max(half.y).max(half.z)),
    };
    Some((shape, offset))
}

/// Puts a simulated pose on an object: its pivot where the body is, turned
/// as the body is.
fn posed(
    start: Transform2D,
    space: Space3d,
    pivot: StageVec,
    position: [f32; 3],
    rotation: [f32; 4],
) -> (Transform2D, Space3d) {
    let [x, y, z, w] = rotation;
    let (angle, lean, turn) = crate::tools3d::split_turn(Quat { x, y, z, w });
    let mut transform = Transform2D {
        rotation: angle,
        tx: 0.0,
        ty: 0.0,
        ..start
    };
    let at = Affine::from_transform(transform).apply(pivot);
    transform.tx = position[0] - at.x;
    transform.ty = position[1] - at.y;
    let space = Space3d {
        z: position[2],
        tilt_x: lean,
        tilt_y: turn,
        ..space
    };
    (transform, space)
}

/// Simulates from the current frame for `frames` frames and keys every
/// falling body on each of them. Returns how many keys were written.
pub fn bake(app: &mut EditorApp, frames: u16) -> Result<usize, String> {
    let q0rg_id = app.session.current_q0rg_id;
    let start = app.session.current_frame;
    let physics = setup_of(app, q0rg_id).physics;
    let project = &app.state.project;
    let mut inputs = Vec::new();
    // Per body: its layer, instance, start pose and pivot, and whether it falls.
    let mut tracks = Vec::new();
    for body in &physics.bodies {
        let Some(reference) = find(project, q0rg_id, body.instance_id, start) else {
            continue;
        };
        let Some(object) = crate::tools3d::object_state(app, reference) else {
            continue;
        };
        let target = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get(reference.placement_idx))
            .map(|placement| placement.target)
            .ok_or("an object with a body is gone")?;
        let Some((shape, offset)) =
            shape_of(project, target, &object.transform, object.pivot, body.shape)
        else {
            continue;
        };
        let q = crate::tools3d::orientation_of(&object.transform, &object.space);
        inputs.push(Body3 {
            dynamic: body.falls,
            shape,
            offset,
            position: [object.centre.x, object.centre.y, object.centre.z],
            rotation: [q.x, q.y, q.z, q.w],
            velocity: [body.push.x, body.push.y, body.push.z],
            spin: [body.spin.x, body.spin.y, body.spin.z],
            bounce: body.bounce,
            friction: body.friction,
            mass: body.weight,
        });
        tracks.push((
            reference.layer_id,
            body.instance_id,
            object.transform,
            object.space,
            object.pivot,
            body.falls,
        ));
    }
    if !tracks.iter().any(|track| track.5) {
        return Err(
            "nothing falls here: give an object a body that falls, on this frame".to_string(),
        );
    }
    let layers: std::collections::BTreeSet<u16> =
        tracks.iter().filter(|t| t.5).map(|t| t.0).collect();
    if let Some(locked) = layers
        .iter()
        .find(|layer| project.layer_is_locked(q0rg_id, **layer))
    {
        return Err(format!("unlock layer {locked} first"));
    }
    let floor = physics
        .floor
        .then(|| f32::from(project.meta.stage_height) + physics.floor_offset);
    let fps = f32::from(project.meta.fps.max(1));
    let poses = simulate(&inputs, physics.gravity, floor, fps, usize::from(frames));

    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let last = start.saturating_add(frames);
    for layer in &layers {
        crate::sim_bake::reach_frame(&mut app.state.project, q0rg_id, *layer, last);
    }
    let cells: Vec<(u16, u16)> = layers
        .iter()
        .flat_map(|layer| (start + 1..=last).map(move |frame| (*layer, frame)))
        .collect();
    crate::timeline_edit::materialize_keyframes_at_cells(&mut app.state.project, q0rg_id, &cells)
        .ok_or("could not add keyframes there")?;
    let mut keyed = 0;
    let q0rg = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .ok_or("the timeline is gone")?;
    for ((layer_id, instance_id, transform, space, pivot, falls), track) in
        tracks.iter().zip(&poses)
    {
        if !falls {
            continue;
        }
        let Some(layer) = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == *layer_id)
        else {
            continue;
        };
        for placement in layer.placements.iter_mut().filter(|placement| {
            placement.instance_id == *instance_id && (start + 1..=last).contains(&placement.frame)
        }) {
            let Some(pose) = track.get(usize::from(placement.frame - start)) else {
                continue;
            };
            let (moved, turned) = posed(*transform, *space, *pivot, pose.position, pose.rotation);
            placement.transform = moved;
            placement.fx.three = Some(turned);
            placement.tween = Tween::None;
            keyed += 1;
        }
    }
    app.state.mark_dirty();
    app.textures.invalidate();
    Ok(keyed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_puts_the_pivot_where_the_body_is() {
        let start = Transform2D {
            sx: 2.0,
            sy: 2.0,
            ..Transform2D::IDENTITY
        };
        let pivot = StageVec::new(10.0, 5.0);
        let half_turn = std::f32::consts::FRAC_PI_4.sin();
        let (moved, space) = posed(
            start,
            Space3d::default(),
            pivot,
            [300.0, 200.0, 40.0],
            [0.0, 0.0, half_turn, half_turn],
        );
        let at = Affine::from_transform(moved).apply(pivot);
        assert!((at.x - 300.0).abs() < 1.0e-3 && (at.y - 200.0).abs() < 1.0e-3);
        assert!((space.z - 40.0).abs() < 1.0e-6);
        assert!((moved.rotation - std::f32::consts::FRAC_PI_2).abs() < 1.0e-4);
        assert_eq!((moved.sx, moved.sy), (2.0, 2.0));
    }

    #[test]
    fn a_baked_liquid_lands_and_draws() {
        let mut app = EditorApp::default();
        let (w, h) = (
            app.state.project.meta.stage_width,
            app.state.project.meta.stage_height,
        );
        let mut liquid = Liquid3d::new(
            1,
            1,
            V3::new(280.0, 300.0, -20.0),
            V3::new(360.0, 360.0, 20.0),
        );
        liquid.spacing = 16.0;
        app.state
            .project
            .runtime
            .scene3d
            .setup_mut(1, w, h)
            .liquids
            .push(liquid);
        app.session.current_frame = 2;
        let drops = bake_liquid(&mut app, 1, 12).expect("bake");
        assert!(drops > 20);
        let stored = &app.state.project.runtime.scene3d.setup(1).unwrap().liquids[0];
        assert_eq!(stored.start_frame, 2);
        assert_eq!(stored.frames.len(), 13);
        assert!(stored.drops_at(1).is_none(), "nothing before it is let go");
        // It falls toward the floor, and a surface is drawn over it.
        let first = stored.frames[0].iter().map(|d| d[1]).max().unwrap();
        let last = stored.frames[12].iter().map(|d| d[1]).max().unwrap();
        assert!(last > first);
        let items = q0s_format::three::frame::liquid_items(&app.state.project, 1, 8);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, 1);
    }

    #[test]
    fn baking_drops_a_cube_onto_the_floor_frame_by_frame() {
        let mut app = EditorApp::default();
        app.handle(&egui::Context::default(), crate::app::Action::NewModel);
        app.model_studio = None;
        let reference = PlacementRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let id = crate::scene3d::assign_instance_id(&mut app, reference);
        let (w, h) = (
            app.state.project.meta.stage_width,
            app.state.project.meta.stage_height,
        );
        let setup = app.state.project.runtime.scene3d.setup_mut(1, w, h);
        setup.physics.bodies.push(Body3d::new(id));
        let keys = bake(&mut app, 24).expect("bake");
        assert_eq!(keys, 24);
        let layer = &app.state.project.q0rgs[0].layers[0];
        let first = layer.placements.iter().find(|p| p.frame == 1).unwrap();
        let last = layer.placements.iter().find(|p| p.frame == 24).unwrap();
        // It falls: further down the stage on each frame, never past the floor.
        assert!(
            last.transform.ty > first.transform.ty,
            "{:?} {:?}",
            first.transform,
            last.transform
        );
        assert!(app.history.can_undo());
    }
}
