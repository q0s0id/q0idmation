use q0s_format::v2::{
    parse, validate, write, Anchor, Asset, BitmapAsset, Easing, EasingFamily, EasingMode, Layer,
    Path, Placement, ProjectMeta, ProjectV2, Q0rg, Rgba, Stroke, Target, Transform2D, Tween, Vec2,
    VectorAsset, MAX_Q0RG_NESTING_DEPTH,
};
use q0s_format::Error;

fn sample_v2_project() -> ProjectV2 {
    ProjectV2 {
        meta: ProjectMeta {
            name: "demo-v2".to_string(),
            fps: 24,
            stage_width: 320,
            stage_height: 180,
            entry_q0rg_id: 1,
        },
        assets: vec![
            Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: vec![
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
                ],
            }),
            Asset::Vector(VectorAsset {
                asset_id: 2,
                paths: vec![Path {
                    closed: true,
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(0.0, 0.0),
                            in_handle: None,
                            out_handle: Some(Vec2::new(10.0, 0.0)),
                        },
                        Anchor {
                            point: Vec2::new(20.0, 0.0),
                            in_handle: Some(Vec2::new(15.0, 5.0)),
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(20.0, 20.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                }],
                fill: Some(Rgba {
                    r: 255,
                    g: 200,
                    b: 0,
                    a: 255,
                }),
                stroke: Some(Stroke {
                    color: Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    width: 1.5,
                    cap: q0s_format::geom::CapShape::Round,
                }),
            }),
        ],
        asset_names: std::collections::HashMap::new(),
        layer_metadata: std::collections::HashMap::new(),
        q0rgs: vec![
            Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 10,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer 1".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![
                        Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D {
                                tx: 32.0,
                                ty: 24.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                        },
                        Placement {
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D {
                                tx: 100.0,
                                ty: 50.0,
                                sx: 2.0,
                                sy: 2.0,
                                rotation: 0.5,
                                skew_x: 0.25,
                                skew_y: -0.1,
                            },
                            tween: Tween::Linear { to_frame: 5 },
                        },
                    ],
                }],
            },
            Q0rg {
                q0rg_id: 2,
                name: "Spinner".to_string(),
                frame_count: 6,
                script: "// spin\n".to_string(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer 1".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(2),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            },
        ],
    }
}

fn vector_asset_mut(project: &mut ProjectV2) -> &mut VectorAsset {
    let Asset::Vector(vector) = &mut project.assets[1] else {
        panic!("sample project vector asset is missing");
    };
    vector
}

fn q0rg_chain(edge_count: usize) -> ProjectV2 {
    let mut q0rgs = Vec::with_capacity(edge_count + 1);
    for index in 0..=edge_count {
        let q0rg_id = u16::try_from(index + 1).expect("test chain id must fit u16");
        let layers = if index < edge_count {
            vec![Layer {
                layer_id: 1,
                name: "Layer".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    frame: 0,
                    target: Target::Q0rg(q0rg_id + 1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                }],
            }]
        } else {
            Vec::new()
        };
        q0rgs.push(Q0rg {
            q0rg_id,
            name: format!("q{q0rg_id}"),
            frame_count: 1,
            script: String::new(),
            layers,
        });
    }

    ProjectV2 {
        meta: ProjectMeta {
            name: "nested".to_string(),
            fps: 24,
            stage_width: 64,
            stage_height: 64,
            entry_q0rg_id: 1,
        },
        assets: Vec::new(),
        asset_names: std::collections::HashMap::new(),
        layer_metadata: std::collections::HashMap::new(),
        q0rgs,
    }
}

#[test]
fn v2_roundtrip_binary() {
    let project = sample_v2_project();
    let bytes = write(&project).expect("must serialize");
    let parsed = parse(&bytes).expect("must parse");
    assert_eq!(project, parsed);
}

#[test]
fn custom_asset_names_survive_q1s_roundtrip() {
    let mut project = sample_v2_project();
    project
        .asset_names
        .insert(2, "hero vector / лицо".to_string());
    project.q0rgs[1].name = "hero symbol / герой".to_string();

    let bytes = write(&project).expect("must serialize names");
    let parsed = parse(&bytes).expect("must parse names");

    assert_eq!(parsed, project);
    assert_eq!(
        parsed.asset_names.get(&2).map(String::as_str),
        Some("hero vector / лицо")
    );
}

#[test]
fn asset_name_must_reference_an_asset_and_must_not_be_blank() {
    let mut missing = sample_v2_project();
    missing.asset_names.insert(999, "ghost".to_string());
    assert_eq!(
        validate(&missing).expect_err("missing asset name target must fail"),
        Error::Validation("asset name references a missing asset")
    );

    let mut blank = sample_v2_project();
    blank.asset_names.insert(2, "   ".to_string());
    assert_eq!(
        validate(&blank).expect_err("blank asset name must fail"),
        Error::Validation("asset name must not be empty")
    );
}

#[test]
fn layer_folders_and_depth_order_survive_q1s_roundtrip() {
    let mut project = sample_v2_project();
    project.q0rgs[0].layers = vec![
        Layer {
            layer_id: 40,
            name: "folder child low".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        },
        Layer {
            layer_id: 7,
            name: "folder child high".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        },
        Layer {
            layer_id: 90,
            name: "Folder".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        },
        Layer {
            layer_id: 2,
            name: "Top level".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        },
    ];
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 40),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Normal,
            parent_folder_id: Some(90),
            collapsed: false,
        },
    );
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 7),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Normal,
            parent_folder_id: Some(90),
            collapsed: false,
        },
    );
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 90),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Folder,
            parent_folder_id: None,
            collapsed: true,
        },
    );

    let bytes = write(&project).expect("serialize layer folders");
    let parsed = parse(&bytes).expect("parse layer folders");

    assert_eq!(parsed, project);
    assert_eq!(
        parsed.q0rgs[0]
            .layers
            .iter()
            .map(|layer| layer.layer_id)
            .collect::<Vec<_>>(),
        vec![40, 7, 90, 2]
    );
}

#[test]
fn layer_folder_cannot_hold_keyframes_or_have_noncontiguous_children() {
    let mut project = sample_v2_project();
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 1),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Folder,
            parent_folder_id: None,
            collapsed: false,
        },
    );
    assert_eq!(
        validate(&project).expect_err("folder with placements must fail"),
        Error::Validation("layer folder must not contain keyframes")
    );

    let mut project = sample_v2_project();
    project.q0rgs[0].layers.push(Layer {
        layer_id: 9,
        name: "Folder".to_string(),
        explicit_keyframes: Vec::new(),
        placements: Vec::new(),
    });
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 1),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Normal,
            parent_folder_id: Some(9),
            collapsed: false,
        },
    );
    project.layer_metadata.insert(
        q0s_format::v2::LayerKey::new(1, 9),
        q0s_format::v2::LayerMetadata {
            kind: q0s_format::v2::LayerKind::Folder,
            parent_folder_id: None,
            collapsed: false,
        },
    );
    project.q0rgs[0].layers.insert(
        1,
        Layer {
            layer_id: 8,
            name: "interloper".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        },
    );
    assert_eq!(
        validate(&project).expect_err("folder children must stay contiguous"),
        Error::Validation("folder child layers must immediately precede their folder")
    );
}

#[test]
fn blank_keyframes_survive_q1s_roundtrip() {
    let mut project = sample_v2_project();
    project.q0rgs[0].layers[0].explicit_keyframes = vec![3, 7];

    let bytes = write(&project).expect("must serialize blank keyframes");
    let parsed = parse(&bytes).expect("must parse blank keyframes");

    assert_eq!(parsed, project);
    assert!(parsed.q0rgs[0].layers[0].is_blank_keyframe(3));
    assert!(parsed.q0rgs[0].layers[0].is_blank_keyframe(7));
}

#[test]
fn blank_keyframes_must_be_unique_and_in_bounds() {
    let mut duplicate = sample_v2_project();
    duplicate.q0rgs[0].layers[0].explicit_keyframes = vec![3, 3];
    assert!(matches!(
        validate(&duplicate),
        Err(Error::Validation(
            "explicit keyframe frames must be unique within layer"
        ))
    ));

    let mut out_of_bounds = sample_v2_project();
    out_of_bounds.q0rgs[0].layers[0].explicit_keyframes = vec![10];
    assert!(matches!(
        validate(&out_of_bounds),
        Err(Error::Validation(
            "explicit keyframe is out of q0rg frame_count bounds"
        ))
    ));
}

#[test]
fn v2_rejects_q0rg_self_reference() {
    let mut project = sample_v2_project();
    project.q0rgs[1].layers[0].placements[0].target = Target::Q0rg(2);
    let err = validate(&project).expect_err("must fail");
    assert!(matches!(
        err,
        Error::Validation("q0rg cannot reference itself")
    ));
}

#[test]
fn v2_detects_q0rg_cycle() {
    let mut project = sample_v2_project();
    // q0rg 2 references q0rg 1, while q0rg 1 already references 2 в†’ cycle.
    project.q0rgs[1].layers[0].placements.push(Placement {
        frame: 0,
        target: Target::Q0rg(1),
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
    });
    let err = validate(&project).expect_err("must fail");
    assert!(matches!(err, Error::Validation("q0rg cycle detected")));
}

#[test]
fn v2_rejects_invalid_magic() {
    let mut bytes = write(&sample_v2_project()).expect("must serialize");
    bytes[0] = b'X';
    let err = parse(&bytes).expect_err("must fail");
    assert!(matches!(err, Error::InvalidMagic(_)));
}

#[test]
fn v2_rejects_unsupported_version() {
    let mut bytes = write(&sample_v2_project()).expect("must serialize");
    bytes[4] = 99;
    bytes[5] = 0;
    let err = parse(&bytes).expect_err("must fail");
    assert!(matches!(err, Error::UnsupportedVersion(99)));
}

#[test]
fn v2_rejects_unknown_target() {
    let mut project = sample_v2_project();
    project.q0rgs[0].layers[0].placements[0].target = Target::Asset(999);
    let err = validate(&project).expect_err("must fail");
    assert!(matches!(
        err,
        Error::Validation("placement references unknown asset_id")
    ));
}

#[test]
fn v2_rejects_every_non_finite_geometry_field() {
    type Mutator = fn(&mut ProjectV2, f32);
    let mutators: &[(&str, Mutator)] = &[
        ("vector anchor point must be finite", |project, value| {
            vector_asset_mut(project).paths[0].anchors[0].point.x = value;
        }),
        ("vector anchor point must be finite", |project, value| {
            vector_asset_mut(project).paths[0].anchors[0].point.y = value;
        }),
        ("vector anchor handle must be finite", |project, value| {
            vector_asset_mut(project).paths[0].anchors[0]
                .out_handle
                .as_mut()
                .expect("sample out handle")
                .x = value;
        }),
        ("vector anchor handle must be finite", |project, value| {
            vector_asset_mut(project).paths[0].anchors[1]
                .in_handle
                .as_mut()
                .expect("sample in handle")
                .y = value;
        }),
        ("stroke width must be finite", |project, value| {
            vector_asset_mut(project)
                .stroke
                .as_mut()
                .expect("sample stroke")
                .width = value;
        }),
        ("placement translation must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.tx = value;
        }),
        ("placement translation must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.ty = value;
        }),
        ("placement scale must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.sx = value;
        }),
        ("placement scale must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.sy = value;
        }),
        ("placement rotation must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.rotation = value;
        }),
        ("placement skew must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.skew_x = value;
        }),
        ("placement skew must be finite", |project, value| {
            project.q0rgs[0].layers[0].placements[0].transform.skew_y = value;
        }),
    ];

    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for &(expected, mutate) in mutators {
            let mut project = sample_v2_project();
            mutate(&mut project, value);
            assert_eq!(
                validate(&project).expect_err("non-finite geometry must fail"),
                Error::Validation(expected)
            );
            assert_eq!(
                write(&project).expect_err("writer must reject non-finite geometry"),
                Error::Validation(expected)
            );
        }
    }
}

#[test]
fn v2_enforces_renderer_nesting_limit() {
    let at_limit = q0rg_chain(usize::from(MAX_Q0RG_NESTING_DEPTH));
    validate(&at_limit).expect("renderer depth limit must remain valid");

    let over_limit = q0rg_chain(usize::from(MAX_Q0RG_NESTING_DEPTH) + 1);
    assert_eq!(
        validate(&over_limit).expect_err("deeper content would be dropped by renderers"),
        Error::Validation("q0rg nesting depth exceeds 8")
    );
}

#[test]
fn v2_rejects_hostile_long_chain_without_recursive_dfs() {
    let project = q0rg_chain(4096);
    assert_eq!(
        validate(&project).expect_err("hostile chain must fail safely"),
        Error::Validation("q0rg nesting depth exceeds 8")
    );
}

#[test]
fn v2_rejects_nonzero_reserved_flags() {
    let mut bytes = write(&sample_v2_project()).expect("must serialize");
    bytes[6..8].copy_from_slice(&0x8001_u16.to_le_bytes());

    assert_eq!(
        parse(&bytes).expect_err("reserved flags must be zero"),
        Error::UnsupportedFlags(0x8001)
    );
}

#[test]
fn v2_rejects_trailing_bytes() {
    let mut bytes = write(&sample_v2_project()).expect("must serialize");
    let expected_offset = bytes.len();
    bytes.extend_from_slice(b"junk");

    assert_eq!(
        parse(&bytes).expect_err("trailing bytes must fail"),
        Error::TrailingBytes {
            offset: expected_offset,
            remaining: 4,
        }
    );
}

#[test]
fn eased_tween_round_trips_in_current_q1s() {
    let mut project = sample_v2_project();
    project.q0rgs[0].layers[0].placements[1].tween = Tween::Eased {
        to_frame: 5,
        easing: Easing::Preset {
            family: EasingFamily::Bounce,
            mode: EasingMode::InOut,
        },
    };
    let bytes = write(&project).expect("write eased project");
    let decoded = parse(&bytes).expect("parse eased project");
    assert_eq!(decoded, project);
}

#[test]
fn custom_cubic_easing_round_trips_and_is_not_linear() {
    let easing = Easing::CubicBezier {
        x1: 0.42,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
    };
    assert!(easing.is_valid());
    assert!(easing.sample(0.5) < 0.5);

    let mut project = sample_v2_project();
    project.q0rgs[0].layers[0].placements[1].tween = Tween::Eased {
        to_frame: 5,
        easing,
    };
    let decoded =
        parse(&write(&project).expect("write custom easing")).expect("parse custom easing");
    assert_eq!(decoded, project);
}

#[test]
fn bounce_in_out_has_symmetric_endpoints() {
    let easing = Easing::Preset {
        family: EasingFamily::Bounce,
        mode: EasingMode::InOut,
    };
    assert_eq!(easing.sample(0.0), 0.0);
    assert_eq!(easing.sample(1.0), 1.0);
    assert!((easing.sample(0.25) + easing.sample(0.75) - 1.0).abs() < 1e-5);
}
