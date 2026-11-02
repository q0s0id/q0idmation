//! A small visual reference for WebGL mask intersection and additive particles.
use q0s_format::v2::*;

fn main() {
    let folder = std::path::PathBuf::from(std::env::args_os().nth(1).expect("output folder"));
    let rgba = |r, g, b, a| Rgba { r, g, b, a };
    let vector = |id, color, points: &[[f32; 2]]| {
        Asset::Vector(VectorAsset {
            asset_id: id,
            fill: Some(color),
            stroke: None,
            paths: vec![Path {
                closed: true,
                anchors: points
                    .iter()
                    .map(|p| Anchor {
                        point: Vec2::new(p[0], p[1]),
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
            }],
        })
    };
    let rect = [[0., 0.], [100., 0.], [100., 140.], [0., 140.]];
    let circle: Vec<_> = (0..64)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 64.;
            [a.cos() * 38., a.sin() * 38.]
        })
        .collect();
    let placement = |id, asset, x, y, fx| Placement {
        pivot: None,
        instance_id: id,
        frame: 0,
        target: Target::Asset(asset),
        transform: Transform2D {
            tx: x,
            ty: y,
            ..Transform2D::IDENTITY
        },
        tween: Tween::None,
        fx,
    };
    let layer = |id, name: &str, placements| Layer {
        layer_id: id,
        name: name.into(),
        timeline_frame_count: None,
        explicit_keyframes: vec![0],
        placements,
    };
    let mut project = ProjectV2 {
        meta: ProjectMeta {
            name: "mask and particle reference".into(),
            fps: 24,
            stage_width: 320,
            stage_height: 200,
            background_color: rgba(32, 48, 64, 255),
            entry_q0rg_id: 1,
        },
        assets: vec![
            vector(1, rgba(235, 55, 75, 255), &rect),
            vector(2, rgba(255, 255, 255, 128), &rect),
            vector(3, rgba(255, 255, 255, 255), &circle),
        ],
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "stage".into(),
            frame_count: 48,
            script: String::new(),
            layers: vec![
                layer(
                    1,
                    "clipped red",
                    vec![placement(1, 1, 20., 20., PlacementFx::default())],
                ),
                layer(
                    2,
                    "first half-alpha mask",
                    vec![placement(2, 2, 40., 40., PlacementFx::default())],
                ),
                layer(
                    3,
                    "second half-alpha mask",
                    vec![placement(3, 2, 60., 0., PlacementFx::default())],
                ),
                layer(
                    4,
                    "glowing particles",
                    vec![
                        placement(
                            4,
                            3,
                            205.,
                            95.,
                            PlacementFx {
                                blend_mode: BlendMode::Add,
                                opacity: 0.7,
                                tint: Some(TintFx {
                                    color: rgba(255, 80, 120, 255),
                                    amount: 1.0,
                                }),
                                ..Default::default()
                            },
                        ),
                        placement(
                            5,
                            3,
                            240.,
                            95.,
                            PlacementFx {
                                blend_mode: BlendMode::Add,
                                opacity: 0.7,
                                tint: Some(TintFx {
                                    color: rgba(60, 170, 255, 255),
                                    amount: 1.0,
                                }),
                                ..Default::default()
                            },
                        ),
                    ],
                ),
            ],
        }],
    };
    project.layer_metadata.insert(
        LayerKey::new(1, 1),
        LayerMetadata {
            mask_layer_id: Some(2),
            ..Default::default()
        },
    );
    project.layer_metadata.insert(
        LayerKey::new(1, 2),
        LayerMetadata {
            kind: LayerKind::Mask,
            mask_layer_id: Some(3),
            ..Default::default()
        },
    );
    project.layer_metadata.insert(
        LayerKey::new(1, 3),
        LayerMetadata {
            kind: LayerKind::Mask,
            ..Default::default()
        },
    );
    project.asset_appearances.insert(
        3,
        VectorAppearance {
            material: VectorMaterial::Solid,
            erase_mask: Vec::new(),
            clip_mask: Vec::new(),
            material_source: Vec::new(),
            field_transform: q0s_format::transform::Affine::IDENTITY,
            fill_gradient: Some(GradientFill {
                kind: GradientKind::Radial,
                transform: q0s_format::transform::Affine::from_uniform_scale(38.),
                stops: [(0., 255), (0.2, 230), (0.45, 140), (0.7, 50), (1., 0)]
                    .map(|(position, a)| GradientStop {
                        position,
                        color: rgba(255, 255, 255, a),
                    })
                    .to_vec(),
            }),
        },
    );
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("compositing.q0s"),
        q0s_format::q0s_v2::write_q0s_v2(&project).unwrap(),
    )
    .unwrap();
    let pixels =
        q0s_format::raster::rasterize_q0rg_frame(&project, 1, 0, 320, 200, 1, [32, 48, 64, 255]);
    image::save_buffer(
        folder.join("compositing-reference.png"),
        &pixels,
        320,
        200,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("mask and particle reference written");
}
