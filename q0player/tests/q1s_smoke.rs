use q0player::Player;
use q0s_format::{parse_q1s, q1s_to_q0s_bytes};

#[test]
fn smoke_q1s_to_q0s_to_player() {
    let q1s_bytes = include_bytes!("../../q0s-format/testdata/one_scene.q1s");
    let project = parse_q1s(q1s_bytes).expect("q1s must parse");
    let q0s_bytes = q1s_to_q0s_bytes(&project).expect("q1s must export to q0s");

    let mut player = Player::from_bytes(&q0s_bytes).expect("player should load exported q0s");
    player.set_playing(false);

    let width = 64_u32;
    let height = 64_u32;
    let mut frame = vec![0_u8; (width * height * 4) as usize];
    player.render(&mut frame, width, height);
    let idx = ((24 * width + 32) * 4) as usize;
    assert_eq!(&frame[idx..idx + 4], &[255, 0, 0, 255]);
}
