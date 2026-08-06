use q0s_format::{parse_q0s, write_q0s, Error};

#[test]
fn parses_golden_file() {
    let bytes = include_bytes!("../testdata/one_sprite.q0s");
    let movie = parse_q0s(bytes).expect("golden file must parse");

    assert_eq!(movie.header.version, 1);
    assert_eq!(movie.header.fps, 24);
    assert_eq!(movie.header.frame_count, 2);
    assert_eq!(
        (movie.background.r, movie.background.g, movie.background.b),
        (20, 26, 38)
    );

    let bmp = movie.bitmaps.get(&1).expect("bitmap with id=1 must exist");
    assert_eq!((bmp.width, bmp.height), (2, 2));
    assert_eq!(bmp.rgba.len(), 16);

    assert_eq!(movie.placements_by_frame[0].len(), 1);
    let p = &movie.placements_by_frame[0][0];
    assert_eq!(p.bitmap_id, 1);
    assert_eq!((p.x, p.y), (32, 24));
    assert_eq!((p.scale_x, p.scale_y), (1.0, 1.0));
}

#[test]
fn returns_error_for_truncated_data() {
    let bytes = *b"Q0S";
    let err = parse_q0s(&bytes).expect_err("must fail");
    match err {
        Error::UnexpectedEof { .. } => {}
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn returns_error_for_invalid_tag() {
    let mut bytes = include_bytes!("../testdata/one_sprite.q0s").to_vec();
    bytes[12] = 0xFF;
    let err = parse_q0s(&bytes).expect_err("must fail");
    match err {
        Error::InvalidTag(0xFF) => {}
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn rejects_trailing_bytes() {
    let mut bytes = include_bytes!("../testdata/one_sprite.q0s").to_vec();
    let expected_offset = bytes.len();
    bytes.extend_from_slice(b"junk");

    let err = parse_q0s(&bytes).expect_err("trailing bytes must fail");
    assert_eq!(
        err,
        Error::TrailingBytes {
            offset: expected_offset,
            remaining: 4,
        }
    );
}

#[test]
fn rejects_non_finite_legacy_scales() {
    let valid = include_bytes!("../testdata/one_sprite.q0s");
    let scale_x_offset = valid.len() - 8;

    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bytes = valid.to_vec();
        bytes[scale_x_offset..scale_x_offset + 4].copy_from_slice(&value.to_le_bytes());
        let err = parse_q0s(&bytes).expect_err("non-finite scale must fail");
        assert!(matches!(
            err,
            Error::InvalidRecord {
                reason: "scale must be finite",
                ..
            }
        ));
    }

    let mut movie = parse_q0s(valid).expect("fixture must parse");
    movie.placements_by_frame[0][0].scale_y = f32::INFINITY;
    assert!(matches!(
        write_q0s(&movie),
        Err(Error::Validation("placement scale must be finite"))
    ));
}
