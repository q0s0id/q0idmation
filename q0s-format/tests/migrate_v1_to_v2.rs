use q0s_format::v2::{parse as parse_v2, write as write_v2, Asset, Target};
use q0s_format::{migrate_v1_to_v2, parse_q1s};

#[test]
fn migrate_one_scene_v1_to_v2_preserves_data() {
    let v1_bytes = include_bytes!("../testdata/one_scene.q1s");
    let v1 = parse_q1s(v1_bytes).expect("v1 must parse");

    let v2 = migrate_v1_to_v2(v1.clone()).expect("migration must succeed");

    assert_eq!(v2.meta.name, v1.meta.name);
    assert_eq!(v2.meta.fps, v1.meta.fps);
    assert_eq!(v2.meta.stage_width, v1.meta.stage_width);
    assert_eq!(v2.meta.stage_height, v1.meta.stage_height);
    assert_eq!(v2.meta.entry_q0rg_id, v1.meta.entry_scene_id);

    assert_eq!(v2.assets.len(), v1.assets.len());
    for (a, expected) in v2.assets.iter().zip(v1.assets.iter()) {
        match a {
            Asset::Bitmap(b) => {
                assert_eq!(b.asset_id, expected.asset_id);
                assert_eq!(b.width, expected.width);
                assert_eq!(b.height, expected.height);
                assert_eq!(b.rgba, expected.rgba);
            }
            Asset::Vector(_) | Asset::Q0v(_) => {
                panic!("v1 assets must migrate as bitmap")
            }
        }
    }

    assert_eq!(v2.q0rgs.len(), v1.scenes.len());
    let v2_scene = &v2.q0rgs[0];
    let v1_scene = &v1.scenes[0];
    assert_eq!(v2_scene.q0rg_id, v1_scene.scene_id);
    assert_eq!(v2_scene.frame_count, v1_scene.frame_count);
    assert_eq!(v2_scene.script, "");
    assert_eq!(v2_scene.layers.len(), v1_scene.layers.len());

    let v2_p = &v2_scene.layers[0].placements[0];
    let v1_p = &v1_scene.layers[0].placements[0];
    assert_eq!(v2_p.frame, v1_p.frame);
    match v2_p.target {
        Target::Asset(id) => assert_eq!(id, v1_p.asset_id),
        Target::Q0rg(_) => panic!("v1 placements must target Asset"),
    }
    assert_eq!(v2_p.transform.tx, f32::from(v1_p.x));
    assert_eq!(v2_p.transform.ty, f32::from(v1_p.y));
    assert_eq!(v2_p.transform.sx, v1_p.scale_x);
    assert_eq!(v2_p.transform.sy, v1_p.scale_y);
    assert_eq!(v2_p.transform.rotation, 0.0);
}

#[test]
fn migrated_v2_roundtrips_through_binary() {
    let v1_bytes = include_bytes!("../testdata/one_scene.q1s");
    let v1 = parse_q1s(v1_bytes).expect("v1 must parse");
    let v2 = migrate_v1_to_v2(v1).expect("migration must succeed");

    let bytes = write_v2(&v2).expect("v2 must serialize");
    let parsed = parse_v2(&bytes).expect("v2 must parse");
    assert_eq!(v2, parsed);
}

#[test]
fn migrate_empty_v1_to_v2() {
    let v1_bytes = include_bytes!("../testdata/empty.q1s");
    let v1 = parse_q1s(v1_bytes).expect("v1 must parse");
    let v2 = migrate_v1_to_v2(v1).expect("migration must succeed");
    assert!(!v2.q0rgs.is_empty());
}
