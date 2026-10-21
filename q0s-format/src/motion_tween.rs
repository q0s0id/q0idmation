//! Motion interpolation around an authored local transform anchor.
use crate::transform::Affine;
use crate::v2::{Asset, LayerKey, LayerKind, Placement, ProjectV2, Target, Transform2D, Vec2};
use std::collections::HashMap;

/// Interpolate the anchor's stage position and the object's authored linear
/// components independently, then solve translation around that anchor. Keep
/// unwrapped angles and the supplied easing progress (including overshoot).
pub fn interpolate(source: &Placement, destination: &Placement, t: f32) -> Transform2D {
    if t == 0.0 {
        return source.transform;
    }
    if t == 1.0 {
        return destination.transform;
    }
    let (a, b) = (source.transform, destination.transform);
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let mut value = Transform2D {
        tx: lerp(a.tx, b.tx),
        ty: lerp(a.ty, b.ty),
        sx: lerp(a.sx, b.sx),
        sy: lerp(a.sy, b.sy),
        rotation: lerp(a.rotation, b.rotation),
        skew_x: lerp(a.skew_x, b.skew_x),
        skew_y: lerp(a.skew_y, b.skew_y),
    };
    let Some(from) = source.pivot.or(destination.pivot) else {
        return value;
    };
    let to = destination.pivot.unwrap_or(from);
    let a = Affine::from_transform(a).apply(from);
    let b = Affine::from_transform(b).apply(to);
    let local = Vec2::new(lerp(from.x, to.x), lerp(from.y, to.y));
    let offset = Affine::from_transform(Transform2D {
        tx: 0.0,
        ty: 0.0,
        ..value
    })
    .apply(local);
    value.tx = lerp(a.x, b.x) - offset.x;
    value.ty = lerp(a.y, b.y) - offset.y;
    value
}

type Bounds = (f32, f32, f32, f32);
fn target_bounds(
    project: &ProjectV2,
    target: Target,
    cache: &mut HashMap<(u8, u16), Option<Bounds>>,
    depth: u8,
) -> Option<Bounds> {
    if depth > crate::v2::MAX_Q0RG_NESTING_DEPTH {
        return None;
    }
    let key = match target {
        Target::Asset(id) => (0, id),
        Target::Q0rg(id) => (1, id),
    };
    if let Some(bounds) = cache.get(&key) {
        return *bounds;
    }
    let mut bounds = None::<Bounds>;
    let mut include = |point: Vec2| {
        if !point.x.is_finite() || !point.y.is_finite() {
            return;
        }
        bounds = Some(match bounds {
            None => (point.x, point.y, point.x, point.y),
            Some((x0, y0, x1, y1)) => (
                x0.min(point.x),
                y0.min(point.y),
                x1.max(point.x),
                y1.max(point.y),
            ),
        });
    };
    match target {
        Target::Asset(id) => match project.assets.iter().find(|asset| asset.id() == id)? {
            Asset::Vector(vector) => {
                for path in &vector.paths {
                    for point in crate::geom::flatten_path(path, 16) {
                        include(point);
                    }
                }
            }
            Asset::Bitmap(bitmap) => {
                include(Vec2::new(0.0, 0.0));
                include(Vec2::new(bitmap.width as f32, bitmap.height as f32));
            }
            Asset::Q0v(video) => {
                let h = q0video::q0v::probe_header(&video.bytes).ok()?;
                include(Vec2::new(0.0, 0.0));
                include(Vec2::new(h.spec.width as f32, h.spec.height as f32));
            }
            Asset::Rig(_) => {}
        },
        Target::Q0rg(id) => {
            let symbol = project.q0rgs.iter().find(|symbol| symbol.q0rg_id == id)?;
            for layer in &symbol.layers {
                if !project.layer_is_visible(id, layer.layer_id)
                    || project
                        .layer_metadata
                        .get(&LayerKey {
                            q0rg_id: id,
                            layer_id: layer.layer_id,
                        })
                        .is_some_and(|meta| {
                            matches!(meta.kind, LayerKind::Folder | LayerKind::Mask)
                        })
                {
                    continue;
                }
                for (index, transform) in crate::raster::active_placements_at(layer, 0) {
                    let Some((x0, y0, x1, y1)) =
                        target_bounds(project, layer.placements[index].target, cache, depth + 1)
                    else {
                        continue;
                    };
                    let affine = Affine::from_transform(transform);
                    for p in [
                        Vec2::new(x0, y0),
                        Vec2::new(x1, y0),
                        Vec2::new(x1, y1),
                        Vec2::new(x0, y1),
                    ] {
                        include(affine.apply(p));
                    }
                }
            }
        }
    }
    cache.insert(key, bounds);
    bounds
}

/// Default anchor for a newly authored placement/tween, in target coordinates.
pub fn default_pivot(project: &ProjectV2, target: Target) -> Option<Vec2> {
    let (x0, y0, x1, y1) = target_bounds(project, target, &mut HashMap::new(), 0)?;
    Some(Vec2::new((x0 + x1) * 0.5, (y0 + y1) * 0.5))
}

/// Recover the legacy pure rotation case whose endpoints keep the artwork's
/// default centre fixed. Old files did not store custom anchors, so do not
/// guess a remote fixed point for an authored translation-and-rotation move.
pub(crate) fn recover_legacy_pivots(project: &mut ProjectV2) {
    let mut cache = HashMap::new();
    let mut repairs = HashMap::new();
    for (qi, symbol) in project.q0rgs.iter().enumerate() {
        for (li, layer) in symbol.layers.iter().enumerate() {
            for (si, source) in layer.placements.iter().enumerate() {
                if source.pivot.is_some() || !source.tween.is_motion() {
                    continue;
                }
                let Some(to_frame) = source.tween.to_frame() else {
                    continue;
                };
                let occurrence = layer.placements[..si]
                    .iter()
                    .filter(|p| p.frame == source.frame && p.target == source.target)
                    .count();
                let Some((di, dest)) = layer
                    .placements
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.frame == to_frame && p.target == source.target)
                    .nth(occurrence)
                else {
                    continue;
                };
                if dest.pivot.is_some()
                    || (source.transform.rotation - dest.transform.rotation).abs() < 1.0e-5
                {
                    continue;
                }
                let Some((x0, y0, x1, y1)) = target_bounds(project, source.target, &mut cache, 0)
                else {
                    continue;
                };
                let centre = Vec2::new((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                if centre.x.hypot(centre.y) < 1.0e-5 {
                    continue;
                }
                let a = Affine::from_transform(source.transform);
                let b = Affine::from_transform(dest.transform);
                let (p, q) = (a.apply(centre), b.apply(centre));
                if (p.x - q.x).hypot(p.y - q.y) > 0.5 {
                    continue;
                }
                let difference = Affine {
                    a11: a.a11 - b.a11,
                    a12: a.a12 - b.a12,
                    a21: a.a21 - b.a21,
                    a22: a.a22 - b.a22,
                    tx: 0.0,
                    ty: 0.0,
                };
                let Some(inverse) = difference.inverse() else {
                    continue;
                };
                let pivot = inverse.apply(Vec2::new(b.tx - a.tx, b.ty - a.ty));
                if !pivot.x.is_finite() || !pivot.y.is_finite() {
                    continue;
                }
                repairs.insert((qi, li, si), pivot);
                repairs.insert((qi, li, di), pivot);
            }
        }
    }
    for ((qi, li, pi), pivot) in repairs {
        project.q0rgs[qi].layers[li].placements[pi].pivot = Some(pivot);
    }
}
