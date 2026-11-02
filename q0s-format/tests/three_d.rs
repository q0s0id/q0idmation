//! 3D through the whole format: models, placements in 3D and timeline 3D
//! setups survive saving, and the software renderer draws them in layer
//! order with the flat art around them.

use std::sync::Arc;

use q0s_format::raster::rasterize_q0rg_frame;
use q0s_format::three::model::{Material, Mesh, ModelData, Node};
use q0s_format::three::scene::{Light3d, Light3dKind, Space3d};
use q0s_format::three::{ModelAsset, V3};
use q0s_format::v2::{
    parse, write, Anchor, Asset, Layer, Path, Placement, PlacementFx, ProjectMeta, ProjectV2, Q0rg,
    Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
};

const RED: Rgba = Rgba {
    r: 220,
    g: 30,
    b: 30,
    a: 255,
};

fn cube_mesh() -> Mesh {
    let mut mesh = Mesh {
        name: "cube".into(),
        node: Some(0),
        ..Default::default()
    };
    for z in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for x in [-1.0, 1.0] {
                mesh.positions.push(V3::new(x, y, z));
            }
        }
    }
    for face in [
        [4, 5, 7, 6],
        [1, 0, 2, 3],
        [5, 1, 3, 7],
        [0, 4, 6, 2],
        [2, 6, 7, 3],
        [0, 1, 5, 4],
    ] {
        mesh.push_face(&face, None, 0, false);
    }
    mesh
}

fn cube_model(asset_id: u16) -> Asset {
    let mut data = ModelData::default();
    data.materials[0] = Material::with_color("red", RED);
    data.materials[0].shading = q0s_format::three::Shading::Unlit;
    data.nodes.push(Node::new("cube"));
    data.meshes.push(cube_mesh());
    data.scale = 20.0;
    Asset::Model(ModelAsset {
        asset_id,
        data: Arc::new(data),
    })
}

fn square(asset_id: u16, size: f32, fill: Rgba) -> Asset {
    let p = |x, y| Anchor {
        point: Vec2::new(x, y),
        in_handle: None,
        out_handle: None,
    };
    Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![Path {
            anchors: vec![
                p(-size, -size),
                p(size, -size),
                p(size, size),
                p(-size, size),
            ],
            closed: true,
        }],
        fill: Some(fill),
        stroke: None,
    })
}

fn placed(target: u16, x: f32, y: f32, three: Option<Space3d>) -> Placement {
    Placement {
        pivot: None,
        instance_id: 0,
        frame: 0,
        target: Target::Asset(target),
        transform: Transform2D {
            tx: x,
            ty: y,
            ..Transform2D::IDENTITY
        },
        tween: Tween::None,
        fx: PlacementFx {
            three,
            ..PlacementFx::default()
        },
    }
}

fn project(assets: Vec<Asset>, placements: Vec<Placement>) -> ProjectV2 {
    let mut project = ProjectV2 {
        meta: ProjectMeta {
            name: "three".into(),
            fps: 24,
            stage_width: 200,
            stage_height: 200,
            background_color: Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets,
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "Stage".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Layer 1".into(),
                explicit_keyframes: Vec::new(),
                placements,
            }],
        }],
    };
    project.runtime.scenes = project.scenes();
    project
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * 200 + x) * 4;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_placed_model_draws_in_software() {
    let project = project(vec![cube_model(1)], vec![placed(1, 100.0, 100.0, None)]);
    let pixels = rasterize_q0rg_frame(&project, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    let middle = pixel(&pixels, 100, 100);
    assert!(
        middle[0] > 180 && middle[1] < 80,
        "the cube is red: {middle:?}"
    );
    assert_eq!(pixel(&pixels, 10, 10), [255, 255, 255, 255]);
}

#[test]
fn flat_art_above_a_model_covers_it_and_below_does_not() {
    let blue = Rgba {
        r: 20,
        g: 40,
        b: 200,
        a: 255,
    };
    let below = project(
        vec![cube_model(1), square(2, 30.0, blue)],
        vec![placed(2, 100.0, 100.0, None), placed(1, 100.0, 100.0, None)],
    );
    let pixels = rasterize_q0rg_frame(&below, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    assert!(pixel(&pixels, 100, 100)[0] > 180, "the model is on top");
    let above = project(
        vec![cube_model(1), square(2, 30.0, blue)],
        vec![placed(1, 100.0, 100.0, None), placed(2, 100.0, 100.0, None)],
    );
    let pixels = rasterize_q0rg_frame(&above, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    assert!(pixel(&pixels, 100, 100)[2] > 180, "the square is on top");
}

#[test]
fn a_drawing_turned_in_3d_gets_narrower() {
    let green = Rgba {
        r: 20,
        g: 200,
        b: 40,
        a: 255,
    };
    let width = |pixels: &[u8]| {
        (0..200)
            .filter(|x| pixel(pixels, *x, 100)[1] > 150 && pixel(pixels, *x, 100)[0] < 100)
            .count()
    };
    let flat = project(
        vec![square(2, 60.0, green)],
        vec![placed(2, 100.0, 100.0, Some(Space3d::default()))],
    );
    let flat_width = width(&rasterize_q0rg_frame(
        &flat,
        1,
        0,
        200,
        200,
        1,
        [255, 255, 255, 255],
    ));
    let turned = project(
        vec![square(2, 60.0, green)],
        vec![placed(
            2,
            100.0,
            100.0,
            Some(Space3d {
                tilt_y: 60.0,
                ..Space3d::default()
            }),
        )],
    );
    let turned_width = width(&rasterize_q0rg_frame(
        &turned,
        1,
        0,
        200,
        200,
        1,
        [255, 255, 255, 255],
    ));
    assert!(
        (flat_width as i32 - 120).abs() <= 3,
        "flat card is as drawn: {flat_width}"
    );
    assert!(
        (turned_width as f32 - 60.0).abs() < 8.0,
        "turned card is half as wide: {turned_width}"
    );
}

#[test]
fn three_d_lights_reach_flat_art_when_asked() {
    let grey = Rgba {
        r: 128,
        g: 128,
        b: 128,
        a: 255,
    };
    let mut lit = project(
        vec![square(2, 90.0, grey)],
        vec![placed(2, 100.0, 100.0, None)],
    );
    let setup = lit.runtime.scene3d.setup_mut(1, 200, 200);
    let mut lamp = Light3d::new(1, Light3dKind::Point, V3::new(40.0, 100.0, -60.0), V3::ZERO);
    lamp.range = 200.0;
    lamp.intensity = 1.5;
    setup.lights.push(lamp);
    setup.ambient = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    setup.light_flat = true;
    let pixels = rasterize_q0rg_frame(&lit, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    let near = pixel(&pixels, 40, 100)[0];
    let far = pixel(&pixels, 185, 100)[0];
    assert!(
        near > 150 && far < 40,
        "near the lamp {near}, far from it {far}"
    );
    lit.runtime.scene3d.setups[0].light_flat = false;
    let pixels = rasterize_q0rg_frame(&lit, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    assert_eq!(pixel(&pixels, 185, 100)[0], 128, "unlit art stays as drawn");
}

#[test]
fn models_and_3d_settings_survive_saving() {
    let mut project = project(
        vec![cube_model(1), square(2, 10.0, RED)],
        vec![
            placed(
                1,
                50.0,
                60.0,
                Some(Space3d {
                    z: 40.0,
                    tilt_x: 10.0,
                    tilt_y: -30.0,
                    clip: 0,
                    clip_time: 0.5,
                    clip_speed: 1.5,
                    clip_loop: false,
                    lit: true,
                }),
            ),
            placed(2, 10.0, 10.0, Some(Space3d::default())),
        ],
    );
    let setup = project.runtime.scene3d.setup_mut(1, 200, 200);
    setup.camera.enabled = true;
    setup.camera.set_key(0);
    setup.lights.push(Light3d::new(
        1,
        Light3dKind::Spot,
        V3::new(1.0, 2.0, -3.0),
        V3::ZERO,
    ));
    let bytes = write(&project).unwrap();
    assert_eq!(parse(&bytes).unwrap(), project);
    let movie = q0s_format::write_q0s_v2(&project).unwrap();
    assert_eq!(q0s_format::parse_q0s_v2(&movie).unwrap(), project);
}

#[test]
fn a_model_casts_a_shadow_on_lit_flat_art() {
    let grey = Rgba {
        r: 200,
        g: 200,
        b: 200,
        a: 255,
    };
    let mut scene = project(
        vec![cube_model(1), square(2, 95.0, grey)],
        vec![
            placed(2, 100.0, 100.0, None),
            placed(
                1,
                100.0,
                100.0,
                Some(Space3d {
                    z: -60.0,
                    ..Space3d::default()
                }),
            ),
        ],
    );
    let setup = scene.runtime.scene3d.setup_mut(1, 200, 200);
    let mut sun = Light3d::new(
        1,
        Light3dKind::Sun,
        V3::new(-300.0, 100.0, -400.0),
        V3::new(100.0, 100.0, 0.0),
    );
    sun.shadows = true;
    setup.lights.push(sun);
    setup.ambient = Rgba {
        r: 40,
        g: 40,
        b: 40,
        a: 255,
    };
    setup.light_flat = true;
    let pixels = rasterize_q0rg_frame(&scene, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    let shaded = pixel(&pixels, 160, 100)[0];
    let open = pixel(&pixels, 40, 100)[0];
    assert!(
        open > shaded + 60,
        "the cube's shadow falls on the art: open {open}, shaded {shaded}"
    );
    scene.runtime.scene3d.setups[0].lights[0].shadows = false;
    let pixels = rasterize_q0rg_frame(&scene, 1, 0, 200, 200, 1, [255, 255, 255, 255]);
    let unshaded = pixel(&pixels, 160, 100)[0];
    assert!(
        (i32::from(unshaded) - i32::from(open)).abs() < 25,
        "without shadows it is lit: {unshaded} vs {open}"
    );
}
