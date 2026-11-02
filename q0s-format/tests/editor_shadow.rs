use q0s_format::raster::{rasterize_placement_fx_tile, rasterize_q0rg_frame_scaled};
use q0s_format::transform::Affine;
use q0s_format::v2::*;

fn fixture(quality: u8) -> ProjectV2 {
    let shadow = Placement {
        instance_id: 1,
        pivot: None,
        frame: 0,
        target: Target::Asset(1),
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
        fx: PlacementFx {
            shadow: Some(DropShadowFx {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 220,
                },
                blur_radius: 40.0,
                offset_x: -12.0,
                offset_y: 8.0,
                strength: 1.0,
                quality,
            }),
            ..Default::default()
        },
    };
    let instance = Placement {
        target: Target::Q0rg(2),
        instance_id: 2,
        transform: Transform2D {
            tx: 150.0,
            ty: 100.0,
            ..Transform2D::IDENTITY
        },
        fx: PlacementFx::default(),
        ..shadow.clone()
    };
    let layer = |placements| Layer {
        layer_id: 1,
        timeline_frame_count: None,
        name: "art".into(),
        explicit_keyframes: Vec::new(),
        placements,
    };
    ProjectV2 {
        meta: ProjectMeta {
            name: "nested shadow".into(),
            fps: 24,
            stage_width: 320,
            stage_height: 240,
            background_color: Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets: vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![Path {
                closed: true,
                anchors: [(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0)]
                    .into_iter()
                    .map(|(x, y)| Anchor {
                        point: Vec2::new(x, y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
            }],
            fill: Some(Rgba {
                r: 220,
                g: 50,
                b: 30,
                a: 255,
            }),
            stroke: None,
        })],
        q0rgs: vec![
            Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![layer(vec![instance])],
            },
            Q0rg {
                q0rg_id: 2,
                name: "symbol".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![layer(vec![shadow])],
            },
        ],
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: Vec::new(),
        runtime: Default::default(),
    }
}

#[test]
fn regression_shadow_tile_is_complete_at_negative_symbol_coordinates() {
    for quality in [1, 3, 15] {
        let project = fixture(quality);
        let placement = &project.q0rgs[1].layers[0].placements[0];
        for outer_only in [false, true] {
            let local = rasterize_placement_fx_tile(
                &project,
                2,
                placement,
                0,
                Affine::IDENTITY,
                placement.fx,
                320,
                240,
                outer_only,
            )
            .unwrap();
            assert!(
                local.x < -20 && local.y < -20,
                "the shadow was clipped at the symbol origin"
            );
            let shifted = rasterize_placement_fx_tile(
                &project,
                2,
                placement,
                0,
                Affine {
                    tx: 150.0,
                    ty: 100.0,
                    ..Affine::IDENTITY
                },
                placement.fx,
                320,
                240,
                outer_only,
            )
            .unwrap();
            assert_eq!((shifted.x, shifted.y), (local.x + 150, local.y + 100));
            assert_eq!((shifted.width, shifted.height), (local.width, local.height));
            assert_eq!(
                shifted.rgba, local.rgba,
                "entering or leaving a symbol must not change shadow pixels at quality {quality}"
            );
        }
    }
}

#[test]
fn regression_nested_shadow_matches_the_full_scene_and_quality_changes_the_filter() {
    let project = fixture(15);
    let placement = &project.q0rgs[1].layers[0].placements[0];
    let tile = rasterize_placement_fx_tile(
        &project,
        2,
        placement,
        0,
        Affine {
            tx: 150.0,
            ty: 100.0,
            ..Affine::IDENTITY
        },
        placement.fx,
        320,
        240,
        false,
    )
    .unwrap();
    let mut reconstructed = vec![0u8; 320 * 240 * 4];
    for y in 0..tile.height as usize {
        let start = ((tile.y as usize + y) * 320 + tile.x as usize) * 4;
        let source = y * tile.width as usize * 4;
        reconstructed[start..start + tile.width as usize * 4]
            .copy_from_slice(&tile.rgba[source..source + tile.width as usize * 4]);
    }
    let full = rasterize_q0rg_frame_scaled(&project, 1, 0, 320, 240, 1, [0, 0, 0, 0]);
    assert_eq!(
        full, reconstructed,
        "the local tile and nested scene disagree"
    );
    let low = rasterize_q0rg_frame_scaled(&fixture(1), 1, 0, 320, 240, 1, [0, 0, 0, 0]);
    assert_ne!(full, low, "shadow quality must affect the rendered filter");
}

#[test]
fn regression_shadow_quality_roundtrips_in_q1s_and_packed_q0s() {
    for quality in [1, 3, 15] {
        let project = fixture(quality);
        let bytes = write(&project).unwrap();
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if quality == 3 {
            assert!(version < Q1S_VERSION_SHADOW_QUALITY);
        } else {
            assert_eq!(version, Q1S_VERSION_SHADOW_QUALITY);
        }
        let decoded = parse(&bytes).unwrap();
        assert_eq!(
            decoded.q0rgs[1].layers[0].placements[0].fx,
            project.q0rgs[1].layers[0].placements[0].fx
        );
        let player = q0s_format::q0s_v2::write_q0s_v2(&project).unwrap();
        let decoded = q0s_format::q0s_v2::parse_q0s_v2(&player).unwrap();
        assert_eq!(
            decoded.q0rgs[1].layers[0].placements[0].fx,
            project.q0rgs[1].layers[0].placements[0].fx
        );
    }
    assert!(validate(&fixture(0)).is_err());
    assert!(validate(&fixture(16)).is_err());
}
