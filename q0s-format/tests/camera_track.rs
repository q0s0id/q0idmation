use std::collections::HashMap;

use q0s_format::v2::{
    Asset, BitmapAsset, CameraKey, CameraState, Layer, Placement, PlacementFx, ProjectMeta,
    ProjectRuntimeData, ProjectV2, Q0rg, Target, Transform2D, Tween,
};

fn project_with_pixel() -> ProjectV2 {
    ProjectV2 {
        meta: ProjectMeta {
            name: "camera-test".into(),
            fps: 24,
            stage_width: 8,
            stage_height: 8,
            background_color: q0s_format::v2::Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets: vec![Asset::Bitmap(BitmapAsset {
            asset_id: 1,
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        })],
        asset_names: HashMap::new(),
        asset_appearances: HashMap::new(),
        layer_metadata: HashMap::new(),
        audio_clips: Vec::new(),
        runtime: ProjectRuntimeData::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "root".into(),
            frame_count: 11,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "art".into(),
                explicit_keyframes: vec![0],
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D {
                        tx: 4.0,
                        ty: 4.0,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                }],
            }],
        }],
    }
}

#[test]
fn disabled_camera_is_exact_identity() {
    let project = project_with_pixel();
    assert_eq!(project.camera_state_at(7), project.default_camera_state());
    assert_eq!(
        project.camera_affine_at(7),
        q0s_format::transform::Affine::IDENTITY
    );
}

#[test]
fn camera_tween_interpolates_zoom_and_full_rotation_without_collapsing_turn() {
    let mut project = project_with_pixel();
    project.runtime.camera.enabled = true;
    project.runtime.camera.keys = vec![
        CameraKey {
            frame: 0,
            state: CameraState {
                center_x: 4.0,
                center_y: 4.0,
                zoom: 1.0,
                rotation: 0.0,
            },
            tween: Tween::Linear { to_frame: 10 },
        },
        CameraKey {
            frame: 10,
            state: CameraState {
                center_x: 4.0,
                center_y: 4.0,
                zoom: 3.0,
                rotation: std::f32::consts::TAU,
            },
            tween: Tween::None,
        },
    ];

    let mid = project.camera_state_at(5);
    assert!((mid.center_x - 4.0).abs() < 1.0e-5);
    assert!((mid.center_y - 4.0).abs() < 1.0e-5);
    assert!((mid.zoom - 1.5).abs() < 1.0e-5);
    assert!((mid.rotation - std::f32::consts::PI).abs() < 1.0e-5);
}

#[test]
fn linear_camera_zoom_moves_authored_viewport_edges_linearly() {
    let mut project = project_with_pixel();
    project.meta.stage_width = 640;
    project.meta.stage_height = 480;
    project.q0rgs[0].frame_count = 24;
    project.runtime.camera.enabled = true;
    project.runtime.camera.keys = vec![
        CameraKey {
            frame: 0,
            state: CameraState {
                center_x: 320.0,
                center_y: 240.0,
                zoom: 1.0,
                rotation: 0.0,
            },
            tween: Tween::Linear { to_frame: 23 },
        },
        CameraKey {
            frame: 23,
            state: CameraState {
                center_x: 320.0,
                center_y: 240.0,
                zoom: 3.92,
                rotation: 0.0,
            },
            tween: Tween::None,
        },
    ];

    let edge_x = |state: CameraState| 320.0 - 320.0 / state.zoom;
    let first = edge_x(project.camera_state_at(0));
    let last = edge_x(project.camera_state_at(23));
    for frame in 0..=23 {
        let t = f32::from(frame) / 23.0;
        let expected = first + (last - first) * t;
        let actual = edge_x(project.camera_state_at(frame));
        assert!(
            (actual - expected).abs() < 1.0e-4,
            "camera viewport edge eased at frame {frame}: {actual} != {expected}"
        );
    }
}

#[test]
fn linear_camera_pan_and_zoom_move_viewport_corners_linearly() {
    let mut project = project_with_pixel();
    project.meta.stage_width = 640;
    project.meta.stage_height = 480;
    project.q0rgs[0].frame_count = 31;
    project.runtime.camera.enabled = true;
    project.runtime.camera.keys = vec![
        CameraKey {
            frame: 0,
            state: CameraState {
                center_x: 528.353_7,
                center_y: 368.164_86,
                zoom: 3.92,
                rotation: 0.0,
            },
            tween: Tween::Linear { to_frame: 30 },
        },
        CameraKey {
            frame: 30,
            state: CameraState {
                center_x: 269.967_35,
                center_y: 235.887_37,
                zoom: 1.4,
                rotation: 0.0,
            },
            tween: Tween::None,
        },
    ];

    let corners = |state: CameraState| {
        let half_w = 320.0 / state.zoom;
        let half_h = 240.0 / state.zoom;
        [
            (state.center_x - half_w, state.center_y - half_h),
            (state.center_x + half_w, state.center_y - half_h),
            (state.center_x + half_w, state.center_y + half_h),
            (state.center_x - half_w, state.center_y + half_h),
        ]
    };
    let first = corners(project.camera_state_at(0));
    let last = corners(project.camera_state_at(30));
    for frame in 0..=30 {
        let t = f32::from(frame) / 30.0;
        let actual = corners(project.camera_state_at(frame));
        for index in 0..4 {
            let expected_x = first[index].0 + (last[index].0 - first[index].0) * t;
            let expected_y = first[index].1 + (last[index].1 - first[index].1) * t;
            assert!((actual[index].0 - expected_x).abs() < 1.0e-4);
            assert!((actual[index].1 - expected_y).abs() < 1.0e-4);
        }
    }
}
#[test]
fn camera_roundtrips_through_q1s_and_q0s() {
    let mut project = project_with_pixel();
    project.runtime.camera.enabled = true;
    project.runtime.camera.keys = vec![
        CameraKey {
            frame: 0,
            state: project.default_camera_state(),
            tween: Tween::Linear { to_frame: 10 },
        },
        CameraKey {
            frame: 10,
            state: CameraState {
                center_x: 7.0,
                center_y: 3.0,
                zoom: 1.75,
                rotation: 0.5,
            },
            tween: Tween::None,
        },
    ];

    let q1s = q0s_format::v2::write(&project).expect("write q1s");
    let reparsed = q0s_format::v2::parse(&q1s).expect("parse q1s");
    assert!(q0s_format::v2::wire_equivalent(&project, &reparsed));
    assert_eq!(reparsed.runtime.camera, project.runtime.camera);

    let q0s = q0s_format::write_q0s_v2(&project).expect("write q0s");
    let reparsed = q0s_format::parse_q0s_v2(&q0s).expect("parse q0s");
    assert!(q0s_format::v2::wire_equivalent(&project, &reparsed));
    assert_eq!(reparsed.runtime.camera, project.runtime.camera);
}

#[test]
fn raster_camera_moves_view_without_touching_object_transform() {
    let mut project = project_with_pixel();
    let original_transform = project.q0rgs[0].layers[0].placements[0].transform;
    let base = q0s_format::raster::rasterize_q0rg_frame(&project, 1, 0, 8, 8, 1, [0, 0, 0, 0]);

    project.runtime.camera.enabled = true;
    project.runtime.camera.keys = vec![CameraKey {
        frame: 0,
        state: CameraState {
            center_x: 5.0,
            center_y: 4.0,
            zoom: 1.0,
            rotation: 0.0,
        },
        tween: Tween::None,
    }];
    let moved = q0s_format::raster::rasterize_q0rg_frame(&project, 1, 0, 8, 8, 1, [0, 0, 0, 0]);

    let red_positions = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(i, px)| (px[0] > 200 && px[3] > 200).then_some((i % 8, i / 8)))
            .collect::<Vec<_>>()
    };
    let base_red = red_positions(&base);
    let moved_red = red_positions(&moved);
    assert!(!base_red.is_empty());
    assert!(!moved_red.is_empty());
    assert_eq!(moved_red[0].0 + 1, base_red[0].0);
    assert_eq!(moved_red[0].1, base_red[0].1);
    assert_eq!(
        project.q0rgs[0].layers[0].placements[0].transform,
        original_transform
    );
}
