//! `.q0s` v2 Р Р†Р вЂљРІР‚Сњ vector player format.
//!
//! Same on-disk shape as the corresponding `.q1s` project body, but with
//! `Q0S\0` magic and independent player-format versions. Lets the editor ship
//! that q0player can play frame-by-frame using the shared software
//! rasteriser, with vector data and q0rg transforms intact Р Р†Р вЂљРІР‚Сњ no pre-baked
//! bitmaps, dramatically smaller than the snapshot-style v1 output and
//! resolution-independent on playback.
//!
//! v1 (legacy bitmap-only) is still parsed by `parse_q0s` so old movies
//! keep playing. Current v13 adds per-placement alpha, blend modes, blur,
//! glow and drop-shadow data; v12 and earlier decode those properties as identity.

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
pub const Q0S_VERSION_APPEARANCE_AFFINE: u16 = 10;
pub const Q0S_VERSION_LAYER_STATE: u16 = 11;
pub const Q0S_VERSION_NESTED_LAYER_FOLDERS: u16 = 12;
pub const Q0S_VERSION_PLACEMENT_FX: u16 = 13;
pub const Q0S_VERSION_RIGGING: u16 = 14;
pub const Q0S_VERSION_RIG_PRO: u16 = 15;
pub const Q0S_VERSION_RIG_DEFORMERS: u16 = 16;
pub const Q0S_VERSION_RIG_POSE_VARIANTS: u16 = 17;
pub const Q0S_VERSION_AUDIO_CLIP_FX: u16 = 18;
pub const Q0S_VERSION_AUDIO_TIMELINE_CLIPS: u16 = 19;
pub const Q0S_VERSION_PROJECT_RUNTIME: u16 = 20;
pub const Q0S_VERSION_CURRENT: u16 = Q0S_VERSION_PROJECT_RUNTIME;

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
        Q0S_VERSION_APPEARANCE_FRAGMENTS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_APPEARANCE_FRAGMENTS)
        }
        Q0S_VERSION_APPEARANCE_AFFINE => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_APPEARANCE_AFFINE)
        }
        Q0S_VERSION_LAYER_STATE => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_LAYER_STATE),
        Q0S_VERSION_NESTED_LAYER_FOLDERS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_NESTED_LAYER_FOLDERS)
        }
        Q0S_VERSION_PLACEMENT_FX => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_PLACEMENT_FX)
        }
        Q0S_VERSION_RIGGING => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_RIGGING),
        Q0S_VERSION_RIG_PRO => v2::parse_body_after_header(bytes, v2::Q1S_VERSION_RIG_PRO),
        Q0S_VERSION_RIG_DEFORMERS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_RIG_DEFORMERS)
        }
        Q0S_VERSION_RIG_POSE_VARIANTS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_RIG_POSE_VARIANTS)
        }
        Q0S_VERSION_AUDIO_CLIP_FX => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_AUDIO_CLIP_FX)
        }
        Q0S_VERSION_AUDIO_TIMELINE_CLIPS => {
            v2::parse_body_after_header(bytes, v2::Q1S_VERSION_AUDIO_TIMELINE_CLIPS)
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
            | Q0S_VERSION_APPEARANCE_FRAGMENTS
            | Q0S_VERSION_APPEARANCE_AFFINE
            | Q0S_VERSION_LAYER_STATE
            | Q0S_VERSION_NESTED_LAYER_FOLDERS
            | Q0S_VERSION_PLACEMENT_FX
            | Q0S_VERSION_RIGGING
            | Q0S_VERSION_RIG_PRO
            | Q0S_VERSION_RIG_DEFORMERS
            | Q0S_VERSION_RIG_POSE_VARIANTS
            | Q0S_VERSION_AUDIO_CLIP_FX
            | Q0S_VERSION_AUDIO_TIMELINE_CLIPS
            | Q0S_VERSION_CURRENT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{
        Anchor, Asset, Easing, EasingFamily, EasingMode, Layer, Path as VPath, Placement,
        ProjectMeta, Q0rg, Rgba, RigAsset, RigChannel, RigControl, RigControlKind, RigKey, RigNode,
        RigPropertyRef, Target, Transform2D, Tween, Vec2, VectorAsset,
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

    fn test_audio_q0v_bytes() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 4_800;
        let spec = q0video::q0v::Q0vSpec {
            width: 0,
            height: 0,
            fps: 48_000,
            timeline_frames: SAMPLE_FRAMES as u32,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(std::io::Cursor::new(Vec::new()), spec)
            .expect("audio q0v writer");
        writer
            .write_audio_pcm_i16(&vec![0; SAMPLE_FRAMES * 2])
            .expect("audio q0v pcm");
        writer.finish().expect("finish audio q0v").into_inner()
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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
            .insert(1, "player vector / РіРµСЂРѕР№".to_string());
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
    fn current_q0s_roundtrip_preserves_placement_fx() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].placements[0].fx = v2::PlacementFx {
            opacity: 0.55,
            blend_mode: v2::BlendMode::Multiply,
            blur: Some(v2::BlurFx { radius: 4.0 }),
            glow: Some(v2::GlowFx {
                color: v2::Rgba {
                    r: 250,
                    g: 40,
                    b: 20,
                    a: 200,
                },
                radius: 8.0,
                strength: 1.2,
            }),
            shadow: Some(v2::DropShadowFx {
                color: v2::Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 150,
                },
                blur_radius: 6.0,
                offset_x: 5.0,
                offset_y: 3.0,
                strength: 0.75,
            }),
            audio_gain: 1.0,
            audio_muted: false,
        };
        let bytes = write_q0s_v2(&project).expect("write placement fx q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(
            parse_q0s_v2(&bytes).expect("parse placement fx q0s"),
            project
        );
    }

    #[test]
    fn current_q0s_roundtrip_preserves_audio_clip_gain_and_mute() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].placements[0].fx.audio_gain = 0.625;
        project.q0rgs[0].layers[0].placements[0].fx.audio_muted = true;
        let bytes = write_q0s_v2(&project).expect("write audio clip q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse audio clip q0s"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_timeline_audio_clip() {
        let mut project = small_project();
        project.assets = vec![Asset::Q0v(v2::Q0vAsset {
            asset_id: 77,
            bytes: test_audio_q0v_bytes(),
        })];
        project.q0rgs[0].frame_count = 20;
        project.q0rgs[0].layers[0].placements.clear();
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.audio_clips.push(v2::AudioClip {
            q0rg_id: 1,
            layer_id: 1,
            start_frame: 4,
            asset_id: 77,
            gain: 0.625,
            muted: true,
        });

        let bytes = write_q0s_v2(&project).expect("write timeline audio q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        let parsed = parse_q0s_v2(&bytes).expect("parse timeline audio q0s");
        assert_eq!(parsed, project);
        assert_eq!(parsed.audio_clips, project.audio_clips);
    }

    #[test]
    fn q0s_v17_remains_readable_with_default_audio_clip_fx() {
        let project = small_project();
        let mut bytes = v2::write_version(&project, v2::Q1S_VERSION_RIG_POSE_VARIANTS)
            .expect("write q1s v18 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_RIG_POSE_VARIANTS.to_le_bytes());
        let parsed = parse_q0s_v2(&bytes).expect("parse q0s v17");
        assert_eq!(parsed, project);
        let fx = parsed.q0rgs[0].layers[0].placements[0].fx;
        assert_eq!(fx.audio_gain, 1.0);
        assert!(!fx.audio_muted);
    }

    #[test]
    fn q0s_v12_nested_folders_remain_readable_with_identity_fx() {
        let project = small_project();
        let mut bytes = v2::write_version(&project, v2::Q1S_VERSION_NESTED_LAYER_FOLDERS)
            .expect("write q1s v13 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_NESTED_LAYER_FOLDERS.to_le_bytes());
        let decoded = parse_q0s_v2(&bytes).expect("parse q0s v12");
        assert_eq!(decoded, project);
        assert!(decoded.q0rgs[0].layers[0].placements[0].fx.is_identity());
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
            instance_id: 0,
            frame: 4,
            target: Target::Asset(1),
            transform: v2::Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
                field_transform: crate::transform::Affine::IDENTITY,
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
                field_transform: crate::transform::Affine::IDENTITY,
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
                field_transform: crate::transform::Affine::IDENTITY,
            },
        );
        let bytes = write_q0s_v2(&project).expect("write q0s fragments");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s fragments"), project);
    }

    #[test]
    fn current_player_parser_still_reads_q0s_v9_fragment_body_with_identity_field() {
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
                field_transform: crate::transform::Affine::IDENTITY,
            },
        );
        let mut bytes = v2::write_version(&project, v2::Q1S_VERSION_APPEARANCE_FRAGMENTS)
            .expect("write q1s v10 fragment body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_APPEARANCE_FRAGMENTS.to_le_bytes());
        let parsed = parse_q0s_v2(&bytes).expect("parse q0s v9 fragments");
        assert_eq!(parsed, project);
        assert_eq!(
            parsed.asset_appearances[&1].field_transform,
            crate::transform::Affine::IDENTITY
        );
    }

    #[test]
    fn current_q0s_roundtrip_preserves_appearance_field_affine() {
        let mut project = small_project();
        let source = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let field_transform = crate::transform::Affine {
            a11: 0.8,
            a12: 0.45,
            a21: -0.2,
            a22: 1.1,
            tx: 17.0,
            ty: -9.0,
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
                field_transform,
            },
        );
        let bytes = write_q0s_v2(&project).expect("write affine q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        let parsed = parse_q0s_v2(&bytes).expect("parse affine q0s");
        assert_eq!(
            parsed.asset_appearances[&1].field_transform,
            field_transform
        );
        assert_eq!(parsed, project);
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
                hidden: false,
                locked: false,
            },
        );
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 9),
            v2::LayerMetadata {
                kind: v2::LayerKind::Folder,
                parent_folder_id: None,
                collapsed: true,
                hidden: false,
                locked: false,
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
    fn current_q0s_roundtrip_preserves_nested_layer_folders() {
        let mut project = small_project();
        project.q0rgs[0].layers.push(Layer {
            layer_id: 8,
            name: "inner".into(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.q0rgs[0].layers.push(Layer {
            layer_id: 9,
            name: "outer".into(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        });
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 1),
            v2::LayerMetadata {
                parent_folder_id: Some(8),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 8),
            v2::LayerMetadata {
                kind: v2::LayerKind::Folder,
                parent_folder_id: Some(9),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 9),
            v2::LayerMetadata {
                kind: v2::LayerKind::Folder,
                ..Default::default()
            },
        );
        v2::validate(&project).expect("nested q0s fixture");

        let bytes = write_q0s_v2(&project).expect("write nested q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse nested q0s"), project);
    }

    #[test]
    fn q0s_v11_layer_state_remains_readable() {
        let mut project = small_project();
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 1),
            v2::LayerMetadata {
                hidden: true,
                locked: true,
                ..Default::default()
            },
        );
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_LAYER_STATE).expect("write q1s v12 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_LAYER_STATE.to_le_bytes());
        let parsed = parse_q0s_v2(&bytes).expect("parse q0s v11");
        assert_eq!(parsed, project);
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
        // Patch magic to legacy Q1S Р Р†Р вЂљРІР‚Сњ should no longer be flagged.
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
    #[test]
    fn current_q0s_roundtrip_preserves_layer_visibility_and_lock() {
        let mut project = small_project();
        project.layer_metadata.insert(
            v2::LayerKey::new(1, 1),
            v2::LayerMetadata {
                hidden: true,
                locked: true,
                ..Default::default()
            },
        );

        let bytes = write_q0s_v2(&project).expect("write layer-state q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        let parsed = parse_q0s_v2(&bytes).expect("parse layer-state q0s");
        assert_eq!(parsed, project);
        assert!(!parsed.layer_is_visible(1, 1));
        assert!(parsed.layer_is_locked(1, 1));
    }

    #[test]
    fn q0s_v10_remains_readable_with_visible_unlocked_defaults() {
        let project = small_project();
        let mut bytes = v2::write_version(&project, v2::Q1S_VERSION_APPEARANCE_AFFINE)
            .expect("write q1s v11 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_APPEARANCE_AFFINE.to_le_bytes());

        let parsed = parse_q0s_v2(&bytes).expect("parse q0s v10");
        assert_eq!(parsed, project);
        assert!(parsed.layer_is_visible(1, 1));
        assert!(!parsed.layer_is_locked(1, 1));
    }
    fn add_player_test_rig(project: &mut ProjectV2) {
        project.assets.push(Asset::Rig(RigAsset {
            asset_id: 500,
            owner_q0rg_id: 1,
            nodes: vec![RigNode {
                node_id: 1,
                name: "root".into(),
                parent: None,
                rest: Transform2D::IDENTITY,
                length: 12.0,
                binding: None,
            }],
            controls: vec![RigControl {
                control_id: 1,
                name: "turn".into(),
                kind: RigControlKind::Rotation,
                target_node: Some(1),
                rest_x: 0.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -3.0,
                max_value: 3.0,
                public_in_simple: true,
            }],
            constraints: Vec::new(),
            channels: vec![RigChannel {
                property: RigPropertyRef::ControlValue(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 0.5,
                    easing: Easing::Linear,
                }],
            }],
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            pose_drivers: Vec::new(),
            mirror_pairs: Vec::new(),
            variants: Vec::new(),
        }));
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("player test rig");
        rig.controls.push(RigControl {
            control_id: 2,
            name: "master".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.4,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.drivers.push(v2::RigDriver {
            driver_id: 1,
            source_control: 2,
            source_min: 0.0,
            source_max: 1.0,
            target: RigPropertyRef::NodeRotation(1),
            target_min: -0.25,
            target_max: 0.5,
        });
        rig.poses.push(v2::RigPosePreset {
            pose_id: 1,
            name: "player pose".into(),
            values: vec![v2::RigPoseValue {
                property: RigPropertyRef::ControlValue(2),
                value: 0.9,
            }],
        });
    }

    #[test]
    fn current_q0s_roundtrip_preserves_rigging() {
        let mut project = small_project();
        add_player_test_rig(&mut project);
        let bytes = write_q0s_v2(&project).expect("write rigged q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse rigged q0s"), project);
    }

    #[test]
    fn q0s_v13_placement_fx_body_remains_readable_without_rigs() {
        let project = small_project();
        let mut bytes =
            v2::write_version(&project, v2::Q1S_VERSION_PLACEMENT_FX).expect("write q1s v14 body");
        bytes[0..4].copy_from_slice(&Q0S_V2_MAGIC);
        bytes[4..6].copy_from_slice(&Q0S_VERSION_PLACEMENT_FX.to_le_bytes());
        assert_eq!(parse_q0s_v2(&bytes).expect("parse q0s v13"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_nonempty_rig_deformer() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].placements[0].instance_id = 2;
        add_player_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .expect("rig");
        for (control_id, x, y) in [(3, 0.0, 0.0), (4, 5.0, 4.0), (5, 10.0, 0.0)] {
            rig.controls.push(RigControl {
                control_id,
                name: format!("bend {control_id}"),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: x,
                rest_y: y,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            });
        }
        rig.deformers.push(v2::RigDeformer::Bend {
            deformer_id: 1,
            instance_id: 2,
            asset_id: 1,
            bind_transform: crate::transform::Affine::IDENTITY,
            axis_start: Vec2::new(0.0, 0.0),
            axis_end: Vec2::new(10.0, 0.0),
            start_control: 3,
            middle_control: 4,
            end_control: 5,
        });
        let bytes = write_q0s_v2(&project).expect("write deformer q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse deformer q0s"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_pose_driver_and_variant() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].placements[0].instance_id = 1;
        add_player_test_rig(&mut project);
        let rig = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Rig(rig) => Some(rig),
                _ => None,
            })
            .unwrap();
        rig.controls.push(RigControl {
            control_id: 3,
            name: "pose source".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.pose_drivers.push(v2::RigPoseDriver {
            driver_id: 1,
            source_control: 3,
            pose_id: 1,
            source_min: 0.0,
            source_max: 1.0,
            weight_min: 0.0,
            weight_max: 1.0,
            mode: v2::RigPoseBlendMode::Override,
        });
        rig.variants.push(v2::RigVariantSet {
            variant_id: 1,
            name: "variant".into(),
            instance_id: 1,
            source_control: 3,
            choices: vec![v2::RigVariantChoice {
                name: "base".into(),
                target: Target::Asset(1),
            }],
        });
        let bytes = write_q0s_v2(&project).expect("write phase h q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_CURRENT
        );
        assert_eq!(parse_q0s_v2(&bytes).expect("parse phase h q0s"), project);
    }

    #[test]
    fn current_q0s_roundtrip_preserves_project_runtime_metadata() {
        let mut project = small_project();
        project.q0rgs[0].layers[0].placements[0].instance_id = 77;
        project
            .runtime
            .project_graph
            .nodes
            .push(v2::ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "logic".into(),
                kind: v2::ProjectDependencyKind::Q0lang,
                source: v2::ProjectDependencySource::Embedded(b"gostop! 0\n".to_vec()),
            });
        project.runtime.frame_scripts.push(v2::FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 1,
            source: "gorun! 2\n".into(),
        });
        project
            .runtime
            .instance_names
            .insert(v2::InstanceKey::new(1, 77), "actor".into());

        let bytes = write_q0s_v2(&project).expect("write project runtime q0s");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            Q0S_VERSION_PROJECT_RUNTIME
        );
        assert_eq!(
            parse_q0s_v2(&bytes).expect("parse project runtime q0s"),
            project
        );
    }
}
