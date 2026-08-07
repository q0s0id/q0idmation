//! `.q0s` v2 РІР‚вЂќ vector player format.
//!
//! Same on-disk shape as the corresponding `.q1s` project body, but with
//! `Q0S\0` magic and independent player-format versions. Lets the editor ship
//! that q0player can play frame-by-frame using the shared software
//! rasteriser, with vector data and q0rg transforms intact РІР‚вЂќ no pre-baked
//! bitmaps, dramatically smaller than the snapshot-style v1 output and
//! resolution-independent on playback.
//!
//! v1 (legacy bitmap-only) is still parsed by `parse_q0s` so old movies
//! keep playing.

use crate::error::Error;
#[cfg(test)]
use crate::v2::Q1S_V2_MAGIC;
use crate::v2::{self, ProjectV2};

pub const Q0S_V2_MAGIC: [u8; 4] = *b"Q0S\0";
/// Bumps player-format version to 2. v1 bytes are still recognised by
/// `parse_q0s` (bitmap-only `Movie`).
pub const Q0S_V2_VERSION: u16 = 2;
pub const Q0S_VERSION_KEYFRAMES: u16 = 3;
pub const Q0S_VERSION_ASSET_NAMES: u16 = 4;
pub const Q0S_VERSION_LAYER_FOLDERS: u16 = 5;
pub const Q0S_VERSION_Q0V_ASSETS: u16 = 6;
pub const Q0S_VERSION_EASING: u16 = 7;
pub const Q0S_VERSION_APPEARANCE_MASKS: u16 = 8;
pub const Q0S_VERSION_APPEARANCE_FRAGMENTS: u16 = 9;
pub const Q0S_VERSION_CURRENT: u16 = Q0S_VERSION_APPEARANCE_FRAGMENTS;

/// Serialise a `ProjectV2` as the current vector `.q0s` bytes. Internally we
/// reuse the current `.q1s` writer and patch the magic+version header in-place
/// so the player can tell `.q0s` apart from `.q1s`.
pub fn write_q0s_v2(project: &ProjectV2) -> Result<Vec<u8>, Error> {
    let mut bytes = v2::write(project)?;
    if bytes.len() < 6 {
        return Err(Error::Validation("v2 writer returned a short buffer"));
    }
    bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
    bytes[4..6].copy_from_slice(&Q0S_VERSION_CURRENT.to_le_bytes());
    Ok(bytes)
}

/// Parse `.q0s` v2 bytes into a `ProjectV2`. The body is shared with the
/// current q1s format and is parsed directly from this slice, without a
/// second full-file allocation.
pub fn parse_q0s_v2(bytes: &[u8]) -> Result<ProjectV2, Error> {
    if bytes.len() < 6 {
        return Err(Error::Validation("q0s v2 file is too short"));
    }
    if bytes[0..4] != Q0S_V2_MAGIC {
        let mut m = [0u8; 4];
        m.copy_from_slice(&bytes[0..4]);
        return Err(Error::InvalidMagic(m));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    match version {
        Q0S_V2_VERSION => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_SKEW),
        Q0S_VERSION_KEYFRAMES => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_KEYFRAMES),
        Q0S_VERSION_ASSET_NAMES => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_ASSET_NAMES),
        Q0S_VERSION_LAYER_FOLDERS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_LAYER_FOLDERS)
        }
        Q0S_VERSION_Q0V_ASSETS => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_Q0V_ASSETS),
        Q0S_VERSION_EASING => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_EASING),
        Q0S_VERSION_APPEARANCE_MASKS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_APPEARANCE_MASKS)
        }
        Q0S_VERSION_CURRENT => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_CURRENT),
        _ => Err(Error::UnsupportedVersion(version)),
    }
}

/// Quick-check: is this byte buffer a `.q0s` v2 file? Used by the player
/// when it first opens a file to decide which loader to call.
pub fn is_q0s_v2(bytes: &[u8]) -> bool {
    if bytes.len() < 6 {
        return false;
    }
    if bytes[0..4] != Q0S_V2_MAGIC {
        return false;
    }
    matches!(
        u16::from_le_bytes([bytes[4], bytes[5]]),
        Q0S_V2_VERSION
            | Q0S_VERSION_KEYFRAMES
            | Q0S_VERSION_ASSET_NAMES
            | Q0S_VERSION_LAYER_FOLDERS
            | Q0S_VERSION_Q0V_ASSETS
            | Q0S_VERSION_EASING
            | Q0S_VERSION_APPEARANCE_MASKS
            | Q0S_VERSION_CURRENT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{
        Anchor, Asset, Easing, EasingFamily, EasingMode, Layer, Path as VPath, Placement,
        ProjectMeta, Q0rg, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
    };

    fn test_q0v_bytes() -> Vec<u8> {
        let spec = q0video::q0v::Q0vSpec {
            width: 2,
            height: 2,
            fps: 24,
            timeline_frames: 1,
            video: true,
            audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(std::io::Cursor::new(Vec::new()), spec)
            .expect("q0v writer");
        writer.write_video_frame(0, b"frame").expect("q0v frame");
        writer.finish().expect("finish q0v").into_inner()
    }

    fn small_project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "t".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![VPath {
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(0.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(10.0, 0.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(10.0, 10.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 200,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 3,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "L".to_string(),
                    explicit_keyframes: vec![1],
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn q0s_v2_round_trip() {
        let mut project = small_project();
        project
            .asset_names
            .insert(1, "player vector / герой".to_string());
        let bytes = write_q0s_v2(&project).expect("write");
        assert_eq!(&bytes[0..4], &Q0S_V2_MAGIC);
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        let parsed = parse_q0s_v2(&bytes).expect("parse");
        assert_eq!(parsed, project);
    }

    #[test]
    fn current_q0s_roundtrip_accepts_noncanonical_editor_order() {
        let mut project = small_project();
        project.assets.push(Asset::Vector(v2::VectorAsset {
            asset_id: 2,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        project.assets.swap(0, 1);
        project.q0rgs[0].frame_count = 5;
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements.push(v2::Placement {
            frame: 4,
            target: Target::Asset(1),
            transform: v2::Transform2D::IDENTITY,
            tween: Tween::None,
        });
        layer.placements.swap(0, 1);
        layer.explicit_keyframes = vec![3, 2];

        let bytes = write_q0s_v2(&project).expect("write noncanonical q0s");
        let parsed = parse_q0s_v2(&bytes).expect("parse noncanonical q0s");
        assert_eq!(parsed, v2::canonicalized_for_wire(&project));
    }

    #[test]
    fn current_player_parser_still_reads_q0s_v6_q0v_assets() {
        let mut project = small_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(v2::Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project
            .asset_names
            .insert(7, "legacy q0s video".to_string());
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_Q0V_ASSETS).expect("write q1s v7 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_Q0V_ASSETS.to_le_bytes());

        assert!(is_q0s_v2(&bytes));
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s v6"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_easing_for_player() {
        let mut project = small_project();
        project.q0rgs[0].frame_count = 4;
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.q0rgs[0].layers[0].placements[0].tween = Tween::Eased {
            to_frame: 3,
            easing: Easing::Preset {
                family: EasingFamily::Bounce,
                mode: EasingMode::InOut,
            },
        };
        let mut target = project.q0rgs[0].layers[0].placements[0].clone();
        target.frame = 3;
        target.transform.tx = 90.0;
        target.tween = Tween::None;
        project.q0rgs[0].layers[0].placements.push(target);

        let bytes = write_q0s_v2(&project).expect("write eased q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse eased q0s"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_vector_appearance_and_mask() {
        let mut project = small_project();
        let mask = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(2.0, 2.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(6.0, 2.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(6.0, 6.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(2.0, 6.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        };
        project.asset_appearances.insert(
            1,
            v2::VectorAppearance {
                material: v2::VectorMaterial::SoftHalo {
                    radius: 8.0,
                    opacity: 0.5,
                },
                erase_mask: vec![mask],
                material_source: Vec::new(),
                clip_mask: Vec::new(),
            },
        );

        let bytes = write_q0s_v2(&project).expect("write appearance q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse appearance q0s"), project);
    }

    #[test]
    fn current_player_parser_still_reads_q0s_v8_appearance_body() {
        let mut project = small_project();
        project.asset_appearances.insert(
            1,
            v2::VectorAppearance {
                material: v2::VectorMaterial::SoftHalo {
                    radius: 5.0,
                    opacity: 0.4,
                },
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
            },
        );
        let mut bytes = v2::write_version(&project, v2::Q1S_VERSION_APPEARANCE_MASKS)
            .expect("write q1s v9 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_APPEARANCE_MASKS.to_le_bytes());
        assert!(is_q0s_v2(&bytes));
        assert_eq!(
            parse_q0s_v2(&bytes).expect("parse q0s v8 appearance"),
            project
        );
    }

    #[test]
    fn current_q0s_roundtrip_preserves_post_material_fragments() {
        let mut project = small_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        project.asset_appearances.insert(
            1,
            v2::VectorAppearance {
                material: v2::VectorMaterial::SoftHalo {
                    radius: 6.0,
                    opacity: 0.5,
                },
                erase_mask: Vec::new(),
                material_source: source.clone(),
                clip_mask: source,
            },
        );
        let bytes = write_q0s_v2(&project).expect("write q0s fragments");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_APPEARANCE_FRAGMENTS
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s fragments"), project);
    }

    #[test]
    fn current_player_parser_still_reads_q0s_v7_easing_body() {
        let project = small_project();
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_EASING).expect("write q1s v8 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_EASING.to_le_bytes());

        assert!(is_q0s_v2(&bytes));
        let parsed = parse_q0s_v2(&bytes).expect("parse q0s v7");
        assert_eq!(parsed, project);
        assert!(parsed.asset_appearances.is_empty());
    }

    #[test]
    fn current_player_parser_still_reads_vector_q0s_v2() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_SKEW).expect("write legacy body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_V2_VERSION.to_le_bytes());

        assert!(is_q0s_v2(&bytes));
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s v2"), project);
    }

    #[test]
    fn current_player_parser_still_reads_keyframe_q0s_v3() {
        let project = small_project();
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_KEYFRAMES).expect("write q0s v3 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_KEYFRAMES.to_le_bytes());

        assert!(is_q0s_v2(&bytes));
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s v3"), project);
    }

    #[test]
    fn current_player_parser_still_reads_asset_name_q0s_v4() {
        let mut project = small_project();
        project
            .asset_names
            .insert(1, "legacy named vector".to_string());
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_ASSET_NAMES).expect("write q0s v4 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_ASSET_NAMES.to_le_bytes());

        assert!(is_q0s_v2(&bytes));
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s v4"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_folder_metadata_and_depth_order() {
        let mut project = small_project();
        project.q0rgs[0].layers.push(Layer {
            layer_id: 9,
            name: "Folder".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 1),
            v2::LayerMetadata {
                kind: v2::LayerKind::Normal,
                parent_folder_id: Some(9),
                collapsed: false,
            },
        );
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 9),
            v2::LayerMetadata {
                kind: v2::LayerKind::Folder,
                parent_folder_id: None,
                collapsed: true,
            },
        );

        let bytes = write_q0s_v2(&project).expect("write folder q0s");
        let parsed = parse_q0s_v2(&bytes).expect("parse folder q0s");
        assert_eq!(parsed, project);
        assert_eq!(
            parsed.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, 9]
        );
    }

    #[test]
    fn current_q0s_roundtrip_preserves_embedded_q0v_asset() {
        let mut project = small_project();
        project.assets.clear();
        project.assets.push(Asset::Q0v(v2::Q0vAsset {
            asset_id: 7,
            bytes: test_q0v_bytes(),
        }));
        project.asset_names.insert(7, "embedded video".to_string());
        project.q0rgs[0].layers[0].placements[0].target = Target::Asset(7);

        let bytes = write_q0s_v2(&project).expect("write q0v q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0v q0s"), project);
    }

    #[test]
    fn is_q0s_v2_only_returns_true_for_correct_magic_and_version() {
        let project = small_project();
        let bytes = write_q0s_v2(&project).expect("write");
        assert!(is_q0s_v2(&bytes));
        // Patch magic to legacy Q1S РІР‚вЂќ should no longer be flagged.
        let mut tampered = bytes.clone();
        tampered[0..4].copy_from_slice(&Q1S_V2_MAGIC);
        assert!(!is_q0s_v2(&tampered));
    }

    #[test]
    fn q0s_v2_rejects_nonzero_reserved_flags() {
        let mut bytes = write_q0s_v2(&small_project()).expect("write");
        bytes[6..8].copy_from_slice(&7_u16.to_le_bytes());

        assert_eq!(
            parse_q0s_v2(&bytes).expect_err("reserved flags must be zero"),
            Error::UnsupportedFlags(7)
        );
    }

    #[test]
    fn q0s_v2_rejects_trailing_bytes() {
        let mut bytes = write_q0s_v2(&small_project()).expect("write");
        let expected_offset = bytes.len();
        bytes.extend_from_slice(b"junk");

        assert_eq!(
            parse_q0s_v2(&bytes).expect_err("trailing bytes must fail"),
            Error::TrailingBytes {
                offset: expected_offset,
                remaining: 4,
            }
        );
    }
}
