//! Strokes to their own layer: the outlines on a layer move to a new layer
//! just above it and the fills stay, on every keyframe. Line art can then
//! be locked, hidden or recoloured apart from the colours under it.

use std::collections::{BTreeMap, BTreeSet};

use q0s_format::v2::{Asset, LayerKey, LayerKind, LayerMetadata, ProjectV2, Target, VectorAsset};

/// What a stroked drawing on the layer becomes.
#[derive(Clone, Copy)]
struct Parts {
    /// The fills left behind, if the drawing had any.
    fill: Option<u16>,
    /// The outlines that move up.
    stroke: u16,
}

fn next_asset_id(project: &ProjectV2, taken: &BTreeSet<u16>) -> Option<u16> {
    let used: BTreeSet<u16> = project.assets.iter().map(Asset::id).collect();
    (1..=u16::MAX).find(|id| !used.contains(id) && !taken.contains(id))
}

/// Moves the outlines on `layer_id` of `q0rg_id` to a new layer just above
/// it, named after it, keyed on the same frames. Drawings with fills and
/// outlines are split in two. Returns the new layer's id.
pub fn strokes_to_own_layer(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
) -> Result<u16, String> {
    let metadata = project.layer_metadata(q0rg_id, layer_id);
    if metadata.kind != LayerKind::Normal {
        return Err("pick a drawing layer, not a folder or a mask".to_string());
    }
    let q0rg = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .ok_or("this timeline is gone")?;
    let index = q0rg
        .layers
        .iter()
        .position(|layer| layer.layer_id == layer_id)
        .ok_or("this layer is gone")?;
    let layer = &q0rg.layers[index];
    let vector = |asset_id: u16| match project.assets.iter().find(|asset| asset.id() == asset_id) {
        Some(Asset::Vector(vector)) => Some(vector),
        _ => None,
    };
    let stroked: BTreeSet<u16> = layer
        .placements
        .iter()
        .filter_map(|placement| match placement.target {
            Target::Asset(asset_id) => vector(asset_id)
                .filter(|vector| vector.stroke.is_some())
                .map(|_| asset_id),
            Target::Q0rg(_) => None,
        })
        .collect();
    if stroked.is_empty() {
        return Err("this layer has no lines".to_string());
    }
    // Drawings also placed somewhere else are copied, not changed.
    let shared: BTreeSet<u16> = project
        .q0rgs
        .iter()
        .flat_map(|other| {
            other.layers.iter().filter(move |other_layer| {
                other.q0rg_id != q0rg_id || other_layer.layer_id != layer_id
            })
        })
        .flat_map(|other_layer| other_layer.placements.iter())
        .filter_map(|placement| match placement.target {
            Target::Asset(asset_id) if stroked.contains(&asset_id) => Some(asset_id),
            _ => None,
        })
        .collect();

    let mut taken = BTreeSet::new();
    let mut parts: BTreeMap<u16, Parts> = BTreeMap::new();
    let mut new_assets: Vec<VectorAsset> = Vec::new();
    let mut emptied: BTreeMap<u16, VectorAsset> = BTreeMap::new();
    for &asset_id in &stroked {
        let source = vector(asset_id).ok_or("a drawing is missing")?.clone();
        let fills: Vec<_> = source
            .paths
            .iter()
            .filter(|path| path.closed)
            .cloned()
            .collect();
        if source.fill.is_none() || fills.is_empty() {
            // Only lines: the drawing itself moves up.
            parts.insert(
                asset_id,
                Parts {
                    fill: None,
                    stroke: asset_id,
                },
            );
            continue;
        }
        let stroke_id = next_asset_id(project, &taken).ok_or("no room for another drawing")?;
        taken.insert(stroke_id);
        new_assets.push(VectorAsset {
            asset_id: stroke_id,
            paths: source.paths.clone(),
            fill: None,
            stroke: source.stroke,
        });
        let fill_part = VectorAsset {
            asset_id,
            paths: fills,
            fill: source.fill,
            stroke: None,
        };
        let fill_id = if shared.contains(&asset_id) {
            let id = next_asset_id(project, &taken).ok_or("no room for another drawing")?;
            taken.insert(id);
            new_assets.push(VectorAsset {
                asset_id: id,
                ..fill_part
            });
            id
        } else {
            emptied.insert(asset_id, fill_part);
            asset_id
        };
        parts.insert(
            asset_id,
            Parts {
                fill: Some(fill_id),
                stroke: stroke_id,
            },
        );
    }

    for asset in &mut project.assets {
        if let Asset::Vector(vector) = asset {
            if let Some(fill_part) = emptied.remove(&vector.asset_id) {
                *vector = fill_part;
            }
        }
    }
    project
        .assets
        .extend(new_assets.into_iter().map(Asset::Vector));

    let q0rg = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .expect("found above");
    let new_id = q0rg
        .layers
        .iter()
        .map(|layer| layer.layer_id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("no room for another layer")?;
    let source = &mut q0rg.layers[index];
    let mut lines = source.clone();
    lines.layer_id = new_id;
    lines.name = format!("{} lines", source.name);
    lines.explicit_keyframes = source.keyframe_frames();
    lines.placements = Vec::new();
    let mut kept = Vec::with_capacity(source.placements.len());
    for placement in source.placements.drain(..) {
        let Target::Asset(asset_id) = placement.target else {
            kept.push(placement);
            continue;
        };
        let Some(split) = parts.get(&asset_id) else {
            kept.push(placement);
            continue;
        };
        let mut line = placement.clone();
        line.target = Target::Asset(split.stroke);
        lines.placements.push(line);
        if let Some(fill) = split.fill {
            let mut fill_placement = placement;
            fill_placement.target = Target::Asset(fill);
            kept.push(fill_placement);
        }
    }
    source.placements = kept;
    q0rg.layers.insert(index + 1, lines);
    // Lines sit in the same folder and under the same mask as their fills.
    project.layer_metadata.insert(
        LayerKey::new(q0rg_id, new_id),
        LayerMetadata {
            kind: LayerKind::Normal,
            collapsed: false,
            ..metadata
        },
    );
    Ok(new_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Anchor, Layer, Path as VPath, Placement, Rgba, Stroke, Transform2D, Tween, Vec2,
    };

    fn square(closed: bool) -> VPath {
        VPath {
            anchors: [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
                .into_iter()
                .map(|(x, y)| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed,
        }
    }

    fn placed(asset_id: u16, frame: u16) -> Placement {
        Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }
    }

    #[test]
    fn outlines_move_up_and_fills_stay_on_every_keyframe() {
        let mut project = crate::app::EditorApp::default().state.project;
        let q0rg_id = project.q0rgs[0].q0rg_id;
        let stroke = Stroke {
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            width: 3.0,
            cap: q0s_format::geom::CapShape::Round,
            join: q0s_format::geom::JoinShape::Round,
        };
        let red = Rgba {
            r: 200,
            g: 0,
            b: 0,
            a: 255,
        };
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 500,
            paths: vec![square(true), square(false)],
            fill: Some(red),
            stroke: Some(stroke),
        }));
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 501,
            paths: vec![square(false)],
            fill: None,
            stroke: Some(stroke),
        }));
        project.q0rgs[0].layers = vec![Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "pan".to_string(),
            explicit_keyframes: vec![0, 5],
            placements: vec![placed(500, 0), placed(501, 5)],
        }];
        let lines = strokes_to_own_layer(&mut project, q0rg_id, 1).unwrap();
        let layers = &project.q0rgs[0].layers;
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1].layer_id, lines, "just above");
        assert_eq!(layers[1].name, "pan lines");
        assert_eq!(layers[1].keyframe_frames(), vec![0, 5]);
        let fills = &layers[0];
        assert_eq!(
            fills.placements.len(),
            1,
            "the line-only drawing moved up whole"
        );
        let Target::Asset(fill_id) = fills.placements[0].target else {
            panic!()
        };
        let Some(Asset::Vector(fill)) = project.assets.iter().find(|a| a.id() == fill_id) else {
            panic!()
        };
        assert!(fill.stroke.is_none() && fill.fill == Some(red));
        assert_eq!(fill.paths.len(), 1, "only the closed shape keeps a fill");
        let moved: Vec<u16> = layers[1]
            .placements
            .iter()
            .map(|placement| match placement.target {
                Target::Asset(id) => id,
                Target::Q0rg(_) => 0,
            })
            .collect();
        assert_eq!(moved.len(), 2);
        assert!(moved.contains(&501));
        for id in moved {
            let Some(Asset::Vector(line)) = project.assets.iter().find(|a| a.id() == id) else {
                panic!()
            };
            assert!(line.fill.is_none() && line.stroke == Some(stroke));
        }
        assert!(
            strokes_to_own_layer(&mut project, q0rg_id, 1).is_err(),
            "nothing left to move"
        );
    }
}
