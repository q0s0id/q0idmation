use q0s_format::raster::rasterize_q0rg_frame;
use q0s_format::v2::{
    parse, validate, write, Anchor, Asset, CameraKey, CameraState, Easing, Layer, Path, Placement,
    ProjectMeta, ProjectV2, Q0rg, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
    Q1S_VERSION_SHAPE_TWEENS,
};
use q0s_format::{parse_q0s_v2, write_q0s_v2, Error};

fn square(asset_id: u16, left: f32, color: (u8, u8, u8)) -> Asset {
    let corner = |x: f32, y: f32| Anchor {
        point: Vec2::new(x, y),
        in_handle: None,
        out_handle: None,
    };
    Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![Path {
            anchors: vec![
                corner(left, 24.0),
                corner(left + 16.0, 24.0),
                corner(left + 16.0, 40.0),
                corner(left, 40.0),
            ],
            closed: true,
        }],
        fill: Some(Rgba {
            r: color.0,
            g: color.1,
            b: color.2,
            a: 255,
        }),
        stroke: None,
    })
}

fn placement(frame: u16, asset_id: u16, tween: Tween) -> Placement {
    Placement {
        pivot: None,
        instance_id: 0,
        frame,
        target: Target::Asset(asset_id),
        transform: Transform2D::IDENTITY,
        tween,
        fx: Default::default(),
    }
}

/// A red square on the left at frame 0 that becomes a blue square on the
/// right at frame 4.
fn project(tween: Tween) -> ProjectV2 {
    ProjectV2 {
        meta: ProjectMeta {
            name: "morph".to_string(),
            fps: 24,
            stage_width: 64,
            stage_height: 64,
            background_color: Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets: vec![square(1, 8.0, (200, 0, 0)), square(2, 40.0, (0, 0, 200))],
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "Stage".to_string(),
            frame_count: 8,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "shapes".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![placement(0, 1, tween), placement(4, 2, Tween::None)],
            }],
        }],
    }
}

fn shape_tween() -> Tween {
    Tween::Shape {
        to_frame: 4,
        easing: Easing::Linear,
    }
}

fn pixel(project: &ProjectV2, frame: u16, x: usize, y: usize) -> [u8; 4] {
    let rgba = rasterize_q0rg_frame(project, 1, frame, 64, 64, 1, [255, 255, 255, 255]);
    let at = (y * 64 + x) * 4;
    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
}

fn near(actual: [u8; 4], wanted: [u8; 4]) -> bool {
    actual
        .iter()
        .zip(wanted)
        .all(|(left, right)| left.abs_diff(right) <= 3)
}

#[test]
fn shape_tweens_round_trip_in_both_formats() {
    let project = project(shape_tween());
    validate(&project).expect("shape tween project is valid");

    let q1s = write(&project).expect("write q1s");
    assert_eq!(
        u16::from_le_bytes([q1s[4], q1s[5]]),
        Q1S_VERSION_SHAPE_TWEENS
    );
    let mut expected = project.clone();
    expected.runtime.scenes = project.scenes();
    assert_eq!(parse(&q1s).expect("parse q1s"), expected);

    let q0s = write_q0s_v2(&project).expect("write q0s");
    assert_eq!(
        u16::from_le_bytes([q0s[4], q0s[5]]),
        q0s_format::q0s_v2::Q0S_VERSION_SHAPE_TWEENS
    );
    assert_eq!(parse_q0s_v2(&q0s).expect("parse q0s"), expected);
}

#[test]
fn projects_without_shape_tweens_keep_their_older_file_version() {
    for tween in [Tween::None, Tween::Linear { to_frame: 4 }] {
        let bytes = write(&project(tween)).expect("write q1s");
        assert!(
            u16::from_le_bytes([bytes[4], bytes[5]]) < Q1S_VERSION_SHAPE_TWEENS,
            "older editors must still open projects that do not use the feature"
        );
    }
}

#[test]
fn an_eased_shape_tween_keeps_its_curve() {
    let mut project = project(Tween::Shape {
        to_frame: 4,
        easing: Easing::CubicBezier {
            x1: 0.2,
            y1: 0.0,
            x2: 0.7,
            y2: 1.0,
        },
    });
    let bytes = write(&project).expect("write eased shape tween");
    let parsed = parse(&bytes).expect("parse eased shape tween");
    assert_eq!(
        parsed.q0rgs[0].layers[0].placements[0].tween,
        project.q0rgs[0].layers[0].placements[0].tween
    );

    // The same rules as any other tween still apply.
    project.q0rgs[0].layers[0].placements[0].tween = Tween::Shape {
        to_frame: 0,
        easing: Easing::Linear,
    };
    assert_eq!(
        validate(&project),
        Err(Error::Validation("tween to_frame must be > frame"))
    );
}

#[test]
fn camera_keys_cannot_be_shape_tweened() {
    let mut project = project(Tween::None);
    project.runtime.camera.enabled = true;
    let state = CameraState::identity_for_stage(64, 64);
    project.runtime.camera.keys = vec![
        CameraKey {
            frame: 0,
            state,
            tween: shape_tween(),
        },
        CameraKey {
            frame: 4,
            state,
            tween: Tween::None,
        },
    ];
    assert_eq!(
        validate(&project),
        Err(Error::Validation("camera keys cannot use shape tweens"))
    );
}

#[test]
fn exported_frames_show_the_in_between_shape() {
    let project = project(shape_tween());
    let white = [255, 255, 255, 255];

    // Both keyframes are the authored drawings.
    assert!(near(pixel(&project, 0, 16, 32), [200, 0, 0, 255]));
    assert!(near(pixel(&project, 0, 32, 32), white));
    assert!(near(pixel(&project, 4, 48, 32), [0, 0, 200, 255]));
    assert!(near(pixel(&project, 4, 32, 32), white));

    // Halfway, one purple square sits in the middle and neither end is drawn.
    assert!(near(pixel(&project, 2, 32, 32), [100, 0, 100, 255]));
    assert!(near(pixel(&project, 2, 12, 32), white));
    assert!(near(pixel(&project, 2, 52, 32), white));

    // A quarter of the way: 8..24 has moved to 16..32.
    assert!(near(pixel(&project, 1, 24, 32), [150, 0, 50, 255]));
    assert!(near(pixel(&project, 1, 10, 32), white));
}

#[test]
fn without_the_tween_the_first_drawing_is_simply_held() {
    let project = project(Tween::None);
    assert!(near(pixel(&project, 2, 16, 32), [200, 0, 0, 255]));
    assert!(near(pixel(&project, 2, 32, 32), [255, 255, 255, 255]));
}
