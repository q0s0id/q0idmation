use q0s_format::{
    parse_q1s, q1s_to_q0s_bytes, validate_q1s, write_q1s, Error, Q1AssetBitmap, Q1Layer,
    Q1Placement, Q1Project, Q1ProjectMeta, Q1Scene,
};

fn sample_project() -> Q1Project {
    Q1Project {
        meta: Q1ProjectMeta {
            name: "demo".to_string(),
            fps: 24,
            stage_width: 320,
            stage_height: 180,
            entry_scene_id: 1,
        },
        assets: vec![Q1AssetBitmap {
            asset_id: 1,
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        }],
        scenes: vec![Q1Scene {
            scene_id: 1,
            name: "Main".to_string(),
            frame_count: 2,
            layers: vec![Q1Layer {
                layer_id: 1,
                name: "Layer1".to_string(),
                placements: vec![Q1Placement {
                    frame: 0,
                    asset_id: 1,
                    x: 32,
                    y: 24,
                    scale_x: 1.0,
                    scale_y: 1.0,
                }],
            }],
        }],
    }
}

#[test]
fn roundtrip_q1s_binary() {
    let project = sample_project();
    let bytes = write_q1s(&project).expect("must serialize");
    let parsed = parse_q1s(&bytes).expect("must parse");
    assert_eq!(project, parsed);
}

#[test]
fn rejects_duplicate_asset_ids() {
    let mut project = sample_project();
    project.assets.push(project.assets[0].clone());
    let err = validate_q1s(&project).expect_err("must fail");
    assert!(matches!(err, Error::Validation("asset_id must be unique")));
}

#[test]
fn exports_q1s_to_q0s() {
    let project = sample_project();
    let q0s = q1s_to_q0s_bytes(&project).expect("must export");
    assert!(q0s.starts_with(b"Q0S\0"));
}

#[test]
fn parses_golden_q1s_files() {
    let empty = include_bytes!("../testdata/empty.q1s");
    let one_scene = include_bytes!("../testdata/one_scene.q1s");
    parse_q1s(empty).expect("empty project must parse");
    parse_q1s(one_scene).expect("one_scene project must parse");
}

#[test]
fn fails_on_corrupted_q1s() {
    let corrupted = include_bytes!("../testdata/corrupted.q1s");
    let err = parse_q1s(corrupted).expect_err("corrupted file must fail");
    assert!(matches!(
        err,
        Error::InvalidMagic(_) | Error::UnexpectedEof { .. }
    ));
}

#[test]
fn rejects_non_finite_q1s_scales() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut project = sample_project();
        project.scenes[0].layers[0].placements[0].scale_x = value;
        assert!(matches!(
            validate_q1s(&project),
            Err(Error::Validation("placement scale must be finite"))
        ));
        assert!(matches!(
            write_q1s(&project),
            Err(Error::Validation("placement scale must be finite"))
        ));
    }
}

#[test]
fn rejects_nonzero_reserved_flags() {
    let mut bytes = write_q1s(&sample_project()).expect("must serialize");
    bytes[6..8].copy_from_slice(&0x0001_u16.to_le_bytes());

    assert_eq!(
        parse_q1s(&bytes).expect_err("reserved flags must be zero"),
        Error::UnsupportedFlags(1)
    );
}

#[test]
fn rejects_trailing_bytes() {
    let mut bytes = write_q1s(&sample_project()).expect("must serialize");
    let expected_offset = bytes.len();
    bytes.extend_from_slice(b"junk");

    assert_eq!(
        parse_q1s(&bytes).expect_err("trailing bytes must fail"),
        Error::TrailingBytes {
            offset: expected_offset,
            remaining: 4,
        }
    );
}
