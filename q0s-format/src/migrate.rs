use crate::error::Error;
use crate::q1s::Q1Project;
use crate::v2::{
    Asset, BitmapAsset, Layer, Placement, ProjectMeta, ProjectV2, Q0rg, Target, Transform2D, Tween,
};

pub fn migrate_v1_to_v2(v1: Q1Project) -> Result<ProjectV2, Error> {
    let assets = v1
        .assets
        .into_iter()
        .map(|a| {
            Asset::Bitmap(BitmapAsset {
                asset_id: a.asset_id,
                width: a.width,
                height: a.height,
                rgba: a.rgba,
            })
        })
        .collect();

    let q0rgs = v1
        .scenes
        .into_iter()
        .map(|s| Q0rg {
            q0rg_id: s.scene_id,
            name: s.name,
            frame_count: s.frame_count,
            script: String::new(),
            layers: s
                .layers
                .into_iter()
                .map(|l| Layer {
                    layer_id: l.layer_id,
                    name: l.name,
                    explicit_keyframes: Vec::new(),
                    placements: l
                        .placements
                        .into_iter()
                        .map(|p| Placement {
                            instance_id: 0,
                            frame: p.frame,
                            target: Target::Asset(p.asset_id),
                            transform: Transform2D {
                                tx: f32::from(p.x),
                                ty: f32::from(p.y),
                                sx: p.scale_x,
                                sy: p.scale_y,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                            fx: Default::default(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();

    let project = ProjectV2 {
        meta: ProjectMeta {
            name: v1.meta.name,
            fps: v1.meta.fps,
            stage_width: v1.meta.stage_width,
            stage_height: v1.meta.stage_height,
            entry_q0rg_id: v1.meta.entry_scene_id,
        },
        assets,
        asset_names: std::collections::HashMap::new(),
        asset_appearances: std::collections::HashMap::new(),
        layer_metadata: std::collections::HashMap::new(),
        q0rgs,
    };
    crate::v2::validate(&project)?;
    Ok(project)
}
