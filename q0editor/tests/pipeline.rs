use std::fs;

use q0editor::file_io::{load_project, save_project};
use q0s_format::v2::Asset;

#[test]
fn open_v1_golden_migrates_to_v2() {
    let tmp_dir = std::env::temp_dir().join("q0editor-tests");
    fs::create_dir_all(&tmp_dir).expect("tmp dir");
    let v1_path = tmp_dir.join("one_scene_copy.q1s");
    fs::write(
        &v1_path,
        include_bytes!("../../q0s-format/testdata/one_scene.q1s"),
    )
    .expect("seed v1 file");

    let project = load_project(&v1_path).expect("v1 must load via migration");
    assert!(!project.q0rgs.is_empty());
    assert!(project.assets.iter().all(|a| matches!(a, Asset::Bitmap(_))));
}

#[test]
fn save_then_load_roundtrips_default_project() {
    use q0editor::state::default_project;
    let tmp_dir = std::env::temp_dir().join("q0editor-tests");
    fs::create_dir_all(&tmp_dir).expect("tmp dir");
    let path = tmp_dir.join("default_roundtrip.q1s");

    let original = default_project();
    save_project(&path, &original).expect("save");
    let loaded = load_project(&path).expect("load");
    assert_eq!(loaded, original);
}
