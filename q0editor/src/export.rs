//! Export a `ProjectV2` to the `.q0s` player format.
//!
//! The player format is a versioned wrapper:
//!   * **v1** вЂ” legacy bitmap-only `Movie` (one bitmap per frame). Big
//!     and pixelated; we no longer write it. Old files still play
//!     because q0player's loader recognises the v1 magic+version.
//!   * **v2** вЂ” full vector data: assets (vector paths + bitmaps), q0rg
//!     hierarchy, transforms, tweens. Same on-disk layout as `.q1s` but
//!     with `Q0S\0` magic. q0player rasterises each frame at playback
//!     time via the shared software rasteriser, so the file stays small
//!     and looks crisp at any zoom.
//!
//! The editor writes v2 so exported files retain the full project structure.

use q0s_format::v2::ProjectV2;

/// Public entry: produce raw bytes for a `.q0s` v2 file.
pub fn export_to_q0s_bytes(project: &ProjectV2) -> Result<Vec<u8>, q0s_format::Error> {
    q0s_format::write_q0s_v2(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Anchor, Asset, Layer, Path as VPath, Placement, ProjectMeta, Q0rg, Rgba, Target,
        Transform2D, Tween, Vec2, VectorAsset,
    };

    fn one_red_square_project() -> ProjectV2 {
        let asset = Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(30.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(30.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        });
        ProjectV2 {
            meta: ProjectMeta {
                name: "t".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![asset],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 2,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "L".to_string(),
                    explicit_keyframes: Vec::new(),
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
    fn export_v2_round_trips_through_player_loader() {
        let project = one_red_square_project();
        let bytes = export_to_q0s_bytes(&project).expect("export");
        assert!(q0s_format::is_q0s_v2(&bytes));
        let parsed = q0s_format::parse_q0s_v2(&bytes).expect("parse");
        assert_eq!(parsed, project);
    }

    #[test]
    fn export_v2_is_dramatically_smaller_than_a_bitmap_snapshot_would_be() {
        // Sanity: vector .q0s should be tiny compared to even a single
        // pre-baked frame bitmap (stage_w*stage_h*4 = 16384 bytes for
        // 64Г—64). One asset + one placement is well under 200 bytes.
        let project = one_red_square_project();
        let bytes = export_to_q0s_bytes(&project).expect("export");
        let bitmap_per_frame = (64 * 64 * 4) as usize;
        assert!(
            bytes.len() < bitmap_per_frame,
            "vector .q0s ({}B) should beat bitmap-per-frame ({}B)",
            bytes.len(),
            bitmap_per_frame
        );
    }
}
