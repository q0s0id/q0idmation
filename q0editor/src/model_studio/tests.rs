//! The 3D features through the editor: importing, the model editor and
//! saving back, and keying 3D placements.

use std::collections::BTreeSet;

use q0s_format::three::Space3d;
use q0s_format::v2::{Asset, Target};

use super::commands::{self, Command};
use super::{Constraint, Element, Mode, OpKind};
use crate::app::{Action, EditorApp};

fn project_app() -> (EditorApp, egui::Context) {
    let mut app = EditorApp::default();
    let ctx = egui::Context::default();
    app.handle(&ctx, Action::CreateProject(Default::default()));
    app.state.project.q0rgs[0].layers[0].placements.clear();
    (app, ctx)
}

fn models(app: &EditorApp) -> Vec<u16> {
    app.state
        .project
        .assets
        .iter()
        .filter_map(|a| match a {
            Asset::Model(m) => Some(m.asset_id),
            _ => None,
        })
        .collect()
}

#[test]
fn an_imported_file_lands_in_the_library_and_on_the_stage() {
    let (mut app, ctx) = project_app();
    let dir = std::env::temp_dir().join(format!("q0-3d-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("box.obj");
    std::fs::write(&path, "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n").unwrap();
    app.handle(&ctx, Action::ImportModelFromPath(path));
    let ids = models(&app);
    assert_eq!(ids.len(), 1, "{}", app.session.status);
    assert_eq!(
        app.state
            .project
            .asset_names
            .get(&ids[0])
            .map(String::as_str),
        Some("box")
    );
    let placed = &app.state.project.q0rgs[0].layers[0].placements;
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].target, Target::Asset(ids[0]));
    // Sized for the stage: half its height across.
    let data = match &app.state.project.assets[app.state.project.assets.len() - 1] {
        Asset::Model(m) => m.data.clone(),
        _ => unreachable!(),
    };
    let across = data
        .bounds()
        .map(|(lo, hi)| (hi - lo).max_elem() * data.scale)
        .unwrap();
    let expected = f32::from(app.state.project.meta.stage_height) * 0.5;
    assert!((across - expected).abs() < 1.0, "{across} vs {expected}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_model_editor_extrudes_and_saves_back() {
    let (mut app, ctx) = project_app();
    app.handle(&ctx, Action::NewModel);
    let id = models(&app)[0];
    assert!(
        app.model_studio.is_some(),
        "a new model opens in the editor"
    );
    let studio = app.model_studio.as_mut().unwrap();
    let faces_before = studio.model.meshes[0].faces.len();
    studio.mode = Mode::Edit;
    studio.element = Element::Face;
    let top = (0..faces_before)
        .find(|f| {
            let mesh = &studio.model.meshes[0];
            mesh.face_normal(&mesh.faces[*f]).y > 0.9
        })
        .unwrap();
    studio.faces = BTreeSet::from([top]);
    commands::extrude(studio, egui::pos2(0.0, 0.0), false);
    super::view::confirm(studio);
    assert_eq!(studio.model.meshes[0].faces.len(), faces_before + 4);
    commands::run(studio, Command::Smooth);
    let smoothed = studio.model.meshes[0].faces.len();
    assert_eq!(smoothed, (faces_before + 4) * 4);
    studio.undo();
    assert_eq!(
        studio.model.meshes[0].faces.len(),
        faces_before + 4,
        "undo steps back"
    );
    super::apply(&mut app);
    let stored = app
        .state
        .project
        .assets
        .iter()
        .find_map(|a| match a {
            Asset::Model(m) if m.asset_id == id => Some(m.data.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        stored.meshes[0].faces.len(),
        faces_before + 4,
        "saved to the library"
    );
    // The project's own undo takes the model back.
    app.handle(&ctx, Action::Undo);
    let restored = app
        .state
        .project
        .assets
        .iter()
        .find_map(|a| match a {
            Asset::Model(m) if m.asset_id == id => Some(m.data.meshes[0].faces.len()),
            _ => None,
        })
        .unwrap();
    assert_eq!(restored, faces_before);
}

#[test]
fn bones_bind_and_key_a_clip() {
    let (mut app, ctx) = project_app();
    app.handle(&ctx, Action::NewModel);
    let studio = app.model_studio.as_mut().unwrap();
    studio.mode = Mode::Pose;
    commands::run(studio, Command::AddBone);
    let bone = studio.bone.expect("the new bone is picked");
    commands::run(studio, Command::Bind);
    assert!(studio.model.meshes[0].skin.is_some(), "{}", studio.status);
    // Turn the bone by hand: a key appears in a new clip.
    studio.time = 0.5;
    super::view::begin(
        studio,
        OpKind::Rotate,
        egui::pos2(10.0, 0.0),
        Constraint::Free,
        "",
    );
    let view = studio.view(egui::Rect::from_min_size(
        egui::pos2(-200.0, -200.0),
        egui::vec2(400.0, 400.0),
    ));
    super::view::update(studio, &view, egui::pos2(0.0, 10.0), false);
    super::view::confirm(studio);
    assert_eq!(studio.model.clips.len(), 1);
    let clip = &studio.model.clips[0];
    assert!(clip
        .channels
        .iter()
        .any(|c| c.node == bone && c.times.iter().any(|t| (t - 0.5).abs() < 1.0e-4)));
}

#[test]
fn a_drawing_goes_into_3d_and_tweens_its_depth() {
    let fx = q0s_format::v2::PlacementFx {
        three: Some(Space3d {
            z: 100.0,
            ..Space3d::default()
        }),
        ..Default::default()
    };
    let later = q0s_format::v2::PlacementFx {
        three: Some(Space3d {
            z: 300.0,
            tilt_y: 90.0,
            ..Space3d::default()
        }),
        ..Default::default()
    };
    let mid = fx.lerp_to(later, 0.5).three.unwrap();
    assert!((mid.z - 200.0).abs() < 1.0e-3 && (mid.tilt_y - 45.0).abs() < 1.0e-3);
}
