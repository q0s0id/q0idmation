use std::collections::HashSet;

use egui::{Color32, Context, Painter, PointerButton, Response, Stroke, Ui};
use q0s_format::rig::{
    blended_pose_values, build_skin_anchor_weights, control_world_position, evaluate_rig,
    mirror_pose_values, rest_node_worlds, rig_for_q0rg, rig_for_q0rg_mut,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, Easing, ProjectV2, RigAsset, RigBinding, RigConstraint, RigControl, RigControlKind,
    RigDeformer, RigDriver, RigMirrorPair, RigNode, RigPoseBlendMode, RigPoseDriver, RigPosePreset,
    RigPoseValue, RigPropertyRef, RigSkinBoneBind, RigSkinWeight, RigVariantChoice, RigVariantSet,
    Target, Transform2D, Vec2, VectorAsset,
};

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{RigBoneDrag, RigControlDrag, RigEditMode, RigMode, Selection};

const CONTROL_HIT_PX: f32 = 10.0;
const BONE_HIT_PX: f32 = 8.0;

pub fn ensure_rig(project: &mut ProjectV2, q0rg_id: u16) -> Option<u16> {
    if let Some(rig) = rig_for_q0rg(project, q0rg_id) {
        return Some(rig.asset_id);
    }
    project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let asset_id = next_asset_id(project)?;
    project.assets.push(Asset::Rig(RigAsset {
        asset_id,
        owner_q0rg_id: q0rg_id,
        nodes: Vec::new(),
        controls: Vec::new(),
        constraints: Vec::new(),
        channels: Vec::new(),
        drivers: Vec::new(),
        poses: Vec::new(),
        deformers: Vec::new(),
        pose_drivers: Vec::new(),
        mirror_pairs: Vec::new(),
        variants: Vec::new(),
    }));
    Some(asset_id)
}

pub fn delete_rig(project: &mut ProjectV2, q0rg_id: u16) -> bool {
    let Some(asset_id) = rig_for_q0rg(project, q0rg_id).map(|rig| rig.asset_id) else {
        return false;
    };
    project.assets.retain(|asset| asset.id() != asset_id);
    project.asset_names.remove(&asset_id);
    project.asset_appearances.remove(&asset_id);
    true
}

fn next_asset_id(project: &ProjectV2) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| !project.assets.iter().any(|asset| asset.id() == *candidate))
}

fn next_instance_id(project: &ProjectV2) -> Option<u32> {
    let used = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| (placement.instance_id != 0).then_some(placement.instance_id))
        .collect::<HashSet<_>>();
    (1..=u32::MAX).find(|candidate| !used.contains(candidate))
}

fn next_node_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| !rig.nodes.iter().any(|node| node.node_id == *candidate))
}

fn next_control_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        !rig.controls
            .iter()
            .any(|control| control.control_id == *candidate)
    })
}

fn next_constraint_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        !rig.constraints
            .iter()
            .any(|constraint| constraint.id() == *candidate)
    })
}

fn next_driver_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        !rig.drivers
            .iter()
            .any(|driver| driver.driver_id == *candidate)
    })
}
fn next_deformer_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        rig.deformers
            .iter()
            .all(|deformer| deformer.id() != *candidate)
    })
}
fn next_pose_driver_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        rig.pose_drivers
            .iter()
            .all(|driver| driver.driver_id != *candidate)
    })
}

fn next_variant_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| {
        rig.variants
            .iter()
            .all(|variant| variant.variant_id != *candidate)
    })
}

fn next_pose_id(rig: &RigAsset) -> Option<u16> {
    (1..=u16::MAX).find(|candidate| !rig.poses.iter().any(|pose| pose.pose_id == *candidate))
}

fn add_control(
    project: &mut ProjectV2,
    q0rg_id: u16,
    kind: RigControlKind,
    target_node: Option<u16>,
) -> Option<u16> {
    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    if target_node.is_some_and(|id| !rig.nodes.iter().any(|node| node.node_id == id)) {
        return None;
    }
    let control_id = next_control_id(rig)?;
    let (name, rest_x, rest_y, rest_value, min_value, max_value) = match kind {
        RigControlKind::Position2D => {
            let (x, y) = target_node
                .and_then(|id| rig.nodes.iter().find(|node| node.node_id == id))
                .map(|node| (node.rest.tx, node.rest.ty))
                .unwrap_or((0.0, 0.0));
            (format!("Joystick {control_id}"), x, y, 0.0, -1000.0, 1000.0)
        }
        RigControlKind::Rotation => {
            let value = target_node
                .and_then(|id| rig.nodes.iter().find(|node| node.node_id == id))
                .map(|node| node.rest.rotation)
                .unwrap_or(0.0);
            (
                format!("Dial {control_id}"),
                0.0,
                0.0,
                value,
                -std::f32::consts::PI,
                std::f32::consts::PI,
            )
        }
        RigControlKind::Slider => (format!("Slider {control_id}"), 0.0, 0.0, 0.0, 0.0, 1.0),
        RigControlKind::Toggle => (format!("Toggle {control_id}"), 0.0, 0.0, 0.0, 0.0, 1.0),
    };
    rig.controls.push(RigControl {
        control_id,
        name,
        kind,
        target_node,
        rest_x,
        rest_y,
        rest_value,
        min_value,
        max_value,
        public_in_simple: true,
    });
    Some(control_id)
}

fn add_master_slider(project: &mut ProjectV2, q0rg_id: u16) -> Option<u16> {
    add_control(project, q0rg_id, RigControlKind::Slider, None)
}

fn add_master_driver(
    project: &mut ProjectV2,
    q0rg_id: u16,
    source_control: u16,
    target: RigPropertyRef,
    target_min: f32,
    target_max: f32,
) -> Result<u16, &'static str> {
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("rig is missing")?;
    let source = rig
        .controls
        .iter()
        .find(|control| control.control_id == source_control)
        .ok_or("master control is missing")?;
    if !matches!(source.kind, RigControlKind::Slider | RigControlKind::Toggle) {
        return Err("master driver source must be a slider or toggle");
    }
    if rig.drivers.iter().any(|driver| driver.target == target) {
        return Err("that property is already driven by another master control");
    }
    let driver_id = next_driver_id(rig).ok_or("no free driver ids")?;
    rig.drivers.push(RigDriver {
        driver_id,
        source_control,
        source_min: source.min_value,
        source_max: source.max_value,
        target,
        target_min,
        target_max,
    });
    Ok(driver_id)
}

fn add_pose_driver(
    project: &mut ProjectV2,
    q0rg_id: u16,
    source_control: u16,
    pose_id: u16,
    mode: RigPoseBlendMode,
) -> Result<u16, &'static str> {
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("rig is missing")?;
    let control = rig
        .controls
        .iter()
        .find(|control| control.control_id == source_control)
        .ok_or("source control is missing")?;
    if !matches!(
        control.kind,
        RigControlKind::Slider | RigControlKind::Toggle
    ) {
        return Err("pose driver source must be a slider or toggle");
    }
    if !rig.poses.iter().any(|pose| pose.pose_id == pose_id) {
        return Err("pose is missing");
    }
    let driver_id = next_pose_driver_id(rig).ok_or("no free pose driver ids")?;
    rig.pose_drivers.push(RigPoseDriver {
        driver_id,
        source_control,
        pose_id,
        source_min: control.min_value,
        source_max: control.max_value.max(control.min_value + 1.0e-4),
        weight_min: 0.0,
        weight_max: 1.0,
        mode,
    });
    Ok(driver_id)
}

fn add_node_mirror_pairs(
    project: &mut ProjectV2,
    q0rg_id: u16,
    left: u16,
    right: u16,
) -> Result<(), &'static str> {
    if left == right {
        return Err("mirror pair needs two different bones");
    }
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("rig is missing")?;
    if !rig.nodes.iter().any(|node| node.node_id == left)
        || !rig.nodes.iter().any(|node| node.node_id == right)
    {
        return Err("mirror bone is missing");
    }
    let candidates = [
        (
            RigPropertyRef::NodeTx(left),
            RigPropertyRef::NodeTx(right),
            -1.0,
        ),
        (
            RigPropertyRef::NodeTy(left),
            RigPropertyRef::NodeTy(right),
            1.0,
        ),
        (
            RigPropertyRef::NodeRotation(left),
            RigPropertyRef::NodeRotation(right),
            -1.0,
        ),
        (
            RigPropertyRef::NodeScaleX(left),
            RigPropertyRef::NodeScaleX(right),
            1.0,
        ),
        (
            RigPropertyRef::NodeScaleY(left),
            RigPropertyRef::NodeScaleY(right),
            1.0,
        ),
    ];
    for (a, b, _) in candidates {
        if rig.mirror_pairs.iter().any(|pair| {
            [pair.left, pair.right].contains(&a) || [pair.left, pair.right].contains(&b)
        }) {
            return Err("one of those bone properties is already mirrored");
        }
    }
    rig.mirror_pairs
        .extend(
            candidates
                .into_iter()
                .map(|(left, right, multiplier)| RigMirrorPair {
                    left,
                    right,
                    multiplier,
                    offset: 0.0,
                }),
        );
    Ok(())
}

fn add_control_mirror_pairs(
    project: &mut ProjectV2,
    q0rg_id: u16,
    left: u16,
    right: u16,
) -> Result<(), &'static str> {
    if left == right {
        return Err("mirror pair needs two different controls");
    }
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("rig is missing")?;
    let left_kind = rig
        .controls
        .iter()
        .find(|control| control.control_id == left)
        .map(|control| control.kind)
        .ok_or("mirror control is missing")?;
    let right_kind = rig
        .controls
        .iter()
        .find(|control| control.control_id == right)
        .map(|control| control.kind)
        .ok_or("mirror control is missing")?;
    if left_kind != right_kind {
        return Err("mirror controls must have the same kind");
    }
    let candidates = match left_kind {
        RigControlKind::Position2D => vec![
            (
                RigPropertyRef::ControlX(left),
                RigPropertyRef::ControlX(right),
                -1.0,
            ),
            (
                RigPropertyRef::ControlY(left),
                RigPropertyRef::ControlY(right),
                1.0,
            ),
        ],
        RigControlKind::Rotation => vec![(
            RigPropertyRef::ControlValue(left),
            RigPropertyRef::ControlValue(right),
            -1.0,
        )],
        RigControlKind::Slider | RigControlKind::Toggle => vec![(
            RigPropertyRef::ControlValue(left),
            RigPropertyRef::ControlValue(right),
            1.0,
        )],
    };
    for (a, b, _) in &candidates {
        if rig
            .mirror_pairs
            .iter()
            .any(|pair| [pair.left, pair.right].contains(a) || [pair.left, pair.right].contains(b))
        {
            return Err("one of those control properties is already mirrored");
        }
    }
    rig.mirror_pairs
        .extend(
            candidates
                .into_iter()
                .map(|(left, right, multiplier)| RigMirrorPair {
                    left,
                    right,
                    multiplier,
                    offset: 0.0,
                }),
        );
    Ok(())
}

fn create_variant_set_from_selection(
    project: &mut ProjectV2,
    selection: &Selection,
    frame: u16,
    source_control: u16,
) -> Result<u16, &'static str> {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = *selection
    else {
        return Err("select a display object first");
    };
    let (target, _, _) =
        selected_active_occurrence(project, q0rg_id, layer_id, placement_idx, frame)
            .ok_or("selected object is not active on this frame")?;
    let (instance_id, _) =
        ensure_selected_track_instance_id(project, q0rg_id, layer_id, placement_idx, frame)
            .ok_or("selected object is not active on this frame")?;
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("create a rig first")?;
    let control = rig
        .controls
        .iter()
        .find(|control| control.control_id == source_control)
        .ok_or("selected scalar control is missing")?;
    if !matches!(
        control.kind,
        RigControlKind::Slider | RigControlKind::Toggle
    ) {
        return Err("variant source must be a slider or toggle");
    }
    if rig
        .variants
        .iter()
        .any(|variant| variant.instance_id == instance_id)
    {
        return Err("that object already has a variant set");
    }
    if rig
        .deformers
        .iter()
        .any(|deformer| deformer.instance_id() == instance_id)
    {
        return Err("drawing substitutions cannot share one instance with a vector deformer");
    }
    let variant_id = next_variant_id(rig).ok_or("no free variant ids")?;
    rig.variants.push(RigVariantSet {
        variant_id,
        name: format!("Variants {variant_id}"),
        instance_id,
        source_control,
        choices: vec![RigVariantChoice {
            name: "default".into(),
            target,
        }],
    });
    Ok(variant_id)
}

fn target_label(project: &ProjectV2, target: Target) -> String {
    match target {
        Target::Asset(id) => project
            .asset_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("asset #{id}")),
        Target::Q0rg(id) => project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == id)
            .map(|q0rg| q0rg.name.clone())
            .unwrap_or_else(|| format!("q0rg #{id}")),
    }
}

fn save_pose(project: &mut ProjectV2, q0rg_id: u16, frame: u16) -> Option<u16> {
    let snapshot = rig_for_q0rg(project, q0rg_id)?.clone();
    let pose_id = next_pose_id(&snapshot)?;
    let values = q0s_format::rig::capture_pose_values(&snapshot, f32::from(frame));
    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    rig.poses.push(RigPosePreset {
        pose_id,
        name: format!("Pose {pose_id}"),
        values,
    });
    Some(pose_id)
}

fn apply_pose_values(
    project: &mut ProjectV2,
    q0rg_id: u16,
    values: &[RigPoseValue],
    frame: u16,
    auto_key: bool,
) -> Result<(), &'static str> {
    let snapshot = rig_for_q0rg(project, q0rg_id)
        .ok_or("rig is missing")?
        .clone();
    let current = evaluate_rig(&snapshot, f32::from(frame), &[]);
    for control in &snapshot.controls {
        match control.kind {
            RigControlKind::Position2D => {
                let base = current
                    .controls
                    .get(&control.control_id)
                    .copied()
                    .unwrap_or(q0s_format::rig::RigControlValue {
                        x: control.rest_x,
                        y: control.rest_y,
                        value: control.rest_value,
                    });
                let x = values
                    .iter()
                    .find(|value| value.property == RigPropertyRef::ControlX(control.control_id))
                    .map(|value| value.value)
                    .unwrap_or(base.x);
                let y = values
                    .iter()
                    .find(|value| value.property == RigPropertyRef::ControlY(control.control_id))
                    .map(|value| value.value)
                    .unwrap_or(base.y);
                if values.iter().any(|value| {
                    matches!(
                        value.property,
                        RigPropertyRef::ControlX(id) | RigPropertyRef::ControlY(id)
                            if id == control.control_id
                    )
                }) {
                    set_control_position(
                        project,
                        q0rg_id,
                        control.control_id,
                        frame,
                        x,
                        y,
                        auto_key,
                    );
                }
            }
            RigControlKind::Rotation | RigControlKind::Slider | RigControlKind::Toggle => {
                if let Some(value) = values.iter().find(|value| {
                    value.property == RigPropertyRef::ControlValue(control.control_id)
                }) {
                    set_control_value(
                        project,
                        q0rg_id,
                        control.control_id,
                        frame,
                        value.value,
                        auto_key,
                    );
                }
            }
        }
    }
    for value in values {
        match value.property {
            RigPropertyRef::NodeRotation(id) => {
                set_node_rotation(project, q0rg_id, id, frame, value.value, auto_key);
            }
            RigPropertyRef::NodeTx(id)
            | RigPropertyRef::NodeTy(id)
            | RigPropertyRef::NodeScaleX(id)
            | RigPropertyRef::NodeScaleY(id) => {
                set_node_property(
                    project,
                    q0rg_id,
                    id,
                    frame,
                    value.property,
                    value.value,
                    auto_key,
                );
            }
            RigPropertyRef::ConstraintWeight(id) => {
                set_constraint_weight(project, q0rg_id, id, frame, value.value, auto_key);
            }
            RigPropertyRef::ControlX(_)
            | RigPropertyRef::ControlY(_)
            | RigPropertyRef::ControlValue(_) => {}
        }
    }
    Ok(())
}

fn apply_pose(
    project: &mut ProjectV2,
    q0rg_id: u16,
    pose_id: u16,
    frame: u16,
    auto_key: bool,
) -> Result<(), &'static str> {
    let values = rig_for_q0rg(project, q0rg_id)
        .and_then(|rig| rig.poses.iter().find(|pose| pose.pose_id == pose_id))
        .map(|pose| pose.values.clone())
        .ok_or("pose is missing")?;
    apply_pose_values(project, q0rg_id, &values, frame, auto_key)
}

pub fn add_bone(
    project: &mut ProjectV2,
    q0rg_id: u16,
    parent: Option<u16>,
    start: Vec2,
    end: Vec2,
    frame: u16,
) -> Option<u16> {
    ensure_rig(project, q0rg_id)?;
    let rig_snapshot = rig_for_q0rg(project, q0rg_id)?.clone();
    if parent.is_some_and(|id| !rig_snapshot.nodes.iter().any(|node| node.node_id == id)) {
        return None;
    }

    let parent_inverse = parent
        .and_then(|parent_id| {
            evaluate_rig(&rig_snapshot, f32::from(frame), &[])
                .node_world
                .get(&parent_id)
                .copied()
        })
        .and_then(|affine| affine.inverse())
        .unwrap_or(Affine::IDENTITY);
    let local_start = parent_inverse.apply(start);
    let local_end = parent_inverse.apply(end);
    let dx = local_end.x - local_start.x;
    let dy = local_end.y - local_start.y;
    let length = (dx * dx + dy * dy).sqrt();
    if !length.is_finite() || length < 0.5 {
        return None;
    }

    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    let node_id = next_node_id(rig)?;
    rig.nodes.push(RigNode {
        node_id,
        name: format!("Bone {node_id}"),
        parent,
        rest: Transform2D {
            tx: local_start.x,
            ty: local_start.y,
            rotation: dy.atan2(dx),
            ..Transform2D::IDENTITY
        },
        length,
        binding: None,
    });
    Some(node_id)
}

pub fn remove_node_subtree(project: &mut ProjectV2, q0rg_id: u16, node_id: u16) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    if !rig.nodes.iter().any(|node| node.node_id == node_id) {
        return false;
    }
    let mut removed = HashSet::from([node_id]);
    loop {
        let before = removed.len();
        for node in &rig.nodes {
            if node.parent.is_some_and(|parent| removed.contains(&parent)) {
                removed.insert(node.node_id);
            }
        }
        if before == removed.len() {
            break;
        }
    }
    let removed_controls = rig
        .controls
        .iter()
        .filter(|control| {
            control
                .target_node
                .is_some_and(|node| removed.contains(&node))
        })
        .map(|control| control.control_id)
        .collect::<HashSet<_>>();
    rig.nodes.retain(|node| !removed.contains(&node.node_id));
    rig.controls
        .retain(|control| !removed_controls.contains(&control.control_id));
    rig.constraints.retain(|constraint| match *constraint {
        RigConstraint::RotationLimit { node_id, .. }
        | RigConstraint::PositionLimit { node_id, .. } => !removed.contains(&node_id),
        RigConstraint::Aim {
            node_id,
            target_control,
            ..
        }
        | RigConstraint::Distance {
            node_id,
            target_control,
            ..
        } => !removed.contains(&node_id) && !removed_controls.contains(&target_control),
        RigConstraint::Transform {
            node_id,
            target_node,
            ..
        } => !removed.contains(&node_id) && !removed.contains(&target_node),
        RigConstraint::TwoBoneIk {
            root_node,
            mid_node,
            tip_node,
            target_control,
            pole_control,
            ..
        } => {
            !removed.contains(&root_node)
                && !removed.contains(&mid_node)
                && !removed.contains(&tip_node)
                && !removed_controls.contains(&target_control)
                && pole_control.is_none_or(|id| !removed_controls.contains(&id))
        }
    });
    let remaining_constraints = rig
        .constraints
        .iter()
        .map(RigConstraint::id)
        .collect::<HashSet<_>>();
    rig.channels.retain(|channel| match channel.property {
        RigPropertyRef::NodeTx(id)
        | RigPropertyRef::NodeTy(id)
        | RigPropertyRef::NodeRotation(id)
        | RigPropertyRef::NodeScaleX(id)
        | RigPropertyRef::NodeScaleY(id) => !removed.contains(&id),
        RigPropertyRef::ControlX(id)
        | RigPropertyRef::ControlY(id)
        | RigPropertyRef::ControlValue(id) => !removed_controls.contains(&id),
        RigPropertyRef::ConstraintWeight(id) => remaining_constraints.contains(&id),
    });
    rig.drivers.retain(|driver| {
        !removed_controls.contains(&driver.source_control)
            && match driver.target {
                RigPropertyRef::NodeTx(id)
                | RigPropertyRef::NodeTy(id)
                | RigPropertyRef::NodeRotation(id)
                | RigPropertyRef::NodeScaleX(id)
                | RigPropertyRef::NodeScaleY(id) => !removed.contains(&id),
                RigPropertyRef::ConstraintWeight(id) => remaining_constraints.contains(&id),
                RigPropertyRef::ControlX(id)
                | RigPropertyRef::ControlY(id)
                | RigPropertyRef::ControlValue(id) => !removed_controls.contains(&id),
            }
    });
    for pose in &mut rig.poses {
        pose.values.retain(|value| match value.property {
            RigPropertyRef::NodeTx(id)
            | RigPropertyRef::NodeTy(id)
            | RigPropertyRef::NodeRotation(id)
            | RigPropertyRef::NodeScaleX(id)
            | RigPropertyRef::NodeScaleY(id) => !removed.contains(&id),
            RigPropertyRef::ConstraintWeight(id) => remaining_constraints.contains(&id),
            RigPropertyRef::ControlX(id)
            | RigPropertyRef::ControlY(id)
            | RigPropertyRef::ControlValue(id) => !removed_controls.contains(&id),
        });
    }
    true
}
fn selected_active_occurrence(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
) -> Option<(Target, u16, Affine)> {
    let layer = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?;
    let mut seen = Vec::<Target>::new();
    for active in q0s_format::raster::active_placement_states_at(layer, frame) {
        let placement = layer.placements.get(active.index)?;
        let occurrence = seen
            .iter()
            .filter(|target| **target == placement.target)
            .count() as u16;
        seen.push(placement.target);
        if active.index == placement_idx {
            return Some((
                placement.target,
                occurrence,
                Affine::from_transform(active.transform),
            ));
        }
    }
    None
}

fn ensure_selected_track_instance_id(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
) -> Option<(u32, Affine)> {
    let (target, occurrence, visible_transform) =
        selected_active_occurrence(project, q0rg_id, layer_id, placement_idx, frame)?;
    let existing = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?
        .placements
        .get(placement_idx)?
        .instance_id;
    if existing != 0 {
        return Some((existing, visible_transform));
    }
    let instance_id = next_instance_id(project)?;

    // Legacy/current unbound placements have no identity yet. Propagate the new
    // id through the same target occurrence at every authored keyframe so a
    // held/tweened display object remains one logical object after binding.
    let matching_indices = {
        let layer = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        layer
            .placements
            .iter()
            .enumerate()
            .filter_map(|(candidate_idx, candidate)| {
                if candidate.target != target || candidate.instance_id != 0 {
                    return None;
                }
                let mut seen = Vec::<Target>::new();
                for active in q0s_format::raster::active_placement_states_at(layer, candidate.frame)
                {
                    let active_placement = layer.placements.get(active.index)?;
                    let active_occurrence = seen
                        .iter()
                        .filter(|seen_target| **seen_target == active_placement.target)
                        .count() as u16;
                    seen.push(active_placement.target);
                    if active.index == candidate_idx {
                        return (active_occurrence == occurrence).then_some(candidate_idx);
                    }
                }
                None
            })
            .collect::<Vec<_>>()
    };
    let q0rg = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let layer = q0rg
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)?;
    for index in matching_indices {
        if let Some(placement) = layer.placements.get_mut(index) {
            placement.instance_id = instance_id;
        }
    }
    // The selected source must always be included even if malformed legacy
    // timing kept it out of the occurrence scan.
    layer.placements.get_mut(placement_idx)?.instance_id = instance_id;
    Some((instance_id, visible_transform))
}

fn selected_vector_for_deformer(
    project: &mut ProjectV2,
    selection: &Selection,
    frame: u16,
) -> Result<(u16, u32, u16, Affine, VectorAsset), &'static str> {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = *selection
    else {
        return Err("select a transformed vector display object first");
    };
    let (target, _, _) =
        selected_active_occurrence(project, q0rg_id, layer_id, placement_idx, frame)
            .ok_or("selected object is not active on this frame")?;
    let Target::Asset(asset_id) = target else {
        return Err("deformers currently require a vector display object");
    };
    let vector = project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Vector(vector) if vector.asset_id == asset_id => Some(vector.clone()),
            _ => None,
        })
        .ok_or("selected asset is not a vector")?;
    if vector.paths.is_empty() || vector.paths.iter().all(|path| path.anchors.is_empty()) {
        return Err("selected vector has no anchors to deform");
    }
    let (instance_id, bind_transform) =
        ensure_selected_track_instance_id(project, q0rg_id, layer_id, placement_idx, frame)
            .ok_or("selected object is not active on this frame")?;
    let rig = rig_for_q0rg(project, q0rg_id).ok_or("create a rig first")?;
    if rig.nodes.iter().any(|node| {
        node.binding
            .is_some_and(|binding| binding.instance_id == instance_id)
    }) {
        return Err("unbind this object from its rigid bone before adding a deformer");
    }
    if rig
        .deformers
        .iter()
        .any(|deformer| deformer.instance_id() == instance_id)
    {
        return Err("that display object already has a deformer");
    }
    Ok((q0rg_id, instance_id, asset_id, bind_transform, vector))
}

fn vector_local_bounds(vector: &VectorAsset) -> Option<(Vec2, Vec2)> {
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut found = false;
    for path in &vector.paths {
        for anchor in &path.anchors {
            for point in [Some(anchor.point), anchor.in_handle, anchor.out_handle]
                .into_iter()
                .flatten()
            {
                if !point.x.is_finite() || !point.y.is_finite() {
                    continue;
                }
                min.x = min.x.min(point.x);
                min.y = min.y.min(point.y);
                max.x = max.x.max(point.x);
                max.y = max.y.max(point.y);
                found = true;
            }
        }
    }
    found.then_some((min, max))
}

fn add_free_position_control(
    project: &mut ProjectV2,
    q0rg_id: u16,
    name: String,
    position: Vec2,
    public_in_simple: bool,
) -> Option<u16> {
    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    let control_id = next_control_id(rig)?;
    rig.controls.push(RigControl {
        control_id,
        name,
        kind: RigControlKind::Position2D,
        target_node: None,
        rest_x: position.x,
        rest_y: position.y,
        rest_value: 0.0,
        min_value: -100_000.0,
        max_value: 100_000.0,
        public_in_simple,
    });
    Some(control_id)
}

fn create_skin_deformer_from_selection(
    project: &mut ProjectV2,
    selection: &Selection,
    frame: u16,
) -> Result<u16, &'static str> {
    let (q0rg_id, instance_id, asset_id, bind_transform, vector) =
        selected_vector_for_deformer(project, selection, frame)?;
    let snapshot = rig_for_q0rg(project, q0rg_id)
        .ok_or("create a rig first")?
        .clone();
    if snapshot.nodes.is_empty() {
        return Err("draw at least one bone before auto skinning");
    }
    let worlds = rest_node_worlds(&snapshot);
    let bones = snapshot
        .nodes
        .iter()
        .filter_map(|node| {
            worlds
                .get(&node.node_id)
                .and_then(|world| world.inverse())
                .map(|inverse| RigSkinBoneBind {
                    node_id: node.node_id,
                    inverse_rest_world: inverse,
                })
        })
        .collect::<Vec<_>>();
    if bones.len() != snapshot.nodes.len() {
        return Err("one of the rest bone transforms is singular");
    }
    let anchors = build_skin_anchor_weights(&snapshot, &vector, bind_transform, 4);
    if anchors.len()
        != vector
            .paths
            .iter()
            .map(|path| path.anchors.len())
            .sum::<usize>()
    {
        return Err("could not assign valid weights to every vector anchor");
    }
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("create a rig first")?;
    let deformer_id = next_deformer_id(rig).ok_or("no free deformer ids")?;
    rig.deformers.push(RigDeformer::Skin {
        deformer_id,
        instance_id,
        asset_id,
        bind_transform,
        bones,
        anchors,
    });
    Ok(deformer_id)
}

fn create_bend_deformer_from_selection(
    project: &mut ProjectV2,
    selection: &Selection,
    frame: u16,
) -> Result<u16, &'static str> {
    let (q0rg_id, instance_id, asset_id, bind_transform, vector) =
        selected_vector_for_deformer(project, selection, frame)?;
    let (min, max) = vector_local_bounds(&vector).ok_or("vector bounds are empty")?;
    let width = max.x - min.x;
    let height = max.y - min.y;
    if width.abs().max(height.abs()) <= 1.0e-5 {
        return Err("vector is too small for a bend deformer");
    }
    let center = Vec2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
    let (axis_start, axis_end) = if width >= height {
        (Vec2::new(min.x, center.y), Vec2::new(max.x, center.y))
    } else {
        (Vec2::new(center.x, min.y), Vec2::new(center.x, max.y))
    };
    let axis_middle = Vec2::new(
        (axis_start.x + axis_end.x) * 0.5,
        (axis_start.y + axis_end.y) * 0.5,
    );
    let deformer_id = next_deformer_id(rig_for_q0rg(project, q0rg_id).ok_or("create a rig first")?)
        .ok_or("no free deformer ids")?;
    let start_control = add_free_position_control(
        project,
        q0rg_id,
        format!("Bend {deformer_id} start"),
        bind_transform.apply(axis_start),
        true,
    )
    .ok_or("no free control ids")?;
    let middle_control = add_free_position_control(
        project,
        q0rg_id,
        format!("Bend {deformer_id} curve"),
        bind_transform.apply(axis_middle),
        true,
    )
    .ok_or("no free control ids")?;
    let end_control = add_free_position_control(
        project,
        q0rg_id,
        format!("Bend {deformer_id} end"),
        bind_transform.apply(axis_end),
        true,
    )
    .ok_or("no free control ids")?;
    rig_for_q0rg_mut(project, q0rg_id)
        .ok_or("create a rig first")?
        .deformers
        .push(RigDeformer::Bend {
            deformer_id,
            instance_id,
            asset_id,
            bind_transform,
            axis_start,
            axis_end,
            start_control,
            middle_control,
            end_control,
        });
    Ok(deformer_id)
}

fn create_cage_deformer_from_selection(
    project: &mut ProjectV2,
    selection: &Selection,
    frame: u16,
) -> Result<u16, &'static str> {
    let (q0rg_id, instance_id, asset_id, bind_transform, vector) =
        selected_vector_for_deformer(project, selection, frame)?;
    let (min, max) = vector_local_bounds(&vector).ok_or("vector bounds are empty")?;
    if max.x - min.x <= 1.0e-5 || max.y - min.y <= 1.0e-5 {
        return Err("vector needs non-zero width and height for a cage");
    }
    let deformer_id = next_deformer_id(rig_for_q0rg(project, q0rg_id).ok_or("create a rig first")?)
        .ok_or("no free deformer ids")?;
    let corners = [
        Vec2::new(min.x, min.y),
        Vec2::new(max.x, min.y),
        Vec2::new(max.x, max.y),
        Vec2::new(min.x, max.y),
    ];
    let labels = ["TL", "TR", "BR", "BL"];
    let mut controls = [0_u16; 4];
    for (index, (corner, label)) in corners.into_iter().zip(labels).enumerate() {
        controls[index] = add_free_position_control(
            project,
            q0rg_id,
            format!("Cage {deformer_id} {label}"),
            bind_transform.apply(corner),
            true,
        )
        .ok_or("no free control ids")?;
    }
    rig_for_q0rg_mut(project, q0rg_id)
        .ok_or("create a rig first")?
        .deformers
        .push(RigDeformer::Cage {
            deformer_id,
            instance_id,
            asset_id,
            bind_transform,
            rest_min: min,
            rest_max: max,
            controls,
        });
    Ok(deformer_id)
}

fn control_is_referenced_elsewhere(rig: &RigAsset, control_id: u16) -> bool {
    rig.constraints.iter().any(|constraint| match *constraint {
        RigConstraint::Aim { target_control, .. }
        | RigConstraint::Distance { target_control, .. } => target_control == control_id,
        RigConstraint::TwoBoneIk {
            target_control,
            pole_control,
            ..
        } => target_control == control_id || pole_control == Some(control_id),
        RigConstraint::RotationLimit { .. }
        | RigConstraint::PositionLimit { .. }
        | RigConstraint::Transform { .. } => false,
    }) || rig.drivers.iter().any(|driver| {
        driver.source_control == control_id
            || matches!(
                driver.target,
                RigPropertyRef::ControlX(id)
                    | RigPropertyRef::ControlY(id)
                    | RigPropertyRef::ControlValue(id)
                    if id == control_id
            )
    }) || rig.channels.iter().any(|channel| {
        matches!(
            channel.property,
            RigPropertyRef::ControlX(id)
                | RigPropertyRef::ControlY(id)
                | RigPropertyRef::ControlValue(id)
                if id == control_id
        )
    }) || rig.poses.iter().any(|pose| {
        pose.values.iter().any(|value| {
            matches!(
                value.property,
                RigPropertyRef::ControlX(id)
                    | RigPropertyRef::ControlY(id)
                    | RigPropertyRef::ControlValue(id)
                    if id == control_id
            )
        })
    }) || rig.deformers.iter().any(|deformer| match deformer {
        RigDeformer::Bend {
            start_control,
            middle_control,
            end_control,
            ..
        } => [*start_control, *middle_control, *end_control].contains(&control_id),
        RigDeformer::Cage { controls, .. } => controls.contains(&control_id),
        RigDeformer::Skin { .. } => false,
    })
}

fn remove_deformer(project: &mut ProjectV2, q0rg_id: u16, deformer_id: u16) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let Some(index) = rig
        .deformers
        .iter()
        .position(|deformer| deformer.id() == deformer_id)
    else {
        return false;
    };
    let removed = rig.deformers.remove(index);
    let candidate_controls = match removed {
        RigDeformer::Bend {
            start_control,
            middle_control,
            end_control,
            ..
        } => {
            vec![start_control, middle_control, end_control]
        }
        RigDeformer::Cage { controls, .. } => controls.to_vec(),
        RigDeformer::Skin { .. } => Vec::new(),
    };
    let removable = candidate_controls
        .into_iter()
        .filter(|id| !control_is_referenced_elsewhere(rig, *id))
        .collect::<HashSet<_>>();
    rig.controls
        .retain(|control| !removable.contains(&control.control_id));
    true
}

fn normalize_skin_weights(weights: &mut Vec<RigSkinWeight>) {
    weights.retain(|weight| weight.weight.is_finite() && weight.weight > 0.0);
    if weights.len() > 4 {
        weights.sort_by(|a, b| b.weight.total_cmp(&a.weight));
        weights.truncate(4);
    }
    let sum = weights.iter().map(|weight| weight.weight).sum::<f32>();
    if sum > 1.0e-9 && sum.is_finite() {
        for weight in weights {
            weight.weight /= sum;
        }
    }
}

pub fn bind_selected_placement_to_node(
    project: &mut ProjectV2,
    selection: &Selection,
    node_id: u16,
    frame: u16,
) -> Result<(), &'static str> {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = *selection
    else {
        return Err("select a q0rg/bitmap/transformed-vector display object first");
    };
    let (instance_id, visible_transform) =
        ensure_selected_track_instance_id(project, q0rg_id, layer_id, placement_idx, frame)
            .ok_or("selected object is not active on this frame")?;
    let rig = rig_for_q0rg(project, q0rg_id).ok_or("create a rig first")?;
    if rig.nodes.iter().any(|node| {
        node.node_id != node_id
            && node
                .binding
                .is_some_and(|binding| binding.instance_id == instance_id)
    }) {
        return Err("that display object is already bound to another bone");
    }
    let pose = evaluate_rig(rig, f32::from(frame), &[]);
    let node_world = pose
        .node_world
        .get(&node_id)
        .copied()
        .ok_or("selected bone no longer exists")?;
    let inverse = node_world.inverse().ok_or("bone transform is singular")?;
    let bind_offset = Affine::compose(inverse, visible_transform);
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("create a rig first")?;
    let node = rig
        .nodes
        .iter_mut()
        .find(|node| node.node_id == node_id)
        .ok_or("selected bone no longer exists")?;
    node.binding = Some(RigBinding {
        instance_id,
        bind_offset,
    });
    Ok(())
}

pub fn unbind_node(project: &mut ProjectV2, q0rg_id: u16, node_id: u16) -> bool {
    let Some(node) = rig_for_q0rg_mut(project, q0rg_id)
        .and_then(|rig| rig.nodes.iter_mut().find(|node| node.node_id == node_id))
    else {
        return false;
    };
    node.binding.take().is_some()
}

pub fn add_rotation_control(
    project: &mut ProjectV2,
    q0rg_id: u16,
    node_id: u16,
    public_in_simple: bool,
) -> Option<u16> {
    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    let node = rig.nodes.iter().find(|node| node.node_id == node_id)?;
    if let Some(existing) = rig.controls.iter().find(|control| {
        control.kind == RigControlKind::Rotation && control.target_node == Some(node_id)
    }) {
        return Some(existing.control_id);
    }
    let node_name = node.name.clone();
    let rest_rotation = node.rest.rotation;
    let control_id = next_control_id(rig)?;
    rig.controls.push(RigControl {
        control_id,
        name: format!("{node_name} rotate"),
        kind: RigControlKind::Rotation,
        target_node: Some(node_id),
        rest_x: 0.0,
        rest_y: 0.0,
        rest_value: rest_rotation,
        min_value: -std::f32::consts::PI,
        max_value: std::f32::consts::PI,
        public_in_simple,
    });
    Some(control_id)
}

pub fn add_position_control(
    project: &mut ProjectV2,
    q0rg_id: u16,
    node_id: u16,
    public_in_simple: bool,
) -> Option<u16> {
    let rig_snapshot = rig_for_q0rg(project, q0rg_id)?.clone();
    let node = rig_snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)?;
    if let Some(existing) = rig_snapshot.controls.iter().find(|control| {
        control.kind == RigControlKind::Position2D && control.target_node == Some(node_id)
    }) {
        return Some(existing.control_id);
    }
    let control_id = next_control_id(&rig_snapshot)?;
    let rig = rig_for_q0rg_mut(project, q0rg_id)?;
    rig.controls.push(RigControl {
        control_id,
        name: format!("{} position", node.name),
        kind: RigControlKind::Position2D,
        target_node: Some(node_id),
        rest_x: node.rest.tx,
        rest_y: node.rest.ty,
        rest_value: 0.0,
        min_value: -100_000.0,
        max_value: 100_000.0,
        public_in_simple,
    });
    Some(control_id)
}

pub fn make_two_bone_ik(
    project: &mut ProjectV2,
    q0rg_id: u16,
    tip_node: u16,
    frame: u16,
) -> Result<(u16, u16, u16), &'static str> {
    let snapshot = rig_for_q0rg(project, q0rg_id)
        .ok_or("create a rig first")?
        .clone();
    let tip = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == tip_node)
        .ok_or("tip bone no longer exists")?;
    let mid_id = tip.parent.ok_or("selected tip needs a parent bone")?;
    let mid = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == mid_id)
        .ok_or("mid bone no longer exists")?;
    let root_id = mid
        .parent
        .ok_or("selected tip needs a two-bone parent chain")?;
    if snapshot.constraints.iter().any(|constraint| {
        matches!(constraint, RigConstraint::TwoBoneIk { tip_node: existing, .. } if *existing == tip_node)
    }) {
        return Err("this tip already has two-bone ik");
    }

    let pose = evaluate_rig(&snapshot, f32::from(frame), &[]);
    let tip_pos = pose
        .node_world
        .get(&tip_node)
        .ok_or("tip pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    let root_pos = pose
        .node_world
        .get(&root_id)
        .ok_or("root pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    let mid_pos = pose
        .node_world
        .get(&mid_id)
        .ok_or("mid pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    let chain_dx = tip_pos.x - root_pos.x;
    let chain_dy = tip_pos.y - root_pos.y;
    let chain_len = (chain_dx * chain_dx + chain_dy * chain_dy).sqrt().max(10.0);
    let inv_len = 1.0 / chain_len;
    let pole_pos = Vec2::new(
        mid_pos.x - chain_dy * inv_len * chain_len * 0.5,
        mid_pos.y + chain_dx * inv_len * chain_len * 0.5,
    );

    let target_control_id = next_control_id(&snapshot).ok_or("no free control ids")?;
    let pole_control_id = (target_control_id + 1..=u16::MAX)
        .find(|candidate| {
            !snapshot
                .controls
                .iter()
                .any(|control| control.control_id == *candidate)
        })
        .ok_or("no free pole control id")?;
    let constraint_id = next_constraint_id(&snapshot).ok_or("no free constraint ids")?;
    let tip_name = tip.name.clone();
    let rig = rig_for_q0rg_mut(project, q0rg_id).ok_or("create a rig first")?;
    rig.controls.push(RigControl {
        control_id: target_control_id,
        name: format!("{tip_name} IK"),
        kind: RigControlKind::Position2D,
        target_node: None,
        rest_x: tip_pos.x,
        rest_y: tip_pos.y,
        rest_value: 0.0,
        min_value: -100_000.0,
        max_value: 100_000.0,
        public_in_simple: true,
    });
    rig.controls.push(RigControl {
        control_id: pole_control_id,
        name: format!("{tip_name} pole"),
        kind: RigControlKind::Position2D,
        target_node: None,
        rest_x: pole_pos.x,
        rest_y: pole_pos.y,
        rest_value: 0.0,
        min_value: -100_000.0,
        max_value: 100_000.0,
        public_in_simple: true,
    });
    rig.constraints.push(RigConstraint::TwoBoneIk {
        constraint_id,
        root_node: root_id,
        mid_node: mid_id,
        tip_node,
        target_control: target_control_id,
        pole_control: Some(pole_control_id),
        weight: 1.0,
        allow_stretch: false,
        max_stretch: 1.0,
    });
    Ok((constraint_id, target_control_id, pole_control_id))
}
fn set_control_position(
    project: &mut ProjectV2,
    q0rg_id: u16,
    control_id: u16,
    frame: u16,
    x: f32,
    y: f32,
    auto_key: bool,
) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    if !rig
        .controls
        .iter()
        .any(|control| control.control_id == control_id)
    {
        return false;
    }
    if auto_key {
        q0s_format::rig::upsert_channel_key(
            rig,
            RigPropertyRef::ControlX(control_id),
            frame,
            x,
            Easing::Linear,
        );
        q0s_format::rig::upsert_channel_key(
            rig,
            RigPropertyRef::ControlY(control_id),
            frame,
            y,
            Easing::Linear,
        );
    } else if let Some(control) = rig
        .controls
        .iter_mut()
        .find(|control| control.control_id == control_id)
    {
        control.rest_x = x;
        control.rest_y = y;
    }
    true
}

fn set_control_value(
    project: &mut ProjectV2,
    q0rg_id: u16,
    control_id: u16,
    frame: u16,
    value: f32,
    auto_key: bool,
) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let Some(control) = rig
        .controls
        .iter()
        .find(|control| control.control_id == control_id)
    else {
        return false;
    };
    let value = value.clamp(control.min_value, control.max_value);
    if auto_key {
        q0s_format::rig::upsert_channel_key(
            rig,
            RigPropertyRef::ControlValue(control_id),
            frame,
            value,
            Easing::Linear,
        );
    } else if let Some(control) = rig
        .controls
        .iter_mut()
        .find(|control| control.control_id == control_id)
    {
        control.rest_value = value;
    }
    true
}

pub(crate) fn set_node_rotation(
    project: &mut ProjectV2,
    q0rg_id: u16,
    node_id: u16,
    frame: u16,
    rotation: f32,
    auto_key: bool,
) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    if !rig.nodes.iter().any(|node| node.node_id == node_id) {
        return false;
    }
    if let Some(control_id) = rig
        .controls
        .iter()
        .find(|control| {
            control.kind == RigControlKind::Rotation && control.target_node == Some(node_id)
        })
        .map(|control| control.control_id)
    {
        if auto_key {
            q0s_format::rig::upsert_channel_key(
                rig,
                RigPropertyRef::ControlValue(control_id),
                frame,
                rotation,
                Easing::Linear,
            );
        } else if let Some(control) = rig
            .controls
            .iter_mut()
            .find(|control| control.control_id == control_id)
        {
            control.rest_value = rotation.clamp(control.min_value, control.max_value);
        }
    } else if auto_key {
        q0s_format::rig::upsert_channel_key(
            rig,
            RigPropertyRef::NodeRotation(node_id),
            frame,
            rotation,
            Easing::Linear,
        );
    } else if let Some(node) = rig.nodes.iter_mut().find(|node| node.node_id == node_id) {
        node.rest.rotation = rotation;
    }
    true
}

fn set_node_property(
    project: &mut ProjectV2,
    q0rg_id: u16,
    node_id: u16,
    frame: u16,
    property: RigPropertyRef,
    mut value: f32,
    auto_key: bool,
) -> bool {
    let property_matches_node = matches!(
        property,
        RigPropertyRef::NodeTx(id)
            | RigPropertyRef::NodeTy(id)
            | RigPropertyRef::NodeScaleX(id)
            | RigPropertyRef::NodeScaleY(id)
            if id == node_id
    );
    if !property_matches_node || !value.is_finite() {
        return false;
    }
    if matches!(
        property,
        RigPropertyRef::NodeScaleX(_) | RigPropertyRef::NodeScaleY(_)
    ) {
        value = value.max(0.01);
    }
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let Some(node) = rig.nodes.iter().find(|node| node.node_id == node_id) else {
        return false;
    };
    if auto_key {
        q0s_format::rig::upsert_channel_key(rig, property, frame, value, Easing::Linear);
        return true;
    }
    let _ = node;
    let Some(node) = rig.nodes.iter_mut().find(|node| node.node_id == node_id) else {
        return false;
    };
    match property {
        RigPropertyRef::NodeTx(_) => node.rest.tx = value,
        RigPropertyRef::NodeTy(_) => node.rest.ty = value,
        RigPropertyRef::NodeScaleX(_) => node.rest.sx = value,
        RigPropertyRef::NodeScaleY(_) => node.rest.sy = value,
        _ => return false,
    }
    true
}

fn world_delta_to_node_parent_local(
    rig: &RigAsset,
    pose: &q0s_format::rig::RigPose,
    node_id: u16,
    delta: Vec2,
) -> Vec2 {
    let parent_world = rig
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .and_then(|node| node.parent)
        .and_then(|parent| pose.node_world.get(&parent))
        .copied()
        .unwrap_or(Affine::IDENTITY);
    let Some(inverse) = parent_world.inverse() else {
        return delta;
    };
    let zero = inverse.apply(Vec2::new(0.0, 0.0));
    let moved = inverse.apply(delta);
    Vec2::new(moved.x - zero.x, moved.y - zero.y)
}

fn node_is_descendant_of(rig: &RigAsset, candidate: u16, ancestor: u16) -> bool {
    let mut current = rig
        .nodes
        .iter()
        .find(|node| node.node_id == candidate)
        .and_then(|node| node.parent);
    let mut visited = HashSet::new();
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        if !visited.insert(parent) {
            return false;
        }
        current = rig
            .nodes
            .iter()
            .find(|node| node.node_id == parent)
            .and_then(|node| node.parent);
    }
    false
}

fn transform_target_would_cycle(
    rig: &RigAsset,
    node_id: u16,
    target_node: u16,
    ignore_constraint: Option<u16>,
) -> bool {
    if node_id == target_node {
        return true;
    }
    let mut stack = vec![target_node];
    let mut visited = HashSet::new();
    while let Some(current) = stack.pop() {
        if current == node_id {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        if let Some(parent) = rig
            .nodes
            .iter()
            .find(|node| node.node_id == current)
            .and_then(|node| node.parent)
        {
            stack.push(parent);
        }
        for constraint in &rig.constraints {
            if ignore_constraint.is_some_and(|id| constraint.id() == id) {
                continue;
            }
            if let RigConstraint::Transform {
                node_id: constrained,
                target_node: target,
                ..
            } = *constraint
            {
                if constrained == current {
                    stack.push(target);
                }
            }
        }
    }
    false
}

pub fn render_properties(app: &mut EditorApp, ui: &mut Ui) {
    let q0rg_id = app.session.current_q0rg_id;
    ui.horizontal(|ui| {
        ui.label("Mode");
        for mode in RigMode::ALL {
            ui.selectable_value(&mut app.session.rig_mode, mode, mode.label());
        }
    });
    ui.label(
        egui::RichText::new(match app.session.rig_mode {
            RigMode::Simple => {
                "fast cutout posing: draw bones, bind parts, add IK, animate controls"
            }
            RigMode::Pro => {
                "full construction: hierarchy, controls, constraints and numeric rest data"
            }
        })
        .small()
        .color(app.settings.theme.text_dim.to_color32()),
    );

    if rig_for_q0rg(&app.state.project, q0rg_id).is_none() {
        ui.add_space(6.0);
        if ui.button("Create rig").clicked() {
            let before = app.state.project.clone();
            if ensure_rig(&mut app.state.project, q0rg_id).is_some() {
                app.history.snapshot(&before);
                app.state.mark_dirty();
                app.session.status = "rig created".into();
            }
        }
        return;
    }

    ui.separator();
    ui.horizontal(|ui| {
        for mode in [RigEditMode::Pose, RigEditMode::AddBone] {
            ui.selectable_value(&mut app.session.rig_edit_mode, mode, mode.label());
        }
    });
    ui.checkbox(&mut app.session.rig_auto_key, "Auto-key controls");
    if !app.session.rig_auto_key {
        ui.label(
            egui::RichText::new("auto-key off: dragging edits the control's default/rest value")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }

    let (node_count, control_count, constraint_count) = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| (rig.nodes.len(), rig.controls.len(), rig.constraints.len()))
        .unwrap_or_default();
    ui.label(format!(
        "{node_count} bones / {control_count} controls / {constraint_count} constraints"
    ));

    if app.session.rig_mode == RigMode::Simple {
        ui.separator();
        render_simple_public_controls(app, ui, q0rg_id);
    }

    ui.separator();
    render_bone_selection(app, ui, q0rg_id);
    if app.session.rig_mode == RigMode::Pro {
        ui.separator();
        render_pro_controls(app, ui, q0rg_id);
        ui.separator();
        render_pro_constraints(app, ui, q0rg_id);
        ui.separator();
        render_pro_deformers(app, ui, q0rg_id);
        ui.separator();
        render_pro_pose_metadata(app, ui, q0rg_id);
        ui.separator();
        render_pro_variants(app, ui, q0rg_id);
        ui.separator();
        render_pro_graph(app, ui, q0rg_id);
    }

    ui.separator();
    render_pose_library(app, ui, q0rg_id);

    ui.separator();
    if ui
        .button("Delete rig...")
        .on_hover_text("Removes rig metadata only. Artwork and timeline placements stay intact.")
        .clicked()
    {
        let before = app.state.project.clone();
        if delete_rig(&mut app.state.project, q0rg_id) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.rig_selected_node = None;
            app.session.rig_selected_control = None;
            app.session.status = "rig removed; artwork kept".into();
        }
    }
}

fn render_bone_selection(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    let nodes = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| {
            rig.nodes
                .iter()
                .map(|node| {
                    (
                        node.node_id,
                        node.name.clone(),
                        node.parent,
                        node.binding.is_some(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ui.label(egui::RichText::new("Bones").strong());
    if nodes.is_empty() {
        ui.label("draw the first bone on the stage with Add bone");
        return;
    }
    for (id, name, parent, bound) in &nodes {
        let indent = parent.map(|_| "  > ").unwrap_or("");
        let label = format!("{indent}{name}{}", if *bound { "  [bound]" } else { "" });
        if ui
            .selectable_label(app.session.rig_selected_node == Some(*id), label)
            .clicked()
        {
            app.session.rig_selected_node = Some(*id);
            if app.session.rig_mode == RigMode::Simple {
                app.session.rig_selected_control = None;
            }
        }
    }

    let Some(node_id) = app.session.rig_selected_node else {
        return;
    };
    ui.add_space(4.0);
    let mut bind = false;
    let mut unbind = false;
    let mut add_rotation = false;
    let mut add_position = false;
    let mut add_ik = false;
    let mut delete = false;
    ui.horizontal_wrapped(|ui| {
        bind = ui.button("Bind selected object").clicked();
        unbind = ui.button("Unbind").clicked();
        add_ik = ui.button("Make 2-bone IK").clicked();
    });
    if app.session.rig_mode == RigMode::Pro {
        ui.horizontal_wrapped(|ui| {
            add_rotation = ui.button("+ rotation control").clicked();
            add_position = ui.button("+ position control").clicked();
            delete = ui.button("Delete bone subtree").clicked();
        });
    }

    if bind {
        let before = app.state.project.clone();
        match bind_selected_placement_to_node(
            &mut app.state.project,
            &app.session.selection,
            node_id,
            app.session.current_frame,
        ) {
            Ok(()) => {
                app.history.snapshot(&before);
                app.state.mark_dirty();
                app.session.status = "object bound to bone".into();
            }
            Err(message) => app.session.status = message.into(),
        }
    }
    if unbind {
        let before = app.state.project.clone();
        if unbind_node(&mut app.state.project, q0rg_id, node_id) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
        }
    }
    if add_rotation {
        let before = app.state.project.clone();
        if let Some(id) = add_rotation_control(&mut app.state.project, q0rg_id, node_id, true) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.rig_selected_control = Some(id);
        }
    }
    if add_position {
        let before = app.state.project.clone();
        if let Some(id) = add_position_control(&mut app.state.project, q0rg_id, node_id, true) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.rig_selected_control = Some(id);
        }
    }
    if add_ik {
        let before = app.state.project.clone();
        match make_two_bone_ik(
            &mut app.state.project,
            q0rg_id,
            node_id,
            app.session.current_frame,
        ) {
            Ok((_, target, _)) => {
                app.history.snapshot(&before);
                app.state.mark_dirty();
                app.session.rig_selected_control = Some(target);
                app.session.status = "two-bone IK created".into();
            }
            Err(message) => app.session.status = message.into(),
        }
    }
    if delete {
        let before = app.state.project.clone();
        if remove_node_subtree(&mut app.state.project, q0rg_id, node_id) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.rig_selected_node = None;
            app.session.rig_selected_control = None;
        }
    }

    if app.session.rig_mode == RigMode::Pro {
        render_selected_node_numeric(app, ui, q0rg_id, node_id);
    }
}
fn render_selected_node_numeric(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, node_id: u16) {
    let before = app.state.project.clone();
    let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) else {
        return;
    };
    let Some(index) = rig.nodes.iter().position(|node| node.node_id == node_id) else {
        return;
    };
    let forbidden_parents = rig
        .nodes
        .iter()
        .filter(|candidate| node_is_descendant_of(rig, candidate.node_id, node_id))
        .map(|candidate| candidate.node_id)
        .collect::<HashSet<_>>();
    let node_ids = rig
        .nodes
        .iter()
        .map(|node| (node.node_id, node.name.clone()))
        .collect::<Vec<_>>();
    let node = &mut rig.nodes[index];
    let mut changed = false;
    ui.add_space(5.0);
    ui.label(egui::RichText::new("Bone properties").strong().small());
    changed |= ui.text_edit_singleline(&mut node.name).changed();
    egui::Grid::new("rig_node_numeric")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Parent");
            let parent_name = node
                .parent
                .and_then(|id| node_ids.iter().find(|(candidate, _)| *candidate == id))
                .map(|(_, name)| name.as_str())
                .unwrap_or("<root>");
            egui::ComboBox::from_id_source("rig_node_parent")
                .selected_text(parent_name)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut node.parent, None, "<root>")
                        .changed()
                    {
                        changed = true;
                    }
                    for (id, name) in &node_ids {
                        if *id != node_id
                            && !forbidden_parents.contains(id)
                            && ui
                                .selectable_value(&mut node.parent, Some(*id), name)
                                .changed()
                        {
                            changed = true;
                        }
                    }
                });
            ui.end_row();
            ui.label("Rest X");
            changed |= ui
                .add(egui::DragValue::new(&mut node.rest.tx).speed(0.25))
                .changed();
            ui.end_row();
            ui.label("Rest Y");
            changed |= ui
                .add(egui::DragValue::new(&mut node.rest.ty).speed(0.25))
                .changed();
            ui.end_row();
            ui.label("Scale X");
            changed |= ui
                .add(egui::DragValue::new(&mut node.rest.sx).speed(0.01))
                .changed();
            ui.end_row();
            ui.label("Scale Y");
            changed |= ui
                .add(egui::DragValue::new(&mut node.rest.sy).speed(0.01))
                .changed();
            ui.end_row();
            ui.label("Length");
            changed |= ui
                .add(egui::DragValue::new(&mut node.length).speed(0.25))
                .changed();
            ui.end_row();
            ui.label("Rotation");
            let mut degrees = node.rest.rotation.to_degrees();
            if ui
                .add(egui::DragValue::new(&mut degrees).suffix(" deg"))
                .changed()
            {
                node.rest.rotation = degrees.to_radians();
                changed = true;
            }
            ui.end_row();
        });
    if changed {
        node.rest.sx = node.rest.sx.max(0.01);
        node.rest.sy = node.rest.sy.max(0.01);
        node.length = node.length.max(0.0);
        app.history.snapshot(&before);
        app.state.mark_dirty();
    }
}

fn render_pro_controls(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    ui.label(egui::RichText::new("Control builder").strong());
    ui.horizontal_wrapped(|ui| {
        let selected_node = app.session.rig_selected_node;
        for (label, kind, target) in [
            ("+ joystick", RigControlKind::Position2D, None),
            ("+ rotation dial", RigControlKind::Rotation, selected_node),
            ("+ slider", RigControlKind::Slider, None),
            ("+ toggle", RigControlKind::Toggle, None),
        ] {
            let enabled = kind != RigControlKind::Rotation || target.is_some();
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                let before = app.state.project.clone();
                let created = if kind == RigControlKind::Slider {
                    add_master_slider(&mut app.state.project, q0rg_id)
                } else {
                    add_control(&mut app.state.project, q0rg_id, kind, target)
                };
                if let Some(control_id) = created {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                    app.session.rig_selected_control = Some(control_id);
                    app.session.status = format!("{} created", label.trim_start_matches("+ "));
                }
            }
        }
    });
    if app.session.rig_selected_node.is_none() {
        ui.label(
            egui::RichText::new("select a bone to create a direct rotation dial")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Controls").strong());
    let controls = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| {
            rig.controls
                .iter()
                .map(|control| (control.control_id, control.name.clone(), control.kind))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (id, name, kind) in controls {
        if ui
            .selectable_label(
                app.session.rig_selected_control == Some(id),
                format!("{name}  [{kind:?}]"),
            )
            .clicked()
        {
            app.session.rig_selected_control = Some(id);
        }
    }

    let Some(control_id) = app.session.rig_selected_control else {
        return;
    };
    let selected_kind = rig_for_q0rg(&app.state.project, q0rg_id)
        .and_then(|rig| {
            rig.controls
                .iter()
                .find(|control| control.control_id == control_id)
        })
        .map(|control| control.kind);
    let before = app.state.project.clone();
    let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) else {
        return;
    };
    let Some(index) = rig
        .controls
        .iter()
        .position(|control| control.control_id == control_id)
    else {
        return;
    };
    let mut changed = false;
    let delete;
    {
        let control = &mut rig.controls[index];
        changed |= ui.text_edit_singleline(&mut control.name).changed();
        changed |= ui
            .checkbox(&mut control.public_in_simple, "Visible in Simple")
            .changed();
        egui::Grid::new("rig_control_numeric")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Rest X");
                changed |= ui.add(egui::DragValue::new(&mut control.rest_x)).changed();
                ui.end_row();
                ui.label("Rest Y");
                changed |= ui.add(egui::DragValue::new(&mut control.rest_y)).changed();
                ui.end_row();
                ui.label("Default value");
                changed |= ui
                    .add(egui::DragValue::new(&mut control.rest_value).speed(0.01))
                    .changed();
                ui.end_row();
                ui.label("Min");
                changed |= ui
                    .add(egui::DragValue::new(&mut control.min_value).speed(0.01))
                    .changed();
                ui.end_row();
                ui.label("Max");
                changed |= ui
                    .add(egui::DragValue::new(&mut control.max_value).speed(0.01))
                    .changed();
                ui.end_row();
            });
        delete = ui.button("Delete control").clicked();
    }
    if delete {
        rig.controls
            .retain(|control| control.control_id != control_id);
        rig.constraints.retain(|constraint| match *constraint {
            RigConstraint::RotationLimit { .. }
            | RigConstraint::PositionLimit { .. }
            | RigConstraint::Transform { .. } => true,
            RigConstraint::Aim { target_control, .. }
            | RigConstraint::Distance { target_control, .. } => target_control != control_id,
            RigConstraint::TwoBoneIk {
                target_control,
                pole_control,
                ..
            } => target_control != control_id && pole_control != Some(control_id),
        });
        let remaining_constraints = rig
            .constraints
            .iter()
            .map(RigConstraint::id)
            .collect::<HashSet<_>>();
        rig.channels.retain(|channel| match channel.property {
            RigPropertyRef::ControlX(id)
            | RigPropertyRef::ControlY(id)
            | RigPropertyRef::ControlValue(id) => id != control_id,
            RigPropertyRef::NodeTx(_)
            | RigPropertyRef::NodeTy(_)
            | RigPropertyRef::NodeRotation(_)
            | RigPropertyRef::NodeScaleX(_)
            | RigPropertyRef::NodeScaleY(_) => true,
            RigPropertyRef::ConstraintWeight(id) => remaining_constraints.contains(&id),
        });
        rig.drivers.retain(|driver| {
            driver.source_control != control_id
                && !matches!(
                    driver.target,
                    RigPropertyRef::ControlX(id)
                        | RigPropertyRef::ControlY(id)
                        | RigPropertyRef::ControlValue(id)
                        if id == control_id
                )
                && !matches!(driver.target, RigPropertyRef::ConstraintWeight(id) if !remaining_constraints.contains(&id))
        });
        for pose in &mut rig.poses {
            pose.values.retain(|value| {
                !matches!(
                    value.property,
                    RigPropertyRef::ControlX(id)
                        | RigPropertyRef::ControlY(id)
                        | RigPropertyRef::ControlValue(id)
                        if id == control_id
                )
                    && !matches!(value.property, RigPropertyRef::ConstraintWeight(id) if !remaining_constraints.contains(&id))
            });
        }
        app.session.rig_selected_control = None;
        changed = true;
    }
    if changed {
        if let Some(control) = rig
            .controls
            .iter_mut()
            .find(|control| control.control_id == control_id)
        {
            if control.min_value > control.max_value {
                std::mem::swap(&mut control.min_value, &mut control.max_value);
            }
            control.rest_value = control
                .rest_value
                .clamp(control.min_value, control.max_value);
        }
        app.history.snapshot(&before);
        app.state.mark_dirty();
    }
    if matches!(
        selected_kind,
        Some(RigControlKind::Slider | RigControlKind::Toggle)
    ) && app.session.rig_selected_control == Some(control_id)
    {
        render_master_driver_editor(app, ui, q0rg_id, control_id);
    }
}

fn render_simple_public_controls(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    let Some(rig) = rig_for_q0rg(&app.state.project, q0rg_id) else {
        return;
    };
    let pose = evaluate_rig(rig, f32::from(app.session.current_frame), &[]);
    let controls = rig
        .controls
        .iter()
        .filter(|control| {
            control.public_in_simple
                && matches!(
                    control.kind,
                    RigControlKind::Slider | RigControlKind::Toggle
                )
        })
        .map(|control| {
            let value = pose
                .controls
                .get(&control.control_id)
                .map(|value| value.value)
                .unwrap_or(control.rest_value);
            (
                control.control_id,
                control.name.clone(),
                control.kind,
                control.min_value,
                control.max_value,
                value,
            )
        })
        .collect::<Vec<_>>();
    if controls.is_empty() {
        return;
    }
    ui.label(egui::RichText::new("Animator controls").strong());
    for (control_id, name, kind, min_value, max_value, mut value) in controls {
        let before = app.state.project.clone();
        let response = match kind {
            RigControlKind::Slider => {
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.add(
                        egui::Slider::new(&mut value, min_value..=max_value)
                            .show_value(true)
                            .clamp_to_range(true),
                    )
                })
                .inner
            }
            RigControlKind::Toggle => {
                let mut enabled = value >= 0.5;
                let response = ui.checkbox(&mut enabled, name);
                value = if enabled { 1.0 } else { 0.0 };
                response
            }
            RigControlKind::Position2D | RigControlKind::Rotation => continue,
        };
        if response.changed()
            && set_control_value(
                &mut app.state.project,
                q0rg_id,
                control_id,
                app.session.current_frame,
                value,
                app.session.rig_auto_key,
            )
        {
            if response.drag_started() || response.gained_focus() || kind == RigControlKind::Toggle
            {
                app.history.snapshot(&before);
            }
            app.state.mark_dirty();
        }
    }
}

fn rig_property_label(rig: &RigAsset, property: RigPropertyRef) -> String {
    let node_name = |id: u16| {
        rig.nodes
            .iter()
            .find(|node| node.node_id == id)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| format!("bone {id}"))
    };
    match property {
        RigPropertyRef::NodeTx(id) => format!("{} x", node_name(id)),
        RigPropertyRef::NodeTy(id) => format!("{} y", node_name(id)),
        RigPropertyRef::NodeRotation(id) => format!("{} rotation", node_name(id)),
        RigPropertyRef::NodeScaleX(id) => format!("{} scale x", node_name(id)),
        RigPropertyRef::NodeScaleY(id) => format!("{} scale y", node_name(id)),
        RigPropertyRef::ConstraintWeight(id) => format!("constraint #{id} weight"),
        RigPropertyRef::ControlX(id) => format!("control #{id} x"),
        RigPropertyRef::ControlY(id) => format!("control #{id} y"),
        RigPropertyRef::ControlValue(id) => format!("control #{id} value"),
    }
}

fn render_master_driver_editor(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16, control_id: u16) {
    let Some(rig_snapshot) = rig_for_q0rg(&app.state.project, q0rg_id).cloned() else {
        return;
    };
    let Some(control) = rig_snapshot
        .controls
        .iter()
        .find(|control| control.control_id == control_id)
    else {
        return;
    };
    if !matches!(
        control.kind,
        RigControlKind::Slider | RigControlKind::Toggle
    ) {
        return;
    }

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Master mappings").strong().small());
    let current_value = evaluate_rig(&rig_snapshot, f32::from(app.session.current_frame), &[])
        .controls
        .get(&control_id)
        .map(|value| value.value)
        .unwrap_or(control.rest_value);
    let mut value = current_value;
    let before_value = app.state.project.clone();
    let response = if control.kind == RigControlKind::Toggle {
        let mut enabled = value >= 0.5;
        let response = ui.checkbox(&mut enabled, "Enabled");
        value = if enabled { 1.0 } else { 0.0 };
        response
    } else {
        ui.add(
            egui::Slider::new(&mut value, control.min_value..=control.max_value)
                .text("Value")
                .clamp_to_range(true),
        )
    };
    if response.changed()
        && set_control_value(
            &mut app.state.project,
            q0rg_id,
            control_id,
            app.session.current_frame,
            value,
            app.session.rig_auto_key,
        )
    {
        if response.drag_started() || response.gained_focus() {
            app.history.snapshot(&before_value);
        }
        app.state.mark_dirty();
    }

    let drivers = rig_snapshot
        .drivers
        .iter()
        .filter(|driver| driver.source_control == control_id)
        .copied()
        .collect::<Vec<_>>();
    if drivers.is_empty() {
        ui.label(
            egui::RichText::new("no mappings yet; add a bone rotation or IK weight below")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }
    for driver in drivers {
        let mut source_min = driver.source_min;
        let mut source_max = driver.source_max;
        let mut target_min = driver.target_min;
        let mut target_max = driver.target_max;
        let mut changed = false;
        let mut delete = false;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(rig_property_label(&rig_snapshot, driver.target));
                delete = ui
                    .small_button("x")
                    .on_hover_text("Delete mapping")
                    .clicked();
            });
            egui::Grid::new(format!("rig_driver_{}", driver.driver_id))
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Input min");
                    changed |= ui
                        .add(egui::DragValue::new(&mut source_min).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Input max");
                    changed |= ui
                        .add(egui::DragValue::new(&mut source_max).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Output min");
                    changed |= ui
                        .add(egui::DragValue::new(&mut target_min).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Output max");
                    changed |= ui
                        .add(egui::DragValue::new(&mut target_max).speed(0.01))
                        .changed();
                    ui.end_row();
                });
        });
        if delete || changed {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                if delete {
                    rig.drivers
                        .retain(|candidate| candidate.driver_id != driver.driver_id);
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                } else if let Some(candidate) = rig
                    .drivers
                    .iter_mut()
                    .find(|candidate| candidate.driver_id == driver.driver_id)
                {
                    if (source_max - source_min).abs() < 1.0e-4 {
                        source_max = source_min + 1.0e-4;
                    }
                    candidate.source_min = source_min;
                    candidate.source_max = source_max;
                    candidate.target_min = target_min;
                    candidate.target_max = target_max;
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
    }

    let used_targets = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| {
            rig.drivers
                .iter()
                .map(|driver| driver.target)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if let Some(node_id) = app.session.rig_selected_node {
        if let Some(node) = rig_snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
        {
            let targets = [
                (
                    RigPropertyRef::NodeTx(node_id),
                    node.rest.tx - 100.0,
                    node.rest.tx + 100.0,
                ),
                (
                    RigPropertyRef::NodeTy(node_id),
                    node.rest.ty - 100.0,
                    node.rest.ty + 100.0,
                ),
                (
                    RigPropertyRef::NodeRotation(node_id),
                    node.rest.rotation - 45.0_f32.to_radians(),
                    node.rest.rotation + 45.0_f32.to_radians(),
                ),
                (RigPropertyRef::NodeScaleX(node_id), 0.5, 1.5),
                (RigPropertyRef::NodeScaleY(node_id), 0.5, 1.5),
            ];
            for (target, target_min, target_max) in targets {
                if used_targets.contains(&target) {
                    continue;
                }
                if ui
                    .button(format!(
                        "+ drive {}",
                        rig_property_label(&rig_snapshot, target)
                    ))
                    .clicked()
                {
                    let before = app.state.project.clone();
                    match add_master_driver(
                        &mut app.state.project,
                        q0rg_id,
                        control_id,
                        target,
                        target_min,
                        target_max,
                    ) {
                        Ok(_) => {
                            app.history.snapshot(&before);
                            app.state.mark_dirty();
                        }
                        Err(message) => app.session.status = message.into(),
                    }
                }
            }
        }
    }
    let weighted_constraints = rig_snapshot
        .constraints
        .iter()
        .filter_map(|constraint| match constraint {
            RigConstraint::TwoBoneIk { constraint_id, .. }
            | RigConstraint::Aim { constraint_id, .. }
            | RigConstraint::Distance { constraint_id, .. } => Some(*constraint_id),
            RigConstraint::RotationLimit { .. }
            | RigConstraint::PositionLimit { .. }
            | RigConstraint::Transform { .. } => None,
        })
        .collect::<Vec<_>>();
    for constraint_id in weighted_constraints {
        let target = RigPropertyRef::ConstraintWeight(constraint_id);
        if used_targets.contains(&target) {
            continue;
        }
        if ui
            .button(format!("+ drive constraint #{constraint_id} weight"))
            .clicked()
        {
            let before = app.state.project.clone();
            match add_master_driver(
                &mut app.state.project,
                q0rg_id,
                control_id,
                target,
                0.0,
                1.0,
            ) {
                Ok(_) => {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
                Err(message) => app.session.status = message.into(),
            }
        }
    }
}

fn render_pose_library(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    if app.session.rig_mode == RigMode::Pro {
        ui.horizontal(|ui| {
            ui.label("Blend");
            ui.add(
                egui::Slider::new(&mut app.session.rig_pose_blend_weight, 0.0..=1.0)
                    .show_value(true),
            );
            egui::ComboBox::from_id_source("rig_pose_blend_mode")
                .selected_text(match app.session.rig_pose_blend_mode {
                    RigPoseBlendMode::Override => "Override",
                    RigPoseBlendMode::Additive => "Additive",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut app.session.rig_pose_blend_mode,
                        RigPoseBlendMode::Override,
                        "Override",
                    );
                    ui.selectable_value(
                        &mut app.session.rig_pose_blend_mode,
                        RigPoseBlendMode::Additive,
                        "Additive",
                    );
                });
        });
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Poses").strong());
        if ui.small_button("Save current").clicked() {
            let before = app.state.project.clone();
            if let Some(pose_id) =
                save_pose(&mut app.state.project, q0rg_id, app.session.current_frame)
            {
                app.history.snapshot(&before);
                app.state.mark_dirty();
                app.session.status = format!("pose {pose_id} saved");
            }
        }
    });
    let poses = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| rig.poses.clone())
        .unwrap_or_default();
    if poses.is_empty() {
        ui.label(
            egui::RichText::new("save a reusable rig pose from the current frame")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        return;
    }
    for pose in poses {
        let mut name = pose.name.clone();
        let mut apply = false;
        let mut replace = false;
        let mut delete = false;
        let mut rename = false;
        let mut blend = false;
        let mut mirror = false;
        let mut drive = false;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                rename = ui.text_edit_singleline(&mut name).changed();
                apply = ui.button("Apply").clicked();
                if app.session.rig_mode == RigMode::Pro {
                    blend = ui
                        .small_button("Blend")
                        .on_hover_text("Apply with the blend weight above")
                        .clicked();
                    mirror = ui
                        .small_button("Mirror")
                        .on_hover_text("Apply through explicit mirror-pair metadata")
                        .clicked();
                    drive = ui
                        .small_button("+ driver")
                        .on_hover_text("Drive this pose from the selected scalar control")
                        .clicked();
                }
                replace = ui
                    .small_button("Update")
                    .on_hover_text("Replace this pose with the current rig pose")
                    .clicked();
                delete = ui.small_button("x").on_hover_text("Delete pose").clicked();
            });
        });
        if rename && !name.trim().is_empty() {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                if let Some(current) = rig
                    .poses
                    .iter_mut()
                    .find(|item| item.pose_id == pose.pose_id)
                {
                    current.name = name;
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
        if apply {
            let before = app.state.project.clone();
            match apply_pose(
                &mut app.state.project,
                q0rg_id,
                pose.pose_id,
                app.session.current_frame,
                app.session.rig_auto_key,
            ) {
                Ok(()) => {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                    app.session.status = format!("pose '{}' applied", pose.name);
                }
                Err(message) => app.session.status = message.into(),
            }
        }
        if blend {
            let before = app.state.project.clone();
            let values = rig_for_q0rg(&app.state.project, q0rg_id).and_then(|rig| {
                blended_pose_values(
                    rig,
                    f32::from(app.session.current_frame),
                    pose.pose_id,
                    app.session.rig_pose_blend_weight,
                    app.session.rig_pose_blend_mode,
                )
            });
            if let Some(values) = values {
                match apply_pose_values(
                    &mut app.state.project,
                    q0rg_id,
                    &values,
                    app.session.current_frame,
                    app.session.rig_auto_key,
                ) {
                    Ok(()) => {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                        app.session.status = format!(
                            "pose '{}' blended at {:.0}%",
                            pose.name,
                            app.session.rig_pose_blend_weight * 100.0
                        );
                    }
                    Err(message) => app.session.status = message.into(),
                }
            }
        }
        if mirror {
            let before = app.state.project.clone();
            let values = rig_for_q0rg(&app.state.project, q0rg_id)
                .and_then(|rig| mirror_pose_values(rig, pose.pose_id));
            if let Some(values) = values {
                match apply_pose_values(
                    &mut app.state.project,
                    q0rg_id,
                    &values,
                    app.session.current_frame,
                    app.session.rig_auto_key,
                ) {
                    Ok(()) => {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                        app.session.status = format!("pose '{}' mirrored", pose.name);
                    }
                    Err(message) => app.session.status = message.into(),
                }
            }
        }
        if drive {
            let source = app.session.rig_selected_control;
            let before = app.state.project.clone();
            match source {
                Some(source_control) => match add_pose_driver(
                    &mut app.state.project,
                    q0rg_id,
                    source_control,
                    pose.pose_id,
                    app.session.rig_pose_blend_mode,
                ) {
                    Ok(id) => {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                        app.session.status = format!("pose driver #{id} created");
                    }
                    Err(message) => app.session.status = message.into(),
                },
                None => app.session.status = "select a slider or toggle first".into(),
            }
        }

        if replace {
            let before = app.state.project.clone();
            let values = rig_for_q0rg(&app.state.project, q0rg_id).map(|rig| {
                q0s_format::rig::capture_pose_values(rig, f32::from(app.session.current_frame))
            });
            if let (Some(values), Some(rig)) =
                (values, rig_for_q0rg_mut(&mut app.state.project, q0rg_id))
            {
                if let Some(current) = rig
                    .poses
                    .iter_mut()
                    .find(|item| item.pose_id == pose.pose_id)
                {
                    current.values = values;
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                    app.session.status = format!("pose '{}' updated", pose.name);
                }
            }
        }
        if delete {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                let count = rig.poses.len();
                rig.poses.retain(|item| item.pose_id != pose.pose_id);
                if rig.poses.len() != count {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
    }
}

fn set_constraint_weight(
    project: &mut ProjectV2,
    q0rg_id: u16,
    constraint_id: u16,
    frame: u16,
    weight: f32,
    auto_key: bool,
) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let weight = weight.clamp(0.0, 1.0);
    if !rig
        .constraints
        .iter()
        .any(|constraint| constraint.id() == constraint_id)
    {
        return false;
    }
    if auto_key {
        q0s_format::rig::upsert_channel_key(
            rig,
            RigPropertyRef::ConstraintWeight(constraint_id),
            frame,
            weight,
            Easing::Linear,
        );
    } else if let Some(constraint) = rig
        .constraints
        .iter_mut()
        .find(|constraint| constraint.id() == constraint_id)
    {
        match constraint {
            RigConstraint::TwoBoneIk {
                weight: current, ..
            }
            | RigConstraint::Aim {
                weight: current, ..
            }
            | RigConstraint::Distance {
                weight: current, ..
            } => *current = weight,
            RigConstraint::RotationLimit { .. }
            | RigConstraint::PositionLimit { .. }
            | RigConstraint::Transform { .. } => return false,
        }
    } else {
        return false;
    }
    true
}

fn set_rotation_limit(
    project: &mut ProjectV2,
    q0rg_id: u16,
    constraint_id: u16,
    min_radians: f32,
    max_radians: f32,
) -> bool {
    let Some(RigConstraint::RotationLimit {
        min_radians: current_min,
        max_radians: current_max,
        ..
    }) = rig_for_q0rg_mut(project, q0rg_id).and_then(|rig| {
        rig.constraints
            .iter_mut()
            .find(|constraint| constraint.id() == constraint_id)
    })
    else {
        return false;
    };
    *current_min = min_radians.min(max_radians);
    *current_max = max_radians.max(min_radians);
    true
}

fn replace_constraint(project: &mut ProjectV2, q0rg_id: u16, replacement: RigConstraint) -> bool {
    let id = replacement.id();
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let Some(slot) = rig
        .constraints
        .iter_mut()
        .find(|constraint| constraint.id() == id)
    else {
        return false;
    };
    *slot = replacement;
    true
}

fn set_ik_settings(
    project: &mut ProjectV2,
    q0rg_id: u16,
    constraint_id: u16,
    allow_stretch: bool,
    max_stretch: f32,
) -> bool {
    let Some(RigConstraint::TwoBoneIk {
        allow_stretch: current_allow,
        max_stretch: current_max,
        ..
    }) = rig_for_q0rg_mut(project, q0rg_id).and_then(|rig| {
        rig.constraints
            .iter_mut()
            .find(|constraint| constraint.id() == constraint_id)
    })
    else {
        return false;
    };
    *current_allow = allow_stretch;
    *current_max = max_stretch.max(1.0);
    true
}

fn remove_constraint(project: &mut ProjectV2, q0rg_id: u16, constraint_id: u16) -> bool {
    let Some(rig) = rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let before = rig.constraints.len();
    rig.constraints
        .retain(|constraint| constraint.id() != constraint_id);
    if before == rig.constraints.len() {
        return false;
    }
    rig.channels
        .retain(|channel| channel.property != RigPropertyRef::ConstraintWeight(constraint_id));
    rig.drivers
        .retain(|driver| driver.target != RigPropertyRef::ConstraintWeight(constraint_id));
    for pose in &mut rig.poses {
        pose.values
            .retain(|value| value.property != RigPropertyRef::ConstraintWeight(constraint_id));
    }
    true
}

fn match_ik_target_to_fk(
    project: &mut ProjectV2,
    q0rg_id: u16,
    constraint_id: u16,
    frame: u16,
    auto_key: bool,
) -> Result<(), &'static str> {
    let mut rig = rig_for_q0rg(project, q0rg_id)
        .ok_or("rig is missing")?
        .clone();
    let (root_node, mid_node, tip_node, target_control, pole_control) = rig
        .constraints
        .iter()
        .find_map(|constraint| match *constraint {
            RigConstraint::TwoBoneIk {
                constraint_id: id,
                root_node,
                mid_node,
                tip_node,
                target_control,
                pole_control,
                ..
            } if id == constraint_id => {
                Some((root_node, mid_node, tip_node, target_control, pole_control))
            }
            _ => None,
        })
        .ok_or("IK constraint is missing")?;
    if let Some(RigConstraint::TwoBoneIk { weight, .. }) = rig
        .constraints
        .iter_mut()
        .find(|constraint| constraint.id() == constraint_id)
    {
        *weight = 0.0;
    }
    rig.channels
        .retain(|channel| channel.property != RigPropertyRef::ConstraintWeight(constraint_id));
    let pose = evaluate_rig(&rig, f32::from(frame), &[]);
    let root = pose
        .node_world
        .get(&root_node)
        .ok_or("root pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    let mid = pose
        .node_world
        .get(&mid_node)
        .ok_or("mid pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    let tip = pose
        .node_world
        .get(&tip_node)
        .ok_or("tip pose is unavailable")?
        .apply(Vec2::new(0.0, 0.0));
    if !set_control_position(
        project,
        q0rg_id,
        target_control,
        frame,
        tip.x,
        tip.y,
        auto_key,
    ) {
        return Err("IK target control is missing");
    }
    if let Some(pole_control) = pole_control {
        let dx = tip.x - root.x;
        let dy = tip.y - root.y;
        let length = (dx * dx + dy * dy).sqrt().max(1.0);
        let cross = dx * (mid.y - root.y) - dy * (mid.x - root.x);
        let sign = if cross < 0.0 { -1.0 } else { 1.0 };
        let pole = Vec2::new(
            mid.x - dy / length * length * 0.5 * sign,
            mid.y + dx / length * length * 0.5 * sign,
        );
        if !set_control_position(
            project,
            q0rg_id,
            pole_control,
            frame,
            pole.x,
            pole.y,
            auto_key,
        ) {
            return Err("IK pole control is missing");
        }
    }
    Ok(())
}

fn bake_ik_pose_to_fk(
    project: &mut ProjectV2,
    q0rg_id: u16,
    constraint_id: u16,
    frame: u16,
    auto_key: bool,
) -> Result<(), &'static str> {
    let mut rig = rig_for_q0rg(project, q0rg_id)
        .ok_or("rig is missing")?
        .clone();
    let (root_node, mid_node) = rig
        .constraints
        .iter()
        .find_map(|constraint| match *constraint {
            RigConstraint::TwoBoneIk {
                constraint_id: id,
                root_node,
                mid_node,
                ..
            } if id == constraint_id => Some((root_node, mid_node)),
            _ => None,
        })
        .ok_or("IK constraint is missing")?;
    if let Some(RigConstraint::TwoBoneIk { weight, .. }) = rig
        .constraints
        .iter_mut()
        .find(|constraint| constraint.id() == constraint_id)
    {
        *weight = 1.0;
    }
    rig.channels
        .retain(|channel| channel.property != RigPropertyRef::ConstraintWeight(constraint_id));
    let pose = evaluate_rig(&rig, f32::from(frame), &[]);
    let root_rotation = pose
        .node_local
        .get(&root_node)
        .ok_or("root local pose is unavailable")?
        .rotation;
    let mid_rotation = pose
        .node_local
        .get(&mid_node)
        .ok_or("mid local pose is unavailable")?
        .rotation;
    if !set_node_rotation(project, q0rg_id, root_node, frame, root_rotation, auto_key)
        || !set_node_rotation(project, q0rg_id, mid_node, frame, mid_rotation, auto_key)
    {
        return Err("FK bones are missing");
    }
    Ok(())
}

fn render_pro_constraints(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    ui.label(egui::RichText::new("Constraints").strong());
    let constraints = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| rig.constraints.clone())
        .unwrap_or_default();
    let pose = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| evaluate_rig(rig, f32::from(app.session.current_frame), &[]));
    let node_choices = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| {
            rig.nodes
                .iter()
                .map(|node| (node.node_id, node.name.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let target_controls = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| {
            rig.controls
                .iter()
                .filter(|control| {
                    control.kind == RigControlKind::Position2D && control.target_node.is_none()
                })
                .map(|control| (control.control_id, control.name.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if constraints.is_empty() {
        ui.label("none");
    }

    for constraint in constraints {
        match constraint {
            RigConstraint::RotationLimit {
                constraint_id,
                node_id,
                min_radians,
                max_radians,
            } => {
                let before = app.state.project.clone();
                let mut min_degrees = min_radians.to_degrees();
                let mut max_degrees = max_radians.to_degrees();
                let mut changed = false;
                let mut snapshot = false;
                let mut delete = false;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "#{constraint_id} rotation limit ? bone {node_id}"
                            ))
                            .strong(),
                        );
                        delete = ui
                            .small_button("?")
                            .on_hover_text("Delete constraint")
                            .clicked();
                    });
                    egui::Grid::new(format!("rig_rotation_limit_{constraint_id}"))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Min");
                            let response = ui.add(
                                egui::DragValue::new(&mut min_degrees)
                                    .speed(1.0)
                                    .suffix(" deg"),
                            );
                            snapshot |= response.drag_started() || response.gained_focus();
                            changed |= response.changed();
                            ui.end_row();

                            ui.label("Max");
                            let response = ui.add(
                                egui::DragValue::new(&mut max_degrees)
                                    .speed(1.0)
                                    .suffix(" deg"),
                            );
                            snapshot |= response.drag_started() || response.gained_focus();
                            changed |= response.changed();
                            ui.end_row();
                        });
                });
                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                } else if changed {
                    if snapshot {
                        app.history.snapshot(&before);
                    }
                    if set_rotation_limit(
                        &mut app.state.project,
                        q0rg_id,
                        constraint_id,
                        min_degrees.to_radians(),
                        max_degrees.to_radians(),
                    ) {
                        app.state.mark_dirty();
                    }
                }
            }
            RigConstraint::PositionLimit {
                constraint_id,
                node_id,
                min_x,
                max_x,
                min_y,
                max_y,
            } => {
                let before = app.state.project.clone();
                let mut edited_node = node_id;
                let mut edited_min_x = min_x;
                let mut edited_max_x = max_x;
                let mut edited_min_y = min_y;
                let mut edited_max_y = max_y;
                let mut changed = false;
                let mut delete = false;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("#{constraint_id} position limit"))
                                .strong(),
                        );
                        delete = ui
                            .small_button("x")
                            .on_hover_text("Delete constraint")
                            .clicked();
                    });
                    egui::Grid::new(format!("rig_position_limit_{constraint_id}"))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Bone");
                            let selected = node_choices
                                .iter()
                                .find(|(id, _)| *id == edited_node)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source((
                                "rig_position_limit_node",
                                constraint_id,
                            ))
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for (id, name) in &node_choices {
                                    changed |=
                                        ui.selectable_value(&mut edited_node, *id, name).changed();
                                }
                            });
                            ui.end_row();
                            for (label, value) in [
                                ("Min X", &mut edited_min_x),
                                ("Max X", &mut edited_max_x),
                                ("Min Y", &mut edited_min_y),
                                ("Max Y", &mut edited_max_y),
                            ] {
                                ui.label(label);
                                changed |=
                                    ui.add(egui::DragValue::new(value).speed(0.25)).changed();
                                ui.end_row();
                            }
                        });
                });
                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                } else if changed {
                    let (min_x, max_x) = if edited_min_x <= edited_max_x {
                        (edited_min_x, edited_max_x)
                    } else {
                        (edited_max_x, edited_min_x)
                    };
                    let (min_y, max_y) = if edited_min_y <= edited_max_y {
                        (edited_min_y, edited_max_y)
                    } else {
                        (edited_max_y, edited_min_y)
                    };
                    if replace_constraint(
                        &mut app.state.project,
                        q0rg_id,
                        RigConstraint::PositionLimit {
                            constraint_id,
                            node_id: edited_node,
                            min_x,
                            max_x,
                            min_y,
                            max_y,
                        },
                    ) {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                    }
                }
            }
            RigConstraint::Aim {
                constraint_id,
                node_id,
                target_control,
                angle_offset,
                weight,
            } => {
                let before = app.state.project.clone();
                let mut edited_node = node_id;
                let mut edited_target = target_control;
                let mut edited_offset = angle_offset.to_degrees();
                let mut edited_weight = pose
                    .as_ref()
                    .and_then(|pose| pose.constraint_weights.get(&constraint_id))
                    .copied()
                    .unwrap_or(weight);
                let mut structure_changed = false;
                let mut weight_changed = false;
                let mut delete = false;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("#{constraint_id} aim / look-at")).strong(),
                        );
                        delete = ui
                            .small_button("x")
                            .on_hover_text("Delete constraint")
                            .clicked();
                    });
                    egui::Grid::new(format!("rig_aim_{constraint_id}"))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Bone");
                            let selected = node_choices
                                .iter()
                                .find(|(id, _)| *id == edited_node)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_aim_node", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &node_choices {
                                        structure_changed |= ui
                                            .selectable_value(&mut edited_node, *id, name)
                                            .changed();
                                    }
                                });
                            ui.end_row();
                            ui.label("Target");
                            let selected = target_controls
                                .iter()
                                .find(|(id, _)| *id == edited_target)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_aim_target", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &target_controls {
                                        structure_changed |= ui
                                            .selectable_value(&mut edited_target, *id, name)
                                            .changed();
                                    }
                                });
                            ui.end_row();
                            ui.label("Angle offset");
                            structure_changed |= ui
                                .add(
                                    egui::DragValue::new(&mut edited_offset)
                                        .speed(1.0)
                                        .suffix(" deg"),
                                )
                                .changed();
                            ui.end_row();
                            ui.label("Weight");
                            weight_changed |= ui
                                .add(egui::Slider::new(&mut edited_weight, 0.0..=1.0))
                                .changed();
                            ui.end_row();
                        });
                });
                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                } else {
                    let mut did_change = false;
                    if structure_changed {
                        did_change |= replace_constraint(
                            &mut app.state.project,
                            q0rg_id,
                            RigConstraint::Aim {
                                constraint_id,
                                node_id: edited_node,
                                target_control: edited_target,
                                angle_offset: edited_offset.to_radians(),
                                weight: edited_weight.clamp(0.0, 1.0),
                            },
                        );
                    }
                    if weight_changed && !structure_changed {
                        did_change |= set_constraint_weight(
                            &mut app.state.project,
                            q0rg_id,
                            constraint_id,
                            app.session.current_frame,
                            edited_weight,
                            app.session.rig_auto_key,
                        );
                    }
                    if did_change {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                    }
                }
            }
            RigConstraint::Transform {
                constraint_id,
                node_id,
                target_node,
                position_weight,
                rotation_weight,
            } => {
                let before = app.state.project.clone();
                let mut edited_node = node_id;
                let mut edited_target = target_node;
                let mut edited_position_weight = position_weight;
                let mut edited_rotation_weight = rotation_weight;
                let mut changed = false;
                let mut delete = false;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("#{constraint_id} transform / parent"))
                                .strong(),
                        );
                        delete = ui
                            .small_button("x")
                            .on_hover_text("Delete constraint")
                            .clicked();
                    });
                    egui::Grid::new(format!("rig_transform_constraint_{constraint_id}"))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Bone");
                            let selected = node_choices
                                .iter()
                                .find(|(id, _)| *id == edited_node)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_transform_node", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &node_choices {
                                        changed |= ui
                                            .selectable_value(&mut edited_node, *id, name)
                                            .changed();
                                    }
                                });
                            ui.end_row();
                            ui.label("Target");
                            let selected = node_choices
                                .iter()
                                .find(|(id, _)| *id == edited_target)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_transform_target", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    if let Some(rig) = rig_for_q0rg(&app.state.project, q0rg_id) {
                                        for (id, name) in &node_choices {
                                            if !transform_target_would_cycle(
                                                rig,
                                                edited_node,
                                                *id,
                                                Some(constraint_id),
                                            ) {
                                                changed |= ui
                                                    .selectable_value(&mut edited_target, *id, name)
                                                    .changed();
                                            }
                                        }
                                    }
                                });
                            ui.end_row();
                            ui.label("Position weight");
                            changed |= ui
                                .add(egui::Slider::new(&mut edited_position_weight, 0.0..=1.0))
                                .changed();
                            ui.end_row();
                            ui.label("Rotation weight");
                            changed |= ui
                                .add(egui::Slider::new(&mut edited_rotation_weight, 0.0..=1.0))
                                .changed();
                            ui.end_row();
                        });
                });
                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                } else if changed
                    && edited_node != edited_target
                    && replace_constraint(
                        &mut app.state.project,
                        q0rg_id,
                        RigConstraint::Transform {
                            constraint_id,
                            node_id: edited_node,
                            target_node: edited_target,
                            position_weight: edited_position_weight.clamp(0.0, 1.0),
                            rotation_weight: edited_rotation_weight.clamp(0.0, 1.0),
                        },
                    )
                {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
            RigConstraint::Distance {
                constraint_id,
                node_id,
                target_control,
                min_distance,
                max_distance,
                weight,
            } => {
                let before = app.state.project.clone();
                let mut edited_node = node_id;
                let mut edited_target = target_control;
                let mut edited_min = min_distance;
                let mut edited_max = max_distance;
                let mut edited_weight = pose
                    .as_ref()
                    .and_then(|pose| pose.constraint_weights.get(&constraint_id))
                    .copied()
                    .unwrap_or(weight);
                let mut structure_changed = false;
                let mut weight_changed = false;
                let mut delete = false;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("#{constraint_id} distance")).strong(),
                        );
                        delete = ui
                            .small_button("x")
                            .on_hover_text("Delete constraint")
                            .clicked();
                    });
                    egui::Grid::new(format!("rig_distance_{constraint_id}"))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Bone");
                            let selected = node_choices
                                .iter()
                                .find(|(id, _)| *id == edited_node)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_distance_node", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &node_choices {
                                        structure_changed |= ui
                                            .selectable_value(&mut edited_node, *id, name)
                                            .changed();
                                    }
                                });
                            ui.end_row();
                            ui.label("Target");
                            let selected = target_controls
                                .iter()
                                .find(|(id, _)| *id == edited_target)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("<missing>");
                            egui::ComboBox::from_id_source(("rig_distance_target", constraint_id))
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &target_controls {
                                        structure_changed |= ui
                                            .selectable_value(&mut edited_target, *id, name)
                                            .changed();
                                    }
                                });
                            ui.end_row();
                            ui.label("Min distance");
                            structure_changed |= ui
                                .add(
                                    egui::DragValue::new(&mut edited_min)
                                        .speed(0.25)
                                        .clamp_range(0.0..=f32::MAX),
                                )
                                .changed();
                            ui.end_row();
                            ui.label("Max distance");
                            structure_changed |= ui
                                .add(
                                    egui::DragValue::new(&mut edited_max)
                                        .speed(0.25)
                                        .clamp_range(0.0..=f32::MAX),
                                )
                                .changed();
                            ui.end_row();
                            ui.label("Weight");
                            weight_changed |= ui
                                .add(egui::Slider::new(&mut edited_weight, 0.0..=1.0))
                                .changed();
                            ui.end_row();
                        });
                });
                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                } else {
                    let (min_distance, max_distance) = if edited_min <= edited_max {
                        (edited_min.max(0.0), edited_max.max(0.0))
                    } else {
                        (edited_max.max(0.0), edited_min.max(0.0))
                    };
                    let mut did_change = false;
                    if structure_changed {
                        did_change |= replace_constraint(
                            &mut app.state.project,
                            q0rg_id,
                            RigConstraint::Distance {
                                constraint_id,
                                node_id: edited_node,
                                target_control: edited_target,
                                min_distance,
                                max_distance,
                                weight: edited_weight.clamp(0.0, 1.0),
                            },
                        );
                    }
                    if weight_changed && !structure_changed {
                        did_change |= set_constraint_weight(
                            &mut app.state.project,
                            q0rg_id,
                            constraint_id,
                            app.session.current_frame,
                            edited_weight,
                            app.session.rig_auto_key,
                        );
                    }
                    if did_change {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                    }
                }
            }
            RigConstraint::TwoBoneIk {
                constraint_id,
                root_node,
                mid_node,
                tip_node,
                weight,
                allow_stretch,
                max_stretch,
                ..
            } => {
                let before = app.state.project.clone();
                let mut current_weight = pose
                    .as_ref()
                    .and_then(|pose| pose.constraint_weights.get(&constraint_id))
                    .copied()
                    .unwrap_or(weight);
                let mut stretch = allow_stretch;
                let mut stretch_max = max_stretch;
                let mut weight_changed = false;
                let mut settings_changed = false;
                let mut snapshot = false;
                let mut match_target = false;
                let mut bake_fk = false;
                let mut delete = false;

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "#{constraint_id} 2-bone IK ? {root_node} > {mid_node} > {tip_node}"
                            ))
                            .strong(),
                        );
                        delete = ui.small_button("?").on_hover_text("Delete constraint").clicked();
                    });
                    ui.horizontal(|ui| {
                        ui.label("FK");
                        let response = ui.add(
                            egui::Slider::new(&mut current_weight, 0.0..=1.0)
                                .show_value(false)
                                .clamp_to_range(true),
                        );
                        snapshot |= response.drag_started() || response.gained_focus();
                        weight_changed |= response.changed();
                        ui.label("IK");
                        ui.label(format!("{:.0}%", current_weight * 100.0));
                    });
                    let response = ui.checkbox(&mut stretch, "Allow stretch");
                    settings_changed |= response.changed();
                    if stretch {
                        ui.horizontal(|ui| {
                            ui.label("Max stretch");
                            let response = ui.add(
                                egui::DragValue::new(&mut stretch_max)
                                    .speed(0.01)
                                    .clamp_range(1.0..=8.0)
                                    .suffix("?"),
                            );
                            snapshot |= response.drag_started() || response.gained_focus();
                            settings_changed |= response.changed();
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        match_target = ui
                            .button("Match IK target to FK")
                            .on_hover_text("Move the IK target/pole onto the current FK pose without changing the blend")
                            .clicked();
                        bake_fk = ui
                            .button("Bake IK pose to FK")
                            .on_hover_text("Write the current full IK rotations into the FK bones so weight can return to 0 without a pop")
                            .clicked();
                    });
                });

                if delete {
                    app.history.snapshot(&before);
                    if remove_constraint(&mut app.state.project, q0rg_id, constraint_id) {
                        app.state.mark_dirty();
                    }
                    continue;
                }
                if weight_changed {
                    if snapshot {
                        app.history.snapshot(&before);
                    }
                    if set_constraint_weight(
                        &mut app.state.project,
                        q0rg_id,
                        constraint_id,
                        app.session.current_frame,
                        current_weight,
                        app.session.rig_auto_key,
                    ) {
                        app.state.mark_dirty();
                    }
                }
                if settings_changed {
                    if !weight_changed || !snapshot {
                        app.history.snapshot(&before);
                    }
                    if set_ik_settings(
                        &mut app.state.project,
                        q0rg_id,
                        constraint_id,
                        stretch,
                        stretch_max,
                    ) {
                        app.state.mark_dirty();
                    }
                }
                if match_target {
                    app.history.snapshot(&before);
                    match match_ik_target_to_fk(
                        &mut app.state.project,
                        q0rg_id,
                        constraint_id,
                        app.session.current_frame,
                        app.session.rig_auto_key,
                    ) {
                        Ok(()) => {
                            app.state.mark_dirty();
                            app.session.status = "IK target matched to FK pose".into();
                        }
                        Err(message) => app.session.status = message.into(),
                    }
                }
                if bake_fk {
                    app.history.snapshot(&before);
                    match bake_ik_pose_to_fk(
                        &mut app.state.project,
                        q0rg_id,
                        constraint_id,
                        app.session.current_frame,
                        app.session.rig_auto_key,
                    ) {
                        Ok(()) => {
                            app.state.mark_dirty();
                            app.session.status = "IK pose baked into FK rotations".into();
                        }
                        Err(message) => app.session.status = message.into(),
                    }
                }
            }
        }
        ui.add_space(4.0);
    }

    let Some(node_id) = app.session.rig_selected_node else {
        return;
    };
    let already_limited = rig_for_q0rg(&app.state.project, q0rg_id).is_some_and(|rig| {
        rig.constraints.iter().any(|constraint| {
            matches!(constraint, RigConstraint::RotationLimit { node_id: id, .. } if *id == node_id)
        })
    });
    if ui
        .add_enabled(
            !already_limited,
            egui::Button::new("+ rotation limit on selected bone"),
        )
        .clicked()
    {
        let before = app.state.project.clone();
        if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
            if let Some(id) = next_constraint_id(rig) {
                rig.constraints.push(RigConstraint::RotationLimit {
                    constraint_id: id,
                    node_id,
                    min_radians: -std::f32::consts::PI,
                    max_radians: std::f32::consts::PI,
                });
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
    let rig_snapshot = rig_for_q0rg(&app.state.project, q0rg_id).cloned();
    let has_position_limit = rig_snapshot.as_ref().is_some_and(|rig| {
        rig.constraints.iter().any(|constraint| {
            matches!(constraint, RigConstraint::PositionLimit { node_id: id, .. } if *id == node_id)
        })
    });
    if ui
        .add_enabled(
            !has_position_limit,
            egui::Button::new("+ position limit on selected bone"),
        )
        .clicked()
    {
        let before = app.state.project.clone();
        if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
            if let (Some(id), Some(node)) = (
                next_constraint_id(rig),
                rig.nodes
                    .iter()
                    .find(|node| node.node_id == node_id)
                    .cloned(),
            ) {
                rig.constraints.push(RigConstraint::PositionLimit {
                    constraint_id: id,
                    node_id,
                    min_x: node.rest.tx - 100.0,
                    max_x: node.rest.tx + 100.0,
                    min_y: node.rest.ty - 100.0,
                    max_y: node.rest.ty + 100.0,
                });
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
    let selected_target_control = rig_snapshot.as_ref().and_then(|rig| {
        app.session
            .rig_selected_control
            .and_then(|id| {
                rig.controls.iter().find(|control| {
                    control.control_id == id
                        && control.kind == RigControlKind::Position2D
                        && control.target_node.is_none()
                })
            })
            .or_else(|| {
                rig.controls.iter().find(|control| {
                    control.kind == RigControlKind::Position2D && control.target_node.is_none()
                })
            })
            .map(|control| control.control_id)
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                selected_target_control.is_some(),
                egui::Button::new("+ aim/look-at"),
            )
            .clicked()
        {
            let before = app.state.project.clone();
            if let (Some(target_control), Some(rig)) = (
                selected_target_control,
                rig_for_q0rg_mut(&mut app.state.project, q0rg_id),
            ) {
                if let Some(id) = next_constraint_id(rig) {
                    rig.constraints.push(RigConstraint::Aim {
                        constraint_id: id,
                        node_id,
                        target_control,
                        angle_offset: 0.0,
                        weight: 1.0,
                    });
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
        if ui
            .add_enabled(
                selected_target_control.is_some(),
                egui::Button::new("+ distance"),
            )
            .clicked()
        {
            let before = app.state.project.clone();
            if let (Some(target_control), Some(rig)) = (
                selected_target_control,
                rig_for_q0rg_mut(&mut app.state.project, q0rg_id),
            ) {
                if let Some(id) = next_constraint_id(rig) {
                    rig.constraints.push(RigConstraint::Distance {
                        constraint_id: id,
                        node_id,
                        target_control,
                        min_distance: 0.0,
                        max_distance: 250.0,
                        weight: 1.0,
                    });
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
    });
    let transform_target = rig_snapshot.as_ref().and_then(|rig| {
        rig.nodes
            .iter()
            .find(|candidate| {
                candidate.node_id != node_id
                    && !transform_target_would_cycle(rig, node_id, candidate.node_id, None)
            })
            .map(|node| node.node_id)
    });
    if ui
        .add_enabled(
            transform_target.is_some(),
            egui::Button::new("+ transform constraint"),
        )
        .clicked()
    {
        let before = app.state.project.clone();
        if let (Some(target_node), Some(rig)) = (
            transform_target,
            rig_for_q0rg_mut(&mut app.state.project, q0rg_id),
        ) {
            if let Some(id) = next_constraint_id(rig) {
                rig.constraints.push(RigConstraint::Transform {
                    constraint_id: id,
                    node_id,
                    target_node,
                    position_weight: 1.0,
                    rotation_weight: 1.0,
                });
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
}

fn render_pro_pose_metadata(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    ui.label(egui::RichText::new("Pose drivers + mirror metadata").strong());
    let snapshot = rig_for_q0rg(&app.state.project, q0rg_id).cloned();
    let Some(snapshot) = snapshot else {
        return;
    };

    if let Some(left) = app.session.rig_selected_node {
        let candidates = snapshot
            .nodes
            .iter()
            .filter(|node| node.node_id != left)
            .map(|node| (node.node_id, node.name.clone()))
            .collect::<Vec<_>>();
        if app.session.rig_mirror_partner_node.is_none() {
            app.session.rig_mirror_partner_node = candidates.first().map(|(id, _)| *id);
        }
        ui.horizontal(|ui| {
            ui.label(format!("mirror bone #{}", left));
            let selected = app
                .session
                .rig_mirror_partner_node
                .and_then(|id| candidates.iter().find(|(candidate, _)| *candidate == id))
                .map(|(_, name)| name.as_str())
                .unwrap_or("<none>");
            egui::ComboBox::from_id_source("rig_mirror_partner_node")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (id, name) in &candidates {
                        ui.selectable_value(
                            &mut app.session.rig_mirror_partner_node,
                            Some(*id),
                            name,
                        );
                    }
                });
            if ui.button("Pair bones").clicked() {
                let before = app.state.project.clone();
                match app.session.rig_mirror_partner_node {
                    Some(right) => {
                        match add_node_mirror_pairs(&mut app.state.project, q0rg_id, left, right) {
                            Ok(()) => {
                                app.history.snapshot(&before);
                                app.state.mark_dirty();
                            }
                            Err(message) => app.session.status = message.into(),
                        }
                    }
                    None => app.session.status = "choose a mirror partner bone".into(),
                }
            }
        });
    }
    if let Some(left) = app.session.rig_selected_control {
        let left_kind = snapshot
            .controls
            .iter()
            .find(|control| control.control_id == left)
            .map(|control| control.kind);
        let candidates = snapshot
            .controls
            .iter()
            .filter(|control| control.control_id != left && Some(control.kind) == left_kind)
            .map(|control| (control.control_id, control.name.clone()))
            .collect::<Vec<_>>();
        if app.session.rig_mirror_partner_control.is_none() {
            app.session.rig_mirror_partner_control = candidates.first().map(|(id, _)| *id);
        }
        ui.horizontal(|ui| {
            ui.label(format!("mirror control #{}", left));
            let selected = app
                .session
                .rig_mirror_partner_control
                .and_then(|id| candidates.iter().find(|(candidate, _)| *candidate == id))
                .map(|(_, name)| name.as_str())
                .unwrap_or("<none>");
            egui::ComboBox::from_id_source("rig_mirror_partner_control")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (id, name) in &candidates {
                        ui.selectable_value(
                            &mut app.session.rig_mirror_partner_control,
                            Some(*id),
                            name,
                        );
                    }
                });
            if ui.button("Pair controls").clicked() {
                let before = app.state.project.clone();
                match app.session.rig_mirror_partner_control {
                    Some(right) => {
                        match add_control_mirror_pairs(&mut app.state.project, q0rg_id, left, right)
                        {
                            Ok(()) => {
                                app.history.snapshot(&before);
                                app.state.mark_dirty();
                            }
                            Err(message) => app.session.status = message.into(),
                        }
                    }
                    None => app.session.status = "choose a mirror partner control".into(),
                }
            }
        });
    }

    let pairs = snapshot.mirror_pairs.clone();
    if !pairs.is_empty() {
        egui::CollapsingHeader::new(format!("Mirror property pairs ({})", pairs.len()))
            .default_open(false)
            .show(ui, |ui| {
                for pair in pairs {
                    let mut delete = false;
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} <-> {}",
                            rig_property_label(&snapshot, pair.left),
                            rig_property_label(&snapshot, pair.right)
                        ));
                        ui.label(format!("x{:.2} {:+.2}", pair.multiplier, pair.offset));
                        delete = ui.small_button("x").clicked();
                    });
                    if delete {
                        let before = app.state.project.clone();
                        if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                            rig.mirror_pairs.retain(|candidate| *candidate != pair);
                            app.history.snapshot(&before);
                            app.state.mark_dirty();
                        }
                    }
                }
            });
    }

    let drivers = snapshot.pose_drivers.clone();
    if drivers.is_empty() {
        ui.label(
            egui::RichText::new(
                "pose drivers: select a scalar control, then use '+ driver' on a pose",
            )
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
    }
    for driver in drivers {
        let pose_name = snapshot
            .poses
            .iter()
            .find(|pose| pose.pose_id == driver.pose_id)
            .map(|pose| pose.name.as_str())
            .unwrap_or("missing pose");
        let source_name = snapshot
            .controls
            .iter()
            .find(|control| control.control_id == driver.source_control)
            .map(|control| control.name.as_str())
            .unwrap_or("missing control");
        let mut source_min = driver.source_min;
        let mut source_max = driver.source_max;
        let mut weight_min = driver.weight_min;
        let mut weight_max = driver.weight_max;
        let mut mode = driver.mode;
        let mut changed = false;
        let mut delete = false;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "#{} {} -> {}",
                        driver.driver_id, source_name, pose_name
                    ))
                    .strong(),
                );
                delete = ui.small_button("x").clicked();
            });
            egui::Grid::new(("pose_driver", driver.driver_id))
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Source min");
                    changed |= ui
                        .add(egui::DragValue::new(&mut source_min).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Source max");
                    changed |= ui
                        .add(egui::DragValue::new(&mut source_max).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Weight min");
                    changed |= ui
                        .add(egui::Slider::new(&mut weight_min, 0.0..=1.0))
                        .changed();
                    ui.end_row();
                    ui.label("Weight max");
                    changed |= ui
                        .add(egui::Slider::new(&mut weight_max, 0.0..=1.0))
                        .changed();
                    ui.end_row();
                    ui.label("Mode");
                    egui::ComboBox::from_id_source(("pose_driver_mode", driver.driver_id))
                        .selected_text(match mode {
                            RigPoseBlendMode::Override => "Override",
                            RigPoseBlendMode::Additive => "Additive",
                        })
                        .show_ui(ui, |ui| {
                            changed |= ui
                                .selectable_value(&mut mode, RigPoseBlendMode::Override, "Override")
                                .changed();
                            changed |= ui
                                .selectable_value(&mut mode, RigPoseBlendMode::Additive, "Additive")
                                .changed();
                        });
                    ui.end_row();
                });
        });
        if delete || changed {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                if delete {
                    rig.pose_drivers
                        .retain(|candidate| candidate.driver_id != driver.driver_id);
                } else if let Some(current) = rig
                    .pose_drivers
                    .iter_mut()
                    .find(|candidate| candidate.driver_id == driver.driver_id)
                {
                    if (source_max - source_min).abs() < 1.0e-4 {
                        source_max = source_min + 1.0e-4;
                    }
                    current.source_min = source_min;
                    current.source_max = source_max;
                    current.weight_min = weight_min;
                    current.weight_max = weight_max;
                    current.mode = mode;
                }
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
}

fn render_pro_variants(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    ui.label(egui::RichText::new("Drawing substitutions").strong());
    let selected_scalar = app.session.rig_selected_control.and_then(|id| {
        rig_for_q0rg(&app.state.project, q0rg_id).and_then(|rig| {
            rig.controls
                .iter()
                .find(|control| {
                    control.control_id == id
                        && matches!(
                            control.kind,
                            RigControlKind::Slider | RigControlKind::Toggle
                        )
                })
                .map(|_| id)
        })
    });
    if ui
        .add_enabled(
            selected_scalar.is_some(),
            egui::Button::new("Create variant set from selected object"),
        )
        .clicked()
    {
        let before = app.state.project.clone();
        match selected_scalar {
            Some(source) => match create_variant_set_from_selection(
                &mut app.state.project,
                &app.session.selection,
                app.session.current_frame,
                source,
            ) {
                Ok(id) => {
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                    app.session.status = format!("variant set #{id} created");
                }
                Err(message) => app.session.status = message.into(),
            },
            None => app.session.status = "select a slider or toggle first".into(),
        }
    }
    let variants = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| rig.variants.clone())
        .unwrap_or_default();
    if variants.is_empty() {
        ui.label(
            egui::RichText::new(
                "select a display object + scalar control to create mouth/hand/head choices",
            )
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
        return;
    }
    let target_options = app
        .state
        .project
        .assets
        .iter()
        .filter_map(|asset| (!matches!(asset, Asset::Rig(_))).then_some(Target::Asset(asset.id())))
        .chain(
            app.state
                .project
                .q0rgs
                .iter()
                .filter(|q0rg| q0rg.q0rg_id != q0rg_id)
                .map(|q0rg| Target::Q0rg(q0rg.q0rg_id)),
        )
        .collect::<Vec<_>>();
    if app.session.rig_variant_add_target.is_none() {
        app.session.rig_variant_add_target = target_options.first().copied();
    }
    for variant in variants {
        let mut name = variant.name.clone();
        let mut rename = false;
        let mut delete = false;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                rename = ui.text_edit_singleline(&mut name).changed();
                ui.label(format!("instance #{}", variant.instance_id));
                delete = ui.small_button("x").clicked();
            });
            for (index, choice) in variant.choices.iter().enumerate() {
                ui.label(format!(
                    "{}: {} -> {}",
                    index,
                    choice.name,
                    target_label(&app.state.project, choice.target)
                ));
            }
            ui.horizontal(|ui| {
                let selected = app
                    .session
                    .rig_variant_add_target
                    .map(|target| target_label(&app.state.project, target))
                    .unwrap_or_else(|| "<none>".into());
                egui::ComboBox::from_id_source(("variant_add_target", variant.variant_id))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for target in &target_options {
                            ui.selectable_value(
                                &mut app.session.rig_variant_add_target,
                                Some(*target),
                                target_label(&app.state.project, *target),
                            );
                        }
                    });
                let source_kind = rig_for_q0rg(&app.state.project, q0rg_id)
                    .and_then(|rig| {
                        rig.controls
                            .iter()
                            .find(|control| control.control_id == variant.source_control)
                    })
                    .map(|control| control.kind);
                let can_add =
                    source_kind != Some(RigControlKind::Toggle) || variant.choices.len() < 2;
                if ui
                    .add_enabled(can_add, egui::Button::new("+ choice"))
                    .clicked()
                {
                    if let Some(target) = app.session.rig_variant_add_target {
                        let before = app.state.project.clone();
                        if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                            if let Some(current) = rig
                                .variants
                                .iter_mut()
                                .find(|candidate| candidate.variant_id == variant.variant_id)
                            {
                                current.choices.push(RigVariantChoice {
                                    name: target_label(&before, target),
                                    target,
                                });
                                let max = (current.choices.len() - 1) as f32;
                                if let Some(control) = rig
                                    .controls
                                    .iter_mut()
                                    .find(|control| control.control_id == current.source_control)
                                {
                                    control.min_value = 0.0;
                                    control.max_value =
                                        max.max(if control.kind == RigControlKind::Toggle {
                                            1.0
                                        } else {
                                            0.0
                                        });
                                    control.rest_value = control
                                        .rest_value
                                        .clamp(control.min_value, control.max_value);
                                }
                                app.history.snapshot(&before);
                                app.state.mark_dirty();
                            }
                        }
                    }
                }
            });
        });
        if rename && !name.trim().is_empty() {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                if let Some(current) = rig
                    .variants
                    .iter_mut()
                    .find(|candidate| candidate.variant_id == variant.variant_id)
                {
                    current.name = name;
                    app.history.snapshot(&before);
                    app.state.mark_dirty();
                }
            }
        }
        if delete {
            let before = app.state.project.clone();
            if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                rig.variants
                    .retain(|candidate| candidate.variant_id != variant.variant_id);
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
}

fn render_pro_deformers(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    ui.label(egui::RichText::new("Vector deformers").strong());
    ui.horizontal_wrapped(|ui| {
        for (label, kind) in [("Auto skin", 0_u8), ("Bend", 1_u8), ("Cage", 2_u8)] {
            if ui.button(label).clicked() {
                let before = app.state.project.clone();
                let result = match kind {
                    0 => create_skin_deformer_from_selection(
                        &mut app.state.project,
                        &app.session.selection,
                        app.session.current_frame,
                    ),
                    1 => create_bend_deformer_from_selection(
                        &mut app.state.project,
                        &app.session.selection,
                        app.session.current_frame,
                    ),
                    _ => create_cage_deformer_from_selection(
                        &mut app.state.project,
                        &app.session.selection,
                        app.session.current_frame,
                    ),
                };
                match result {
                    Ok(id) => {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                        app.session.status = format!("{label} deformer #{id} created");
                    }
                    Err(message) => app.session.status = message.into(),
                }
            }
        }
    });
    ui.label(
        egui::RichText::new("select one vector display object; skin uses bones, bend/cage create public stage controls")
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
    );

    let deformers = rig_for_q0rg(&app.state.project, q0rg_id)
        .map(|rig| rig.deformers.clone())
        .unwrap_or_default();
    if deformers.is_empty() {
        ui.label("none");
        return;
    }
    for deformer in deformers {
        let id = deformer.id();
        let kind = match deformer {
            RigDeformer::Skin { .. } => "Skin",
            RigDeformer::Bend { .. } => "Bend",
            RigDeformer::Cage { .. } => "Cage",
        };
        let mut delete = false;
        egui::CollapsingHeader::new(format!("#{id} {kind} / instance #{}", deformer.instance_id()))
            .default_open(false)
            .show(ui, |ui| {
                ui.label(format!("vector asset #{}", deformer.asset_id()));
                delete = ui.button("Delete deformer").clicked();
                if let RigDeformer::Skin { anchors, .. } = &deformer {
                    let selected_node = app.session.rig_selected_node;
                    let bone_names = rig_for_q0rg(&app.state.project, q0rg_id)
                        .map(|rig| rig.nodes.iter().map(|node| (node.node_id, node.name.clone())).collect::<Vec<_>>())
                        .unwrap_or_default();
                    if ui.button("Re-auto weight all anchors").clicked() {
                        let before = app.state.project.clone();
                        let snapshot = rig_for_q0rg(&app.state.project, q0rg_id).cloned();
                        let vector = app.state.project.assets.iter().find_map(|asset| match asset {
                            Asset::Vector(vector) if vector.asset_id == deformer.asset_id() => Some(vector.clone()),
                            _ => None,
                        });
                        if let (Some(snapshot), Some(vector), Some(rig)) = (
                            snapshot,
                            vector,
                            rig_for_q0rg_mut(&mut app.state.project, q0rg_id),
                        ) {
                            if let Some(RigDeformer::Skin { bind_transform, anchors, .. }) = rig.deformers.iter_mut().find(|candidate| candidate.id() == id) {
                                *anchors = build_skin_anchor_weights(&snapshot, &vector, *bind_transform, 4);
                                app.history.snapshot(&before);
                                app.state.mark_dirty();
                            }
                        }
                    }
                    ui.label(format!("{} weighted anchors", anchors.len()));
                    for anchor_snapshot in anchors.iter().take(128) {
                        let path_index = anchor_snapshot.path_index;
                        let anchor_index = anchor_snapshot.anchor_index;
                        egui::CollapsingHeader::new(format!("path {path_index} / anchor {anchor_index}"))
                            .default_open(false)
                            .show(ui, |ui| {
                                let before = app.state.project.clone();
                                let mut edited = anchor_snapshot.weights.clone();
                                let mut changed = false;
                                let mut remove_index = None;
                                let can_remove = edited.len() > 1;
                                for (index, weight) in edited.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        let name = bone_names.iter().find(|(node_id, _)| *node_id == weight.node_id)
                                            .map(|(_, name)| name.as_str()).unwrap_or("missing bone");
                                        ui.label(name);
                                        changed |= ui.add(egui::Slider::new(&mut weight.weight, 0.001..=1.0).show_value(true)).changed();
                                        if can_remove && ui.small_button("x").clicked() {
                                            remove_index = Some(index);
                                        }
                                    });
                                }
                                if let Some(index) = remove_index {
                                    edited.remove(index);
                                    changed = true;
                                }
                                if let Some(node_id) = selected_node {
                                    let has = edited.iter().any(|weight| weight.node_id == node_id);
                                    if !has && edited.len() < 4 && ui.button("+ selected bone influence").clicked() {
                                        edited.push(RigSkinWeight { node_id, weight: 0.1 });
                                        changed = true;
                                    }
                                }
                                if changed {
                                    normalize_skin_weights(&mut edited);
                                    if let Some(rig) = rig_for_q0rg_mut(&mut app.state.project, q0rg_id) {
                                        if let Some(RigDeformer::Skin { anchors, .. }) = rig.deformers.iter_mut().find(|candidate| candidate.id() == id) {
                                            if let Some(anchor) = anchors.iter_mut().find(|anchor| anchor.path_index == path_index && anchor.anchor_index == anchor_index) {
                                                anchor.weights = edited;
                                                app.history.snapshot(&before);
                                                app.state.mark_dirty();
                                            }
                                        }
                                    }
                                }
                            });
                    }
                    if anchors.len() > 128 {
                        ui.label("weight editor shows the first 128 anchors at once; re-auto still updates all");
                    }
                }
            });
        if delete {
            let before = app.state.project.clone();
            if remove_deformer(&mut app.state.project, q0rg_id, id) {
                app.history.snapshot(&before);
                app.state.mark_dirty();
            }
        }
    }
}

fn render_pro_graph(app: &mut EditorApp, ui: &mut Ui, q0rg_id: u16) {
    let Some(rig) = rig_for_q0rg(&app.state.project, q0rg_id) else {
        return;
    };
    egui::CollapsingHeader::new("Dependency graph")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "view only: edits above change this graph; it is not a second rig model",
                )
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
            );
            egui::ScrollArea::horizontal().show(ui, |ui| {
                let row_h = 34.0_f32;
                let box_w = 145.0_f32;
                let column_gap = 55.0_f32;
                let x_control = 8.0_f32;
                let x_bone = x_control + box_w + column_gap;
                let x_constraint = x_bone + box_w + column_gap;
                let x_deformer = x_constraint + box_w + column_gap;
                let x_output = x_deformer + box_w + column_gap;
                let rows = rig
                    .controls
                    .len()
                    .max(rig.nodes.len())
                    .max(rig.constraints.len())
                    .max(rig.deformers.len())
                    .max(
                        rig.nodes
                            .iter()
                            .filter(|node| node.binding.is_some())
                            .count()
                            + rig.deformers.len(),
                    )
                    .max(1);
                let size = egui::vec2(x_output + box_w + 12.0, rows as f32 * row_h + 32.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                let origin = rect.min;
                let text_color = app.settings.theme.text.to_color32();
                let dim = app.settings.theme.text_dim.to_color32();
                let accent = app.settings.theme.accent.to_color32();
                let make_box = |x: f32, row: usize, label: &str, selected: bool| {
                    let min = origin + egui::vec2(x, 24.0 + row as f32 * row_h);
                    let box_rect = egui::Rect::from_min_size(min, egui::vec2(box_w, 26.0));
                    painter.rect_filled(box_rect, 3.0, Color32::from_black_alpha(52));
                    painter.rect_stroke(
                        box_rect,
                        3.0,
                        Stroke::new(1.2_f32, if selected { accent } else { dim }),
                    );
                    painter.text(
                        box_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(11.0),
                        text_color,
                    );
                    box_rect
                };
                painter.text(
                    origin + egui::vec2(x_control, 6.0),
                    egui::Align2::LEFT_TOP,
                    "controls",
                    egui::FontId::proportional(11.0),
                    dim,
                );
                painter.text(
                    origin + egui::vec2(x_bone, 6.0),
                    egui::Align2::LEFT_TOP,
                    "bones",
                    egui::FontId::proportional(11.0),
                    dim,
                );
                painter.text(
                    origin + egui::vec2(x_constraint, 6.0),
                    egui::Align2::LEFT_TOP,
                    "constraints",
                    egui::FontId::proportional(11.0),
                    dim,
                );
                painter.text(
                    origin + egui::vec2(x_deformer, 6.0),
                    egui::Align2::LEFT_TOP,
                    "deformers",
                    egui::FontId::proportional(11.0),
                    dim,
                );
                painter.text(
                    origin + egui::vec2(x_output, 6.0),
                    egui::Align2::LEFT_TOP,
                    "outputs",
                    egui::FontId::proportional(11.0),
                    dim,
                );

                let control_rects = rig
                    .controls
                    .iter()
                    .enumerate()
                    .map(|(row, control)| {
                        (
                            control.control_id,
                            make_box(
                                x_control,
                                row,
                                &control.name,
                                app.session.rig_selected_control == Some(control.control_id),
                            ),
                        )
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let bone_rects = rig
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(row, node)| {
                        (
                            node.node_id,
                            make_box(
                                x_bone,
                                row,
                                &node.name,
                                app.session.rig_selected_node == Some(node.node_id),
                            ),
                        )
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let constraint_rects = rig
                    .constraints
                    .iter()
                    .enumerate()
                    .map(|(row, constraint)| {
                        let label = match constraint {
                            RigConstraint::RotationLimit { .. } => "rotation limit",
                            RigConstraint::PositionLimit { .. } => "position limit",
                            RigConstraint::Aim { .. } => "aim",
                            RigConstraint::Transform { .. } => "transform",
                            RigConstraint::Distance { .. } => "distance",
                            RigConstraint::TwoBoneIk { .. } => "2-bone IK",
                        };
                        (
                            constraint.id(),
                            make_box(
                                x_constraint,
                                row,
                                &format!("#{0} {label}", constraint.id()),
                                false,
                            ),
                        )
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let deformer_rects = rig
                    .deformers
                    .iter()
                    .enumerate()
                    .map(|(row, deformer)| {
                        let kind = match deformer {
                            RigDeformer::Skin { .. } => "skin",
                            RigDeformer::Bend { .. } => "bend",
                            RigDeformer::Cage { .. } => "cage",
                        };
                        (
                            deformer.id(),
                            make_box(
                                x_deformer,
                                row,
                                &format!("#{} {kind}", deformer.id()),
                                false,
                            ),
                        )
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let mut output_items = rig
                    .nodes
                    .iter()
                    .filter_map(|node| {
                        node.binding.map(|binding| {
                            (
                                binding.instance_id,
                                format!("instance #{}", binding.instance_id),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                for deformer in &rig.deformers {
                    if output_items
                        .iter()
                        .all(|(id, _)| *id != deformer.instance_id())
                    {
                        output_items.push((
                            deformer.instance_id(),
                            format!("instance #{}", deformer.instance_id()),
                        ));
                    }
                }
                let output_rects = output_items
                    .into_iter()
                    .enumerate()
                    .map(|(row, (instance_id, label))| {
                        (instance_id, make_box(x_output, row, &label, false))
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let edge = |from: egui::Rect, to: egui::Rect, color: Color32| {
                    painter.line_segment(
                        [
                            egui::pos2(from.right(), from.center().y),
                            egui::pos2(to.left(), to.center().y),
                        ],
                        Stroke::new(1.0_f32, color),
                    );
                };
                for node in &rig.nodes {
                    if let (Some(parent), Some(to)) = (
                        node.parent.and_then(|id| bone_rects.get(&id)),
                        bone_rects.get(&node.node_id),
                    ) {
                        edge(*parent, *to, dim.gamma_multiply(0.7));
                    }
                    if let Some(binding) = node.binding {
                        if let (Some(from), Some(to)) = (
                            bone_rects.get(&node.node_id),
                            output_rects.get(&binding.instance_id),
                        ) {
                            edge(*from, *to, accent.gamma_multiply(0.8));
                        }
                    }
                }
                for control in &rig.controls {
                    if let (Some(from), Some(to)) = (
                        control_rects.get(&control.control_id),
                        control.target_node.and_then(|id| bone_rects.get(&id)),
                    ) {
                        edge(*from, *to, accent.gamma_multiply(0.75));
                    }
                }
                for constraint in &rig.constraints {
                    let Some(to) = constraint_rects.get(&constraint.id()) else {
                        continue;
                    };
                    let mut controls = Vec::new();
                    let mut bones = Vec::new();
                    match *constraint {
                        RigConstraint::RotationLimit { node_id, .. }
                        | RigConstraint::PositionLimit { node_id, .. } => bones.push(node_id),
                        RigConstraint::Aim {
                            node_id,
                            target_control,
                            ..
                        }
                        | RigConstraint::Distance {
                            node_id,
                            target_control,
                            ..
                        } => {
                            bones.push(node_id);
                            controls.push(target_control);
                        }
                        RigConstraint::Transform {
                            node_id,
                            target_node,
                            ..
                        } => {
                            bones.push(node_id);
                            bones.push(target_node);
                        }
                        RigConstraint::TwoBoneIk {
                            root_node,
                            mid_node,
                            tip_node,
                            target_control,
                            pole_control,
                            ..
                        } => {
                            bones.extend([root_node, mid_node, tip_node]);
                            controls.push(target_control);
                            if let Some(pole) = pole_control {
                                controls.push(pole);
                            }
                        }
                    }
                    for id in controls {
                        if let Some(from) = control_rects.get(&id) {
                            edge(*from, *to, dim);
                        }
                    }
                    for id in bones {
                        if let Some(from) = bone_rects.get(&id) {
                            edge(*from, *to, dim);
                        }
                    }
                }
                for deformer in &rig.deformers {
                    let Some(to) = deformer_rects.get(&deformer.id()) else {
                        continue;
                    };
                    match deformer {
                        RigDeformer::Skin { bones, .. } => {
                            for bone in bones {
                                if let Some(from) = bone_rects.get(&bone.node_id) {
                                    edge(*from, *to, dim);
                                }
                            }
                        }
                        RigDeformer::Bend {
                            start_control,
                            middle_control,
                            end_control,
                            ..
                        } => {
                            for id in [*start_control, *middle_control, *end_control] {
                                if let Some(from) = control_rects.get(&id) {
                                    edge(*from, *to, dim);
                                }
                            }
                        }
                        RigDeformer::Cage { controls, .. } => {
                            for id in controls {
                                if let Some(from) = control_rects.get(id) {
                                    edge(*from, *to, dim);
                                }
                            }
                        }
                    }
                    if let Some(output) = output_rects.get(&deformer.instance_id()) {
                        edge(*to, *output, accent.gamma_multiply(0.8));
                    }
                }
                for driver in &rig.drivers {
                    let Some(from) = control_rects.get(&driver.source_control) else {
                        continue;
                    };
                    match driver.target {
                        RigPropertyRef::NodeTx(id)
                        | RigPropertyRef::NodeTy(id)
                        | RigPropertyRef::NodeRotation(id)
                        | RigPropertyRef::NodeScaleX(id)
                        | RigPropertyRef::NodeScaleY(id) => {
                            if let Some(to) = bone_rects.get(&id) {
                                edge(*from, *to, accent);
                            }
                        }
                        RigPropertyRef::ConstraintWeight(id) => {
                            if let Some(to) = constraint_rects.get(&id) {
                                edge(*from, *to, accent);
                            }
                        }
                        RigPropertyRef::ControlX(_)
                        | RigPropertyRef::ControlY(_)
                        | RigPropertyRef::ControlValue(_) => {}
                    }
                }
            });
        });
}

pub fn handle_stage(
    app: &mut EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
    ctx: &Context,
) {
    let q0rg_id = app.session.current_q0rg_id;
    if rig_for_q0rg(&app.state.project, q0rg_id).is_none() {
        if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Default);
        }
        return;
    }
    let pointer_stage = response.hover_pos().map(|pos| screen_to_stage(pos, view));

    match app.session.rig_edit_mode {
        RigEditMode::AddBone => handle_add_bone(app, response, pointer_stage, view),
        RigEditMode::Pose => handle_pose(app, response, pointer_stage, view),
    }
    paint_overlay(app, painter, view);
    if response.hovered() {
        ctx.set_cursor_icon(match app.session.rig_edit_mode {
            RigEditMode::AddBone => egui::CursorIcon::Crosshair,
            RigEditMode::Pose => egui::CursorIcon::PointingHand,
        });
    }
}

fn handle_add_bone(
    app: &mut EditorApp,
    response: &Response,
    pointer_stage: Option<Vec2>,
    view: &StageView,
) {
    if response.drag_started_by(PointerButton::Primary) {
        if let Some(point) = pointer_stage {
            app.session.rig_pending_bone_start = Some(point);
        }
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        let Some(start) = app.session.rig_pending_bone_start.take() else {
            return;
        };
        let Some(end) = pointer_stage else {
            return;
        };
        if distance_screen(start, end, view) < 6.0 {
            return;
        }
        let before = app.state.project.clone();
        if let Some(node_id) = add_bone(
            &mut app.state.project,
            app.session.current_q0rg_id,
            app.session.rig_selected_node,
            start,
            end,
            app.session.current_frame,
        ) {
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.rig_selected_node = Some(node_id);
            app.session.rig_selected_control = None;
            app.session.status = format!("bone {node_id} created");
        }
    }
}

fn handle_pose(
    app: &mut EditorApp,
    response: &Response,
    pointer_stage: Option<Vec2>,
    view: &StageView,
) {
    let q0rg_id = app.session.current_q0rg_id;
    let Some(rig) = rig_for_q0rg(&app.state.project, q0rg_id) else {
        return;
    };
    let pose = evaluate_rig(rig, f32::from(app.session.current_frame), &[]);

    if response.drag_started_by(PointerButton::Primary) {
        let Some(pointer) = pointer_stage else {
            return;
        };
        if let Some(control_id) = nearest_control(app, rig, &pose, pointer, view) {
            if let Some(control) = rig
                .controls
                .iter()
                .find(|control| control.control_id == control_id)
            {
                let value = pose.controls.get(&control_id).copied().unwrap_or(
                    q0s_format::rig::RigControlValue {
                        x: control.rest_x,
                        y: control.rest_y,
                        value: control.rest_value,
                    },
                );
                if control.kind == RigControlKind::Toggle {
                    let before = app.state.project.clone();
                    if set_control_value(
                        &mut app.state.project,
                        q0rg_id,
                        control_id,
                        app.session.current_frame,
                        if value.value >= 0.5 { 0.0 } else { 1.0 },
                        app.session.rig_auto_key,
                    ) {
                        app.history.snapshot(&before);
                        app.state.mark_dirty();
                        app.session.rig_selected_control = Some(control_id);
                        app.session.rig_selected_node = None;
                    }
                    return;
                }
                app.history.snapshot(&app.state.project);
                app.session.rig_control_drag = Some(RigControlDrag {
                    control_id,
                    start_cursor: pointer,
                    start_x: value.x,
                    start_y: value.y,
                    start_value: value.value,
                });
                app.session.rig_selected_control = Some(control_id);
                app.session.rig_selected_node = None;
                return;
            }
        }
        if let Some(node_id) = nearest_bone(rig, &pose, pointer, view) {
            let Some(center_world) = pose
                .node_world
                .get(&node_id)
                .map(|world| world.apply(Vec2::new(0.0, 0.0)))
            else {
                return;
            };
            let parent_world_angle = rig
                .nodes
                .iter()
                .find(|node| node.node_id == node_id)
                .and_then(|node| node.parent)
                .and_then(|parent| pose.node_world.get(&parent))
                .map(|world| world.a21.atan2(world.a11))
                .unwrap_or(0.0);
            app.history.snapshot(&app.state.project);
            app.session.rig_bone_drag = Some(RigBoneDrag {
                node_id,
                center_world,
                parent_world_angle,
            });
            app.session.rig_selected_node = Some(node_id);
            app.session.rig_selected_control = None;
        }
    }

    if response.dragged_by(PointerButton::Primary) {
        let Some(pointer) = pointer_stage else {
            return;
        };
        if let Some(drag) = app.session.rig_bone_drag {
            let rotation = (pointer.y - drag.center_world.y).atan2(pointer.x - drag.center_world.x)
                - drag.parent_world_angle;
            if set_node_rotation(
                &mut app.state.project,
                q0rg_id,
                drag.node_id,
                app.session.current_frame,
                rotation,
                app.session.rig_auto_key,
            ) {
                app.state.mark_dirty();
            }
            return;
        }
        let Some(drag) = app.session.rig_control_drag else {
            return;
        };
        let Some(rig_snapshot) = rig_for_q0rg(&app.state.project, q0rg_id).cloned() else {
            return;
        };
        let Some(control) = rig_snapshot
            .controls
            .iter()
            .find(|control| control.control_id == drag.control_id)
        else {
            return;
        };
        let changed = match control.kind {
            RigControlKind::Position2D => {
                let world_delta = Vec2::new(
                    pointer.x - drag.start_cursor.x,
                    pointer.y - drag.start_cursor.y,
                );
                let pose = evaluate_rig(&rig_snapshot, f32::from(app.session.current_frame), &[]);
                let delta = control
                    .target_node
                    .map(|node_id| {
                        world_delta_to_node_parent_local(&rig_snapshot, &pose, node_id, world_delta)
                    })
                    .unwrap_or(world_delta);
                set_control_position(
                    &mut app.state.project,
                    q0rg_id,
                    drag.control_id,
                    app.session.current_frame,
                    drag.start_x + delta.x,
                    drag.start_y + delta.y,
                    app.session.rig_auto_key,
                )
            }
            RigControlKind::Rotation => {
                let pose = evaluate_rig(&rig_snapshot, f32::from(app.session.current_frame), &[]);
                let Some(node_id) = control.target_node else {
                    return;
                };
                let Some(center) = pose
                    .node_world
                    .get(&node_id)
                    .map(|world| world.apply(Vec2::new(0.0, 0.0)))
                else {
                    return;
                };
                let parent_angle = rig_snapshot
                    .nodes
                    .iter()
                    .find(|node| node.node_id == node_id)
                    .and_then(|node| node.parent)
                    .and_then(|parent| pose.node_world.get(&parent))
                    .map(|world| world.a21.atan2(world.a11))
                    .unwrap_or(0.0);
                let angle = (pointer.y - center.y).atan2(pointer.x - center.x) - parent_angle;
                set_control_value(
                    &mut app.state.project,
                    q0rg_id,
                    drag.control_id,
                    app.session.current_frame,
                    angle,
                    app.session.rig_auto_key,
                )
            }
            RigControlKind::Slider => {
                let dx = pointer.x - drag.start_cursor.x;
                set_control_value(
                    &mut app.state.project,
                    q0rg_id,
                    drag.control_id,
                    app.session.current_frame,
                    drag.start_value + dx * 0.01,
                    app.session.rig_auto_key,
                )
            }
            RigControlKind::Toggle => false,
        };
        if changed {
            app.state.mark_dirty();
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        app.session.rig_bone_drag = None;
        app.session.rig_control_drag = None;
    }
}

fn nearest_control(
    app: &EditorApp,
    rig: &RigAsset,
    pose: &q0s_format::rig::RigPose,
    pointer: Vec2,
    view: &StageView,
) -> Option<u16> {
    rig.controls
        .iter()
        .filter(|control| app.session.rig_mode == RigMode::Pro || control.public_in_simple)
        .filter_map(|control| {
            let position = control_world_position(rig, pose, control.control_id)?;
            let distance = distance_screen(position, pointer, view);
            (distance <= CONTROL_HIT_PX).then_some((control.control_id, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

fn nearest_bone(
    rig: &RigAsset,
    pose: &q0s_format::rig::RigPose,
    pointer: Vec2,
    view: &StageView,
) -> Option<u16> {
    rig.nodes
        .iter()
        .filter_map(|node| {
            let world = *pose.node_world.get(&node.node_id)?;
            let a = world.apply(Vec2::new(0.0, 0.0));
            let b = world.apply(Vec2::new(node.length, 0.0));
            let distance = point_segment_distance(pointer, a, b) * view.scale;
            (distance <= BONE_HIT_PX).then_some((node.node_id, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}
fn paint_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    let Some(rig) = rig_for_q0rg(&app.state.project, app.session.current_q0rg_id) else {
        return;
    };
    let pose = evaluate_rig(rig, f32::from(app.session.current_frame), &[]);
    let accent = app.settings.theme.accent.to_color32();
    let bone_color = app.settings.theme.text.to_color32().gamma_multiply(0.72);
    let dim = app
        .settings
        .theme
        .text_dim
        .to_color32()
        .gamma_multiply(0.55);

    for node in &rig.nodes {
        let Some(world) = pose.node_world.get(&node.node_id).copied() else {
            continue;
        };
        let a = stage_to_screen(world.apply(Vec2::new(0.0, 0.0)), view);
        let b = stage_to_screen(world.apply(Vec2::new(node.length, 0.0)), view);
        let selected = app.session.rig_selected_node == Some(node.node_id);
        painter.line_segment(
            [a, b],
            Stroke::new(
                if selected { 3.0_f32 } else { 2.0_f32 },
                if selected { accent } else { bone_color },
            ),
        );
        painter.circle_filled(
            a,
            if selected { 5.0 } else { 4.0 },
            Color32::from_black_alpha(160),
        );
        painter.circle_stroke(
            a,
            if selected { 5.0 } else { 4.0 },
            Stroke::new(1.5_f32, if selected { accent } else { bone_color }),
        );
        painter.circle_filled(b, 3.0, if selected { accent } else { bone_color });
    }

    for control in &rig.controls {
        if app.session.rig_mode == RigMode::Simple && !control.public_in_simple {
            continue;
        }
        let Some(position) = control_world_position(rig, &pose, control.control_id) else {
            continue;
        };
        let screen = stage_to_screen(position, view);
        let selected = app.session.rig_selected_control == Some(control.control_id);
        let color = if selected { accent } else { dim };
        match control.kind {
            RigControlKind::Position2D => {
                painter.circle_stroke(
                    screen,
                    if selected { 8.0 } else { 6.5 },
                    Stroke::new(2.0_f32, color),
                );
                painter.line_segment(
                    [
                        screen + egui::vec2(-9.0, 0.0),
                        screen + egui::vec2(9.0, 0.0),
                    ],
                    Stroke::new(1.0_f32, color),
                );
                painter.line_segment(
                    [
                        screen + egui::vec2(0.0, -9.0),
                        screen + egui::vec2(0.0, 9.0),
                    ],
                    Stroke::new(1.0_f32, color),
                );
            }
            RigControlKind::Rotation => {
                painter.circle_stroke(screen, 11.0, Stroke::new(1.8_f32, color));
                painter.circle_filled(screen, 2.2, color);
            }
            RigControlKind::Slider => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(screen, egui::vec2(24.0, 7.0)),
                    2.0,
                    Stroke::new(1.4_f32, color),
                );
            }
            RigControlKind::Toggle => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(screen, egui::vec2(13.0, 13.0)),
                    2.0,
                    Stroke::new(1.6_f32, color),
                );
                if pose
                    .controls
                    .get(&control.control_id)
                    .is_some_and(|value| value.value >= 0.5)
                {
                    painter.circle_filled(screen, 3.2, color);
                }
            }
        }
    }

    if let (RigEditMode::AddBone, Some(start), Some(pointer)) = (
        app.session.rig_edit_mode,
        app.session.rig_pending_bone_start,
        painter
            .ctx()
            .pointer_hover_pos()
            .map(|pos| screen_to_stage(pos, view)),
    ) {
        painter.line_segment(
            [stage_to_screen(start, view), stage_to_screen(pointer, view)],
            Stroke::new(2.0_f32, accent.gamma_multiply(0.75)),
        );
    }
}

fn stage_to_screen(point: Vec2, view: &StageView) -> egui::Pos2 {
    egui::pos2(
        view.origin.x + point.x * view.scale,
        view.origin.y + point.y * view.scale,
    )
}

fn screen_to_stage(point: egui::Pos2, view: &StageView) -> Vec2 {
    Vec2::new(
        (point.x - view.origin.x) / view.scale,
        (point.y - view.origin.y) / view.scale,
    )
}

fn distance_screen(a: Vec2, b: Vec2, view: &StageView) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt() * view.scale
}

fn point_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq <= 1.0e-9 {
        let dx = point.x - a.x;
        let dy = point.y - a.y;
        return (dx * dx + dy * dy).sqrt();
    }
    let t = (((point.x - a.x) * abx + (point.y - a.y) * aby) / len_sq).clamp(0.0, 1.0);
    let px = a.x + abx * t;
    let py = a.y + aby * t;
    let dx = point.x - px;
    let dy = point.y - py;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Anchor, Layer, Path, Placement, ProjectMeta, Q0rg, Rgba, Tween};

    fn test_project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "rig test".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 1,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 20,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "parts".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            tx: 5.0,
                            ty: 6.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        }
    }

    #[test]
    fn simple_and_pro_are_editor_views_not_project_data() {
        let mut project = test_project();
        ensure_rig(&mut project, 1).unwrap();
        let before = project.clone();
        let mut mode = RigMode::Simple;
        assert_eq!(mode, RigMode::Simple);
        mode = RigMode::Pro;
        assert_eq!(mode, RigMode::Pro);
        assert_eq!(project, before);
    }

    #[test]
    fn drawing_parented_bones_builds_one_valid_hierarchy() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let child = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(45.0, 20.0),
            0,
        )
        .unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert_eq!(rig.nodes.len(), 2);
        assert_eq!(
            rig.nodes
                .iter()
                .find(|node| node.node_id == child)
                .unwrap()
                .parent,
            Some(root)
        );
        q0s_format::v2::validate(&project).unwrap();
    }

    #[test]
    fn binding_assigns_stable_instance_id_and_preserves_visible_pose() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        bind_selected_placement_to_node(
            &mut project,
            &Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            },
            root,
            0,
        )
        .unwrap();
        let instance_id = project.q0rgs[0].layers[0].placements[0].instance_id;
        assert_ne!(instance_id, 0);
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert_eq!(rig.nodes[0].binding.unwrap().instance_id, instance_id);
        let pose = evaluate_rig(rig, 0.0, &[]);
        let resolved = pose.binding_transform(instance_id).unwrap();
        let point = resolved.apply(Vec2::new(0.0, 0.0));
        assert!((point.x - 5.0).abs() < 1.0e-5);
        assert!((point.y - 6.0).abs() < 1.0e-5);
    }
    #[test]
    fn auto_key_writes_control_channels_without_changing_rest_value() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let control = add_rotation_control(&mut project, 1, root, true).unwrap();
        let rest = rig_for_q0rg(&project, 1)
            .unwrap()
            .controls
            .iter()
            .find(|c| c.control_id == control)
            .unwrap()
            .rest_value;
        assert!(set_control_value(&mut project, 1, control, 8, 0.75, true));
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert_eq!(
            rig.controls
                .iter()
                .find(|c| c.control_id == control)
                .unwrap()
                .rest_value,
            rest
        );
        let channel = rig
            .channels
            .iter()
            .find(|channel| channel.property == RigPropertyRef::ControlValue(control))
            .unwrap();
        assert_eq!(channel.keys[0].frame, 8);
        assert_eq!(channel.keys[0].value, 0.75);
    }

    #[test]
    fn selected_tip_can_create_two_bone_ik() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let mid = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(50.0, 10.0),
            0,
        )
        .unwrap();
        let tip = add_bone(
            &mut project,
            1,
            Some(mid),
            Vec2::new(50.0, 10.0),
            Vec2::new(55.0, 10.0),
            0,
        )
        .unwrap();
        let (_, target, pole) = make_two_bone_ik(&mut project, 1, tip, 0).unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert!(rig
            .controls
            .iter()
            .any(|control| control.control_id == target));
        assert!(rig
            .controls
            .iter()
            .any(|control| control.control_id == pole));
        assert!(matches!(
            rig.constraints[0],
            RigConstraint::TwoBoneIk { .. }
        ));
        q0s_format::v2::validate(&project).unwrap();
    }

    #[test]
    fn simple_fk_rotation_writes_node_channel_without_pro_control() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        assert!(set_node_rotation(&mut project, 1, root, 6, 0.8, true));
        let rig = rig_for_q0rg(&project, 1).unwrap();
        let channel = rig
            .channels
            .iter()
            .find(|channel| channel.property == RigPropertyRef::NodeRotation(root))
            .expect("simple FK rotation channel");
        assert_eq!(channel.keys[0].frame, 6);
        assert_eq!(channel.keys[0].value, 0.8);
    }

    #[test]
    fn direct_bone_rotation_routes_through_existing_rotation_control() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let control = add_rotation_control(&mut project, 1, root, true).unwrap();
        assert!(set_node_rotation(&mut project, 1, root, 4, 0.6, true));
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert!(rig
            .channels
            .iter()
            .any(|channel| channel.property == RigPropertyRef::ControlValue(control)));
        assert!(!rig
            .channels
            .iter()
            .any(|channel| channel.property == RigPropertyRef::NodeRotation(root)));
    }

    #[test]
    fn child_position_control_stores_parent_local_rest_coordinates() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let child = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(45.0, 10.0),
            0,
        )
        .unwrap();
        let control = add_position_control(&mut project, 1, child, true).unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        let node = rig.nodes.iter().find(|node| node.node_id == child).unwrap();
        let control = rig
            .controls
            .iter()
            .find(|candidate| candidate.control_id == control)
            .unwrap();
        assert!((control.rest_x - node.rest.tx).abs() < 1.0e-5);
        assert!((control.rest_y - node.rest.ty).abs() < 1.0e-5);
    }

    #[test]
    fn pro_reparent_helper_rejects_descendant_as_parent_candidate() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let child = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(20.0, 0.0),
            Vec2::new(40.0, 0.0),
            0,
        )
        .unwrap();
        let grandchild = add_bone(
            &mut project,
            1,
            Some(child),
            Vec2::new(40.0, 0.0),
            Vec2::new(50.0, 0.0),
            0,
        )
        .unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert!(node_is_descendant_of(rig, child, root));
        assert!(node_is_descendant_of(rig, grandchild, root));
        assert!(!node_is_descendant_of(rig, root, child));
    }

    #[test]
    fn ik_weight_is_keyframable_and_sampled_by_shared_solver() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let mid = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(50.0, 10.0),
            0,
        )
        .unwrap();
        let tip = add_bone(
            &mut project,
            1,
            Some(mid),
            Vec2::new(50.0, 10.0),
            Vec2::new(55.0, 10.0),
            0,
        )
        .unwrap();
        let (constraint, target, _) = make_two_bone_ik(&mut project, 1, tip, 0).unwrap();
        assert!(set_control_position(
            &mut project,
            1,
            target,
            0,
            35.0,
            24.0,
            false
        ));
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            0,
            0.0,
            true
        ));
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            10,
            1.0,
            true
        ));
        let rig = rig_for_q0rg(&project, 1).unwrap();
        let fk = evaluate_rig(rig, 0.0, &[]);
        let ik = evaluate_rig(rig, 10.0, &[]);
        assert_eq!(fk.constraint_weights[&constraint], 0.0);
        assert_eq!(ik.constraint_weights[&constraint], 1.0);
        let fk_tip = fk.node_world[&tip].apply(Vec2::new(0.0, 0.0));
        let ik_tip = ik.node_world[&tip].apply(Vec2::new(0.0, 0.0));
        assert!((ik_tip.x - 35.0).abs() < 1.0e-3);
        assert!((ik_tip.y - 24.0).abs() < 1.0e-3);
        assert!((fk_tip.x - ik_tip.x).abs() > 1.0 || (fk_tip.y - ik_tip.y).abs() > 1.0);
    }

    #[test]
    fn match_ik_target_to_fk_prevents_tip_pop_when_switching_to_ik() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let mid = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(50.0, 10.0),
            0,
        )
        .unwrap();
        let tip = add_bone(
            &mut project,
            1,
            Some(mid),
            Vec2::new(50.0, 10.0),
            Vec2::new(55.0, 10.0),
            0,
        )
        .unwrap();
        let (constraint, _, _) = make_two_bone_ik(&mut project, 1, tip, 0).unwrap();
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            0,
            0.0,
            false
        ));
        assert!(set_node_rotation(&mut project, 1, root, 0, 0.42, false));
        assert!(set_node_rotation(&mut project, 1, mid, 0, -0.73, false));
        let before = {
            let rig = rig_for_q0rg(&project, 1).unwrap();
            evaluate_rig(rig, 0.0, &[]).node_world[&tip].apply(Vec2::new(0.0, 0.0))
        };
        match_ik_target_to_fk(&mut project, 1, constraint, 0, false).unwrap();
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            0,
            1.0,
            false
        ));
        let after = {
            let rig = rig_for_q0rg(&project, 1).unwrap();
            evaluate_rig(rig, 0.0, &[]).node_world[&tip].apply(Vec2::new(0.0, 0.0))
        };
        assert!(
            (after.x - before.x).abs() < 1.0e-3,
            "before={before:?} after={after:?}"
        );
        assert!(
            (after.y - before.y).abs() < 1.0e-3,
            "before={before:?} after={after:?}"
        );
    }

    #[test]
    fn bake_ik_pose_to_fk_prevents_tip_pop_when_switching_to_fk() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            0,
        )
        .unwrap();
        let mid = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(30.0, 10.0),
            Vec2::new(50.0, 10.0),
            0,
        )
        .unwrap();
        let tip = add_bone(
            &mut project,
            1,
            Some(mid),
            Vec2::new(50.0, 10.0),
            Vec2::new(55.0, 10.0),
            0,
        )
        .unwrap();
        let (constraint, target, pole) = make_two_bone_ik(&mut project, 1, tip, 0).unwrap();
        assert!(set_control_position(
            &mut project,
            1,
            target,
            0,
            36.0,
            28.0,
            false
        ));
        assert!(set_control_position(
            &mut project,
            1,
            pole,
            0,
            12.0,
            34.0,
            false
        ));
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            0,
            1.0,
            false
        ));
        let before = {
            let rig = rig_for_q0rg(&project, 1).unwrap();
            evaluate_rig(rig, 0.0, &[]).node_world[&tip].apply(Vec2::new(0.0, 0.0))
        };
        bake_ik_pose_to_fk(&mut project, 1, constraint, 0, false).unwrap();
        assert!(set_constraint_weight(
            &mut project,
            1,
            constraint,
            0,
            0.0,
            false
        ));
        let after = {
            let rig = rig_for_q0rg(&project, 1).unwrap();
            evaluate_rig(rig, 0.0, &[]).node_world[&tip].apply(Vec2::new(0.0, 0.0))
        };
        assert!(
            (after.x - before.x).abs() < 1.0e-3,
            "before={before:?} after={after:?}"
        );
        assert!(
            (after.y - before.y).abs() < 1.0e-3,
            "before={before:?} after={after:?}"
        );
    }

    #[test]
    fn pro_control_builder_uses_one_runtime_model_for_all_control_kinds() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let joystick = add_control(&mut project, 1, RigControlKind::Position2D, None).unwrap();
        let dial = add_control(&mut project, 1, RigControlKind::Rotation, Some(root)).unwrap();
        let slider = add_control(&mut project, 1, RigControlKind::Slider, None).unwrap();
        let toggle = add_control(&mut project, 1, RigControlKind::Toggle, None).unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert_eq!(
            rig.controls
                .iter()
                .find(|c| c.control_id == joystick)
                .unwrap()
                .kind,
            RigControlKind::Position2D
        );
        assert_eq!(
            rig.controls
                .iter()
                .find(|c| c.control_id == dial)
                .unwrap()
                .target_node,
            Some(root)
        );
        assert_eq!(
            rig.controls
                .iter()
                .find(|c| c.control_id == slider)
                .unwrap()
                .kind,
            RigControlKind::Slider
        );
        assert_eq!(
            rig.controls
                .iter()
                .find(|c| c.control_id == toggle)
                .unwrap()
                .kind,
            RigControlKind::Toggle
        );
        q0s_format::v2::validate(&project).unwrap();
    }

    #[test]
    fn transform_constraint_cycle_helper_rejects_back_edge_through_parent_chain() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let child = add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(20.0, 0.0),
            Vec2::new(40.0, 0.0),
            0,
        )
        .unwrap();
        let rig = rig_for_q0rg(&project, 1).unwrap();
        assert!(transform_target_would_cycle(rig, root, child, None));
        assert!(!transform_target_would_cycle(rig, child, root, None));
    }

    #[test]
    fn toggle_control_can_drive_selected_bone_property() {
        let mut project = test_project();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            0,
        )
        .unwrap();
        let toggle = add_control(&mut project, 1, RigControlKind::Toggle, None).unwrap();
        add_master_driver(
            &mut project,
            1,
            toggle,
            RigPropertyRef::NodeTy(root),
            0.0,
            12.0,
        )
        .unwrap();
        assert!(set_control_value(&mut project, 1, toggle, 0, 1.0, false));
        let rig = rig_for_q0rg(&project, 1).unwrap();
        let pose = evaluate_rig(rig, 0.0, &[]);
        assert!((pose.node_local[&root].ty - 12.0).abs() < 1.0e-5);
    }

    fn make_test_vector_deformable(project: &mut ProjectV2) {
        let vector = project
            .assets
            .iter_mut()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == 1 => Some(vector),
                _ => None,
            })
            .unwrap();
        vector.fill = Some(Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        });
        vector.paths = vec![Path {
            closed: true,
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: Some(Vec2::new(-1.0, 0.0)),
                    out_handle: Some(Vec2::new(1.0, 0.0)),
                },
                Anchor {
                    point: Vec2::new(30.0, 0.0),
                    in_handle: Some(Vec2::new(29.0, 0.0)),
                    out_handle: Some(Vec2::new(31.0, 0.0)),
                },
                Anchor {
                    point: Vec2::new(30.0, 12.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(0.0, 12.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
        }];
    }

    fn test_placement_selection() -> Selection {
        Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        }
    }

    #[test]
    fn auto_skin_authoring_assigns_stable_identity_and_normalized_weights() {
        let mut project = test_project();
        make_test_vector_deformable(&mut project);
        ensure_rig(&mut project, 1).unwrap();
        let root = add_bone(
            &mut project,
            1,
            None,
            Vec2::new(5.0, 6.0),
            Vec2::new(20.0, 6.0),
            0,
        )
        .unwrap();
        add_bone(
            &mut project,
            1,
            Some(root),
            Vec2::new(20.0, 6.0),
            Vec2::new(35.0, 6.0),
            0,
        )
        .unwrap();
        let id = create_skin_deformer_from_selection(&mut project, &test_placement_selection(), 0)
            .unwrap();
        let instance_id = project.q0rgs[0].layers[0].placements[0].instance_id;
        assert_ne!(instance_id, 0);
        let rig = rig_for_q0rg(&project, 1).unwrap();
        let RigDeformer::Skin {
            instance_id: bound,
            bind_transform,
            anchors,
            ..
        } = rig
            .deformers
            .iter()
            .find(|deformer| deformer.id() == id)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(*bound, instance_id);
        assert!((bind_transform.tx - 5.0).abs() < 1.0e-6);
        assert!((bind_transform.ty - 6.0).abs() < 1.0e-6);
        assert_eq!(anchors.len(), 4);
        for anchor in anchors {
            let sum = anchor
                .weights
                .iter()
                .map(|weight| weight.weight)
                .sum::<f32>();
            assert!((sum - 1.0).abs() < 1.0e-5);
            assert!((1..=4).contains(&anchor.weights.len()));
        }
        q0s_format::v2::validate(&project).unwrap();
    }

    #[test]
    fn bend_and_cage_authoring_use_public_controls_and_cleanup_only_their_controls() {
        for cage in [false, true] {
            let mut project = test_project();
            make_test_vector_deformable(&mut project);
            ensure_rig(&mut project, 1).unwrap();
            let unrelated = add_control(&mut project, 1, RigControlKind::Slider, None).unwrap();
            let id = if cage {
                create_cage_deformer_from_selection(&mut project, &test_placement_selection(), 0)
                    .unwrap()
            } else {
                create_bend_deformer_from_selection(&mut project, &test_placement_selection(), 0)
                    .unwrap()
            };
            let generated = {
                let rig = rig_for_q0rg(&project, 1).unwrap();
                let deformer = rig
                    .deformers
                    .iter()
                    .find(|deformer| deformer.id() == id)
                    .unwrap();
                let ids = match deformer {
                    RigDeformer::Bend {
                        start_control,
                        middle_control,
                        end_control,
                        ..
                    } => vec![*start_control, *middle_control, *end_control],
                    RigDeformer::Cage { controls, .. } => controls.to_vec(),
                    RigDeformer::Skin { .. } => unreachable!(),
                };
                assert!(ids.iter().all(|id| rig
                    .controls
                    .iter()
                    .any(|control| control.control_id == *id && control.public_in_simple)));
                ids
            };
            q0s_format::v2::validate(&project).unwrap();
            assert!(remove_deformer(&mut project, 1, id));
            let rig = rig_for_q0rg(&project, 1).unwrap();
            assert!(rig
                .controls
                .iter()
                .any(|control| control.control_id == unrelated));
            assert!(generated
                .iter()
                .all(|id| rig.controls.iter().all(|control| control.control_id != *id)));
            q0s_format::v2::validate(&project).unwrap();
        }
    }
}
