//! Shared deterministic 2D rig evaluator used by q0editor and q0player.
//!
//! Rig data is stored as a metadata `Asset::Rig`. The asset itself never renders;
//! it resolves bone/control state into affine overrides for display-object tracks.

use std::collections::HashMap;

use crate::transform::Affine;
use crate::v2::{
    Asset, Easing, ProjectV2, RigAsset, RigBinding, RigConstraint, RigControlKind, RigDeformer,
    RigKey, RigPoseBlendMode, RigPoseValue, RigPropertyRef, RigSkinAnchorWeights, RigSkinWeight,
    Target, Transform2D, Vec2, VectorAsset,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigControlValue {
    pub x: f32,
    pub y: f32,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RigControlOverride {
    Position { control_id: u16, x: f32, y: f32 },
    Value { control_id: u16, value: f32 },
    Pose { pose_id: u16, weight: f32 },
}

/// Ephemeral player/runtime control state keyed by the q0rg that owns each rig.
/// It never mutates the serialized project and is deliberately separate from
/// authored timeline channels.
pub type RigRuntimeOverrides = HashMap<u16, Vec<RigControlOverride>>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedRigBinding {
    pub node_id: u16,
    pub instance_id: u32,
    pub transform: Affine,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RigPose {
    pub node_local: HashMap<u16, Transform2D>,
    pub node_world: HashMap<u16, Affine>,
    pub controls: HashMap<u16, RigControlValue>,
    pub constraint_weights: HashMap<u16, f32>,
    pub bindings: Vec<ResolvedRigBinding>,
}

impl RigPose {
    pub fn binding_transform(&self, instance_id: u32) -> Option<Affine> {
        if instance_id == 0 {
            return None;
        }
        self.bindings
            .iter()
            .find(|binding| binding.instance_id == instance_id)
            .map(|binding| binding.transform)
    }
}

pub fn rig_for_q0rg(project: &ProjectV2, q0rg_id: u16) -> Option<&RigAsset> {
    project.assets.iter().find_map(|asset| match asset {
        Asset::Rig(rig) if rig.owner_q0rg_id == q0rg_id => Some(rig),
        _ => None,
    })
}

pub fn rig_for_q0rg_mut(project: &mut ProjectV2, q0rg_id: u16) -> Option<&mut RigAsset> {
    project.assets.iter_mut().find_map(|asset| match asset {
        Asset::Rig(rig) if rig.owner_q0rg_id == q0rg_id => Some(rig),
        _ => None,
    })
}

pub fn evaluate_rig(rig: &RigAsset, frame: f32, overrides: &[RigControlOverride]) -> RigPose {
    let mut controls = rig
        .controls
        .iter()
        .map(|control| {
            (
                control.control_id,
                RigControlValue {
                    x: control.rest_x,
                    y: control.rest_y,
                    value: control.rest_value,
                },
            )
        })
        .collect::<HashMap<_, _>>();

    let mut locals = rig
        .nodes
        .iter()
        .map(|node| (node.node_id, node.rest))
        .collect::<HashMap<_, _>>();
    let mut constraint_weights = rig
        .constraints
        .iter()
        .map(|constraint| {
            let weight = match *constraint {
                RigConstraint::RotationLimit { .. }
                | RigConstraint::PositionLimit { .. }
                | RigConstraint::Transform { .. } => 1.0,
                RigConstraint::Aim { weight, .. }
                | RigConstraint::Distance { weight, .. }
                | RigConstraint::TwoBoneIk { weight, .. } => weight,
            };
            (constraint.id(), weight)
        })
        .collect::<HashMap<_, _>>();

    for channel in &rig.channels {
        let Some(value) = sample_keys(&channel.keys, frame) else {
            continue;
        };
        match channel.property {
            RigPropertyRef::ControlX(id) => {
                if let Some(control) = controls.get_mut(&id) {
                    control.x = value;
                }
            }
            RigPropertyRef::ControlY(id) => {
                if let Some(control) = controls.get_mut(&id) {
                    control.y = value;
                }
            }
            RigPropertyRef::ControlValue(id) => {
                if let Some(control) = controls.get_mut(&id) {
                    control.value = value;
                }
            }
            RigPropertyRef::NodeTx(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.tx = value;
                }
            }
            RigPropertyRef::NodeTy(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.ty = value;
                }
            }
            RigPropertyRef::NodeRotation(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.rotation = value;
                }
            }
            RigPropertyRef::NodeScaleX(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.sx = value.max(1.0e-4);
                }
            }
            RigPropertyRef::NodeScaleY(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.sy = value.max(1.0e-4);
                }
            }
            RigPropertyRef::ConstraintWeight(id) => {
                if let Some(weight) = constraint_weights.get_mut(&id) {
                    *weight = value.clamp(0.0, 1.0);
                }
            }
        }
    }

    // Runtime/public-control overrides intentionally win over authored timeline keys.
    for override_value in overrides {
        match *override_value {
            RigControlOverride::Position { control_id, x, y } => {
                if let Some(control) = controls.get_mut(&control_id) {
                    control.x = x;
                    control.y = y;
                }
            }
            RigControlOverride::Value { control_id, value } => {
                if let Some(control) = controls.get_mut(&control_id) {
                    control.value = value;
                }
            }
            RigControlOverride::Pose { .. } => {}
        }
    }

    let mut pose_drivers = rig.pose_drivers.iter().collect::<Vec<_>>();
    pose_drivers.sort_by_key(|driver| driver.driver_id);
    for driver in pose_drivers {
        let Some(source) = controls.get(&driver.source_control).copied() else {
            continue;
        };
        let span = driver.source_max - driver.source_min;
        if span.abs() < 1.0e-9 {
            continue;
        }
        let t = ((source.value - driver.source_min) / span).clamp(0.0, 1.0);
        let weight =
            (driver.weight_min + (driver.weight_max - driver.weight_min) * t).clamp(0.0, 1.0);
        if weight <= 0.0 {
            continue;
        }
        let Some(preset) = rig.poses.iter().find(|pose| pose.pose_id == driver.pose_id) else {
            continue;
        };
        for pose_value in &preset.values {
            let mut state = RigPropertyState {
                controls: &mut controls,
                locals: &mut locals,
                constraint_weights: &mut constraint_weights,
            };
            apply_pose_driver_value(rig, *pose_value, driver.mode, weight, &mut state);
        }
    }

    let mut runtime_poses = overrides
        .iter()
        .filter_map(|value| match *value {
            RigControlOverride::Pose { pose_id, weight } => Some((pose_id, weight)),
            RigControlOverride::Position { .. } | RigControlOverride::Value { .. } => None,
        })
        .collect::<Vec<_>>();
    runtime_poses.sort_by_key(|(pose_id, _)| *pose_id);
    for (pose_id, weight) in runtime_poses {
        let weight = weight.clamp(0.0, 1.0);
        if weight <= 0.0 {
            continue;
        }
        let Some(preset) = rig.poses.iter().find(|pose| pose.pose_id == pose_id) else {
            continue;
        };
        for pose_value in &preset.values {
            let mut state = RigPropertyState {
                controls: &mut controls,
                locals: &mut locals,
                constraint_weights: &mut constraint_weights,
            };
            apply_pose_driver_value(
                rig,
                *pose_value,
                RigPoseBlendMode::Override,
                weight,
                &mut state,
            );
        }
    }

    for control in &rig.controls {
        let Some(target_node) = control.target_node else {
            continue;
        };
        let Some(value) = controls.get(&control.control_id).copied() else {
            continue;
        };
        let Some(local) = locals.get_mut(&target_node) else {
            continue;
        };
        match control.kind {
            RigControlKind::Position2D => {
                local.tx = value.x;
                local.ty = value.y;
            }
            RigControlKind::Rotation => local.rotation = value.value,
            RigControlKind::Slider | RigControlKind::Toggle => {}
        }
    }

    // Master sliders resolve after direct controls and before constraints. Each target
    // has at most one driver by validation, so evaluation order cannot change the pose.
    for driver in &rig.drivers {
        let Some(source) = controls.get(&driver.source_control).copied() else {
            continue;
        };
        let span = driver.source_max - driver.source_min;
        if span.abs() < 1.0e-9 {
            continue;
        }
        let t = ((source.value - driver.source_min) / span).clamp(0.0, 1.0);
        let value = driver.target_min + (driver.target_max - driver.target_min) * t;
        match driver.target {
            RigPropertyRef::NodeTx(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.tx = value;
                }
            }
            RigPropertyRef::NodeTy(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.ty = value;
                }
            }
            RigPropertyRef::NodeRotation(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.rotation = value;
                }
            }
            RigPropertyRef::NodeScaleX(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.sx = value.max(1.0e-4);
                }
            }
            RigPropertyRef::NodeScaleY(id) => {
                if let Some(local) = locals.get_mut(&id) {
                    local.sy = value.max(1.0e-4);
                }
            }
            RigPropertyRef::ConstraintWeight(id) => {
                if let Some(weight) = constraint_weights.get_mut(&id) {
                    *weight = value.clamp(0.0, 1.0);
                }
            }
            RigPropertyRef::ControlX(_)
            | RigPropertyRef::ControlY(_)
            | RigPropertyRef::ControlValue(_) => {}
        }
    }

    // Transform/aim/distance constraints may depend on another solved node. Run a
    // bounded deterministic relaxation pass count equal to the node count. A valid
    // acyclic dependency graph therefore settles exactly without frame-rate state.
    let passes = rig.nodes.len().max(1);
    let mut ordered_constraints = rig.constraints.iter().collect::<Vec<_>>();
    ordered_constraints.sort_by_key(|constraint| constraint.id());
    for _ in 0..passes {
        let world = build_world_map(rig, &locals);
        for constraint in &ordered_constraints {
            match **constraint {
                RigConstraint::Transform {
                    node_id,
                    target_node,
                    position_weight,
                    rotation_weight,
                    ..
                } => {
                    let Some(target_world) = world.get(&target_node).copied() else {
                        continue;
                    };
                    let parent_world = rig
                        .nodes
                        .iter()
                        .find(|node| node.node_id == node_id)
                        .and_then(|node| node.parent)
                        .and_then(|parent| world.get(&parent).copied())
                        .unwrap_or(Affine::IDENTITY);
                    let Some(parent_inv) = parent_world.inverse() else {
                        continue;
                    };
                    let target_local = Affine::compose(parent_inv, target_world);
                    if let Some(local) = locals.get_mut(&node_id) {
                        let pw = position_weight.clamp(0.0, 1.0);
                        let rw = rotation_weight.clamp(0.0, 1.0);
                        local.tx += (target_local.tx - local.tx) * pw;
                        local.ty += (target_local.ty - local.ty) * pw;
                        let target_angle = target_local.a21.atan2(target_local.a11);
                        local.rotation = lerp_angle(local.rotation, target_angle, rw);
                    }
                }
                RigConstraint::Aim {
                    constraint_id,
                    node_id,
                    target_control,
                    angle_offset,
                    weight,
                } => {
                    let effective_weight = constraint_weights
                        .get(&constraint_id)
                        .copied()
                        .unwrap_or(weight)
                        .clamp(0.0, 1.0);
                    let Some(target) = controls
                        .get(&target_control)
                        .map(|value| Vec2::new(value.x, value.y))
                    else {
                        continue;
                    };
                    let Some(node_world) = world.get(&node_id).copied() else {
                        continue;
                    };
                    let origin = node_world.apply(Vec2::new(0.0, 0.0));
                    let world_angle =
                        (target.y - origin.y).atan2(target.x - origin.x) + angle_offset;
                    let parent_angle = rig
                        .nodes
                        .iter()
                        .find(|node| node.node_id == node_id)
                        .and_then(|node| node.parent)
                        .and_then(|parent| world.get(&parent))
                        .map(|affine| affine.a21.atan2(affine.a11))
                        .unwrap_or(0.0);
                    if let Some(local) = locals.get_mut(&node_id) {
                        local.rotation = lerp_angle(
                            local.rotation,
                            world_angle - parent_angle,
                            effective_weight,
                        );
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
                    let effective_weight = constraint_weights
                        .get(&constraint_id)
                        .copied()
                        .unwrap_or(weight)
                        .clamp(0.0, 1.0);
                    let Some(target) = controls
                        .get(&target_control)
                        .map(|value| Vec2::new(value.x, value.y))
                    else {
                        continue;
                    };
                    let Some(node_world) = world.get(&node_id).copied() else {
                        continue;
                    };
                    let origin = node_world.apply(Vec2::new(0.0, 0.0));
                    let dx = origin.x - target.x;
                    let dy = origin.y - target.y;
                    let distance = (dx * dx + dy * dy).sqrt();
                    if distance < 1.0e-6 {
                        continue;
                    }
                    let clamped = distance.clamp(min_distance, max_distance);
                    let desired = Vec2::new(
                        target.x + dx / distance * clamped,
                        target.y + dy / distance * clamped,
                    );
                    let parent_world = rig
                        .nodes
                        .iter()
                        .find(|node| node.node_id == node_id)
                        .and_then(|node| node.parent)
                        .and_then(|parent| world.get(&parent).copied())
                        .unwrap_or(Affine::IDENTITY);
                    let Some(parent_inv) = parent_world.inverse() else {
                        continue;
                    };
                    let desired_local = parent_inv.apply(desired);
                    if let Some(local) = locals.get_mut(&node_id) {
                        local.tx += (desired_local.x - local.tx) * effective_weight;
                        local.ty += (desired_local.y - local.ty) * effective_weight;
                    }
                }
                RigConstraint::RotationLimit { .. }
                | RigConstraint::PositionLimit { .. }
                | RigConstraint::TwoBoneIk { .. } => {}
            }
        }
    }

    // Solve IK in declaration order. Validation guarantees each chain is structurally valid.
    for constraint in &rig.constraints {
        if let RigConstraint::TwoBoneIk {
            root_node,
            mid_node,
            tip_node: _,
            target_control,
            pole_control,
            weight: authored_weight,
            allow_stretch,
            max_stretch,
            constraint_id,
        } = *constraint
        {
            let weight = constraint_weights
                .get(&constraint_id)
                .copied()
                .unwrap_or(authored_weight);
            solve_two_bone_ik(
                rig,
                &mut locals,
                &controls,
                TwoBoneIkParams {
                    root_id: root_node,
                    mid_id: mid_node,
                    target_control,
                    pole_control,
                    weight,
                    allow_stretch,
                    max_stretch,
                },
            );
        }
    }

    for constraint in &rig.constraints {
        if let RigConstraint::PositionLimit {
            node_id,
            min_x,
            max_x,
            min_y,
            max_y,
            ..
        } = *constraint
        {
            if let Some(local) = locals.get_mut(&node_id) {
                local.tx = local.tx.clamp(min_x, max_x);
                local.ty = local.ty.clamp(min_y, max_y);
            }
        }
    }

    // Limits are applied after IK so a constrained limb cannot escape its authored range.
    for constraint in &rig.constraints {
        if let RigConstraint::RotationLimit {
            node_id,
            min_radians,
            max_radians,
            ..
        } = *constraint
        {
            if let Some(local) = locals.get_mut(&node_id) {
                local.rotation = local.rotation.clamp(min_radians, max_radians);
            }
        }
    }

    let node_world = build_world_map(rig, &locals);
    let bindings = rig
        .nodes
        .iter()
        .filter_map(|node| {
            let binding = node.binding?;
            let world = *node_world.get(&node.node_id)?;
            Some(resolved_binding(node.node_id, binding, world))
        })
        .collect();

    RigPose {
        node_local: locals,
        node_world,
        controls,
        constraint_weights,
        bindings,
    }
}

fn resolved_binding(node_id: u16, binding: RigBinding, node_world: Affine) -> ResolvedRigBinding {
    ResolvedRigBinding {
        node_id,
        instance_id: binding.instance_id,
        transform: Affine::compose(node_world, binding.bind_offset),
    }
}

fn sample_keys(keys: &[RigKey], frame: f32) -> Option<f32> {
    if keys.is_empty() {
        return None;
    }
    let mut before: Option<&RigKey> = None;
    let mut after: Option<&RigKey> = None;
    for key in keys {
        let key_frame = f32::from(key.frame);
        if key_frame <= frame && before.is_none_or(|current| current.frame < key.frame) {
            before = Some(key);
        }
        if key_frame > frame && after.is_none_or(|current| current.frame > key.frame) {
            after = Some(key);
        }
    }
    match (before, after) {
        (Some(a), Some(b)) => {
            let span = f32::from(b.frame - a.frame).max(1.0);
            let t = ((frame - f32::from(a.frame)) / span).clamp(0.0, 1.0);
            let t = a.easing.sample(t);
            Some(a.value + (b.value - a.value) * t)
        }
        (Some(a), None) => Some(a.value),
        (None, Some(b)) => Some(b.value),
        (None, None) => None,
    }
}

fn rest_property_value(rig: &RigAsset, property: RigPropertyRef) -> Option<f32> {
    match property {
        RigPropertyRef::ControlX(id) => rig
            .controls
            .iter()
            .find(|c| c.control_id == id)
            .map(|c| c.rest_x),
        RigPropertyRef::ControlY(id) => rig
            .controls
            .iter()
            .find(|c| c.control_id == id)
            .map(|c| c.rest_y),
        RigPropertyRef::ControlValue(id) => rig
            .controls
            .iter()
            .find(|c| c.control_id == id)
            .map(|c| c.rest_value),
        RigPropertyRef::NodeTx(id) => rig
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .map(|n| n.rest.tx),
        RigPropertyRef::NodeTy(id) => rig
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .map(|n| n.rest.ty),
        RigPropertyRef::NodeRotation(id) => rig
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .map(|n| n.rest.rotation),
        RigPropertyRef::NodeScaleX(id) => rig
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .map(|n| n.rest.sx),
        RigPropertyRef::NodeScaleY(id) => rig
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .map(|n| n.rest.sy),
        RigPropertyRef::ConstraintWeight(id) => {
            rig.constraints
                .iter()
                .find(|c| c.id() == id)
                .map(|constraint| match *constraint {
                    RigConstraint::Aim { weight, .. }
                    | RigConstraint::Distance { weight, .. }
                    | RigConstraint::TwoBoneIk { weight, .. } => weight,
                    RigConstraint::RotationLimit { .. }
                    | RigConstraint::PositionLimit { .. }
                    | RigConstraint::Transform { .. } => 1.0,
                })
        }
    }
}

fn current_property_value(
    property: RigPropertyRef,
    controls: &HashMap<u16, RigControlValue>,
    locals: &HashMap<u16, Transform2D>,
    constraint_weights: &HashMap<u16, f32>,
) -> Option<f32> {
    match property {
        RigPropertyRef::ControlX(id) => controls.get(&id).map(|v| v.x),
        RigPropertyRef::ControlY(id) => controls.get(&id).map(|v| v.y),
        RigPropertyRef::ControlValue(id) => controls.get(&id).map(|v| v.value),
        RigPropertyRef::NodeTx(id) => locals.get(&id).map(|v| v.tx),
        RigPropertyRef::NodeTy(id) => locals.get(&id).map(|v| v.ty),
        RigPropertyRef::NodeRotation(id) => locals.get(&id).map(|v| v.rotation),
        RigPropertyRef::NodeScaleX(id) => locals.get(&id).map(|v| v.sx),
        RigPropertyRef::NodeScaleY(id) => locals.get(&id).map(|v| v.sy),
        RigPropertyRef::ConstraintWeight(id) => constraint_weights.get(&id).copied(),
    }
}

fn set_property_value(
    property: RigPropertyRef,
    value: f32,
    controls: &mut HashMap<u16, RigControlValue>,
    locals: &mut HashMap<u16, Transform2D>,
    constraint_weights: &mut HashMap<u16, f32>,
) {
    match property {
        RigPropertyRef::ControlX(id) => {
            if let Some(v) = controls.get_mut(&id) {
                v.x = value;
            }
        }
        RigPropertyRef::ControlY(id) => {
            if let Some(v) = controls.get_mut(&id) {
                v.y = value;
            }
        }
        RigPropertyRef::ControlValue(id) => {
            if let Some(v) = controls.get_mut(&id) {
                v.value = value;
            }
        }
        RigPropertyRef::NodeTx(id) => {
            if let Some(v) = locals.get_mut(&id) {
                v.tx = value;
            }
        }
        RigPropertyRef::NodeTy(id) => {
            if let Some(v) = locals.get_mut(&id) {
                v.ty = value;
            }
        }
        RigPropertyRef::NodeRotation(id) => {
            if let Some(v) = locals.get_mut(&id) {
                v.rotation = value;
            }
        }
        RigPropertyRef::NodeScaleX(id) => {
            if let Some(v) = locals.get_mut(&id) {
                v.sx = value.max(1.0e-4);
            }
        }
        RigPropertyRef::NodeScaleY(id) => {
            if let Some(v) = locals.get_mut(&id) {
                v.sy = value.max(1.0e-4);
            }
        }
        RigPropertyRef::ConstraintWeight(id) => {
            if let Some(v) = constraint_weights.get_mut(&id) {
                *v = value.clamp(0.0, 1.0);
            }
        }
    }
}

struct RigPropertyState<'a> {
    controls: &'a mut HashMap<u16, RigControlValue>,
    locals: &'a mut HashMap<u16, Transform2D>,
    constraint_weights: &'a mut HashMap<u16, f32>,
}

fn apply_pose_driver_value(
    rig: &RigAsset,
    pose_value: RigPoseValue,
    mode: RigPoseBlendMode,
    weight: f32,
    state: &mut RigPropertyState<'_>,
) {
    let property = pose_value.property;
    let Some(current) = current_property_value(
        property,
        state.controls,
        state.locals,
        state.constraint_weights,
    ) else {
        return;
    };
    let value = match mode {
        RigPoseBlendMode::Override => {
            if matches!(property, RigPropertyRef::NodeRotation(_)) {
                lerp_angle(current, pose_value.value, weight)
            } else {
                current + (pose_value.value - current) * weight
            }
        }
        RigPoseBlendMode::Additive => {
            let Some(rest) = rest_property_value(rig, property) else {
                return;
            };
            current + (pose_value.value - rest) * weight
        }
    };
    if value.is_finite() {
        set_property_value(
            property,
            value,
            state.controls,
            state.locals,
            state.constraint_weights,
        );
    }
}

pub fn blended_pose_values(
    rig: &RigAsset,
    frame: f32,
    pose_id: u16,
    weight: f32,
    mode: RigPoseBlendMode,
) -> Option<Vec<RigPoseValue>> {
    let preset = rig.poses.iter().find(|pose| pose.pose_id == pose_id)?;
    let current_pose = evaluate_rig(rig, frame, &[]);
    let weight = weight.clamp(0.0, 1.0);
    let mut values = Vec::with_capacity(preset.values.len());
    for pose_value in &preset.values {
        let Some(current) = current_property_value(
            pose_value.property,
            &current_pose.controls,
            &current_pose.node_local,
            &current_pose.constraint_weights,
        ) else {
            continue;
        };
        let value = match mode {
            RigPoseBlendMode::Override => {
                if matches!(pose_value.property, RigPropertyRef::NodeRotation(_)) {
                    lerp_angle(current, pose_value.value, weight)
                } else {
                    current + (pose_value.value - current) * weight
                }
            }
            RigPoseBlendMode::Additive => {
                let rest = rest_property_value(rig, pose_value.property)?;
                current + (pose_value.value - rest) * weight
            }
        };
        values.push(RigPoseValue {
            property: pose_value.property,
            value,
        });
    }
    values.sort_by_key(|value| property_sort_key(value.property));
    Some(values)
}

pub fn mirror_pose_values(rig: &RigAsset, pose_id: u16) -> Option<Vec<RigPoseValue>> {
    let pose = rig.poses.iter().find(|pose| pose.pose_id == pose_id)?;
    let mut out = Vec::with_capacity(pose.values.len());
    for value in &pose.values {
        let mut mirrored = *value;
        if let Some(pair) = rig
            .mirror_pairs
            .iter()
            .find(|pair| pair.left == value.property || pair.right == value.property)
        {
            if pair.left == value.property {
                mirrored.property = pair.right;
                mirrored.value = value.value * pair.multiplier + pair.offset;
            } else {
                mirrored.property = pair.left;
                mirrored.value = (value.value - pair.offset) / pair.multiplier;
            }
        }
        out.push(mirrored);
    }
    out.sort_by_key(|value| property_sort_key(value.property));
    Some(out)
}

fn property_sort_key(property: RigPropertyRef) -> (u8, u16) {
    match property {
        RigPropertyRef::NodeTx(id) => (0, id),
        RigPropertyRef::NodeTy(id) => (1, id),
        RigPropertyRef::NodeRotation(id) => (2, id),
        RigPropertyRef::NodeScaleX(id) => (3, id),
        RigPropertyRef::NodeScaleY(id) => (4, id),
        RigPropertyRef::ControlX(id) => (5, id),
        RigPropertyRef::ControlY(id) => (6, id),
        RigPropertyRef::ControlValue(id) => (7, id),
        RigPropertyRef::ConstraintWeight(id) => (8, id),
    }
}

pub fn resolved_variant_target(
    rig: &RigAsset,
    pose: &RigPose,
    instance_id: u32,
    authored_target: Target,
) -> Target {
    let Some(variant) = rig
        .variants
        .iter()
        .find(|variant| variant.instance_id == instance_id)
    else {
        return authored_target;
    };
    let Some(control) = pose.controls.get(&variant.source_control) else {
        return authored_target;
    };
    if variant.choices.is_empty() || !control.value.is_finite() {
        return authored_target;
    }
    let index = control
        .value
        .round()
        .clamp(0.0, (variant.choices.len() - 1) as f32) as usize;
    variant.choices[index].target
}

#[derive(Clone, Copy)]
struct TwoBoneIkParams {
    root_id: u16,
    mid_id: u16,
    target_control: u16,
    pole_control: Option<u16>,
    weight: f32,
    allow_stretch: bool,
    max_stretch: f32,
}

fn solve_two_bone_ik(
    rig: &RigAsset,
    locals: &mut HashMap<u16, Transform2D>,
    controls: &HashMap<u16, RigControlValue>,
    params: TwoBoneIkParams,
) {
    let TwoBoneIkParams {
        root_id,
        mid_id,
        target_control,
        pole_control,
        weight,
        allow_stretch,
        max_stretch,
    } = params;
    let Some(root_node) = rig.nodes.iter().find(|node| node.node_id == root_id) else {
        return;
    };
    let Some(mid_node) = rig.nodes.iter().find(|node| node.node_id == mid_id) else {
        return;
    };
    let Some(target) = controls.get(&target_control).copied() else {
        return;
    };
    let Some(root_local) = locals.get(&root_id).copied() else {
        return;
    };
    let Some(mid_local) = locals.get(&mid_id).copied() else {
        return;
    };

    let parent_world = root_node
        .parent
        .and_then(|parent| world_for_node(rig, locals, parent, &mut HashMap::new(), 0))
        .unwrap_or(Affine::IDENTITY);
    let Some(parent_inverse) = parent_world.inverse() else {
        return;
    };
    let target_local = parent_inverse.apply(Vec2::new(target.x, target.y));
    let pole_local = pole_control
        .and_then(|id| controls.get(&id))
        .map(|pole| parent_inverse.apply(Vec2::new(pole.x, pole.y)));

    let root_position = Vec2::new(root_local.tx, root_local.ty);
    let mut dx = target_local.x - root_position.x;
    let mut dy = target_local.y - root_position.y;
    let mut distance = (dx * dx + dy * dy).sqrt();
    if distance < 1.0e-5 {
        dx = 1.0e-5;
        dy = 0.0;
        distance = 1.0e-5;
    }

    let l1 = root_node.length.max(1.0e-4);
    let l2 = mid_node.length.max(1.0e-4);
    let rest_reach = l1 + l2;
    let stretch = if allow_stretch && distance > rest_reach {
        (distance / rest_reach).min(max_stretch.max(1.0))
    } else {
        1.0
    };
    let solve_l1 = l1 * stretch;
    let solve_l2 = l2 * stretch;
    let clamped_distance = distance.clamp(
        (solve_l1 - solve_l2).abs() + 1.0e-5,
        solve_l1 + solve_l2 - 1.0e-5,
    );

    let bend_sign = pole_local
        .map(|pole| {
            let px = pole.x - root_position.x;
            let py = pole.y - root_position.y;
            if dx * py - dy * px >= 0.0 {
                1.0
            } else {
                -1.0
            }
        })
        .unwrap_or_else(|| if mid_local.rotation >= 0.0 { 1.0 } else { -1.0 });

    let target_angle = dy.atan2(dx);
    let root_cos = ((clamped_distance * clamped_distance + solve_l1 * solve_l1
        - solve_l2 * solve_l2)
        / (2.0 * clamped_distance * solve_l1))
        .clamp(-1.0, 1.0);
    let root_offset = root_cos.acos();
    let mid_cos =
        ((clamped_distance * clamped_distance - solve_l1 * solve_l1 - solve_l2 * solve_l2)
            / (2.0 * solve_l1 * solve_l2))
            .clamp(-1.0, 1.0);
    let solved_root = target_angle - bend_sign * root_offset;
    let solved_mid = bend_sign * mid_cos.acos();
    let weight = weight.clamp(0.0, 1.0);

    if let Some(root) = locals.get_mut(&root_id) {
        root.rotation = lerp_angle(root.rotation, solved_root, weight);
        if allow_stretch {
            root.sx *= 1.0 + (stretch - 1.0) * weight;
        }
    }
    if let Some(mid) = locals.get_mut(&mid_id) {
        mid.rotation = lerp_angle(mid.rotation, solved_mid, weight);
        if allow_stretch {
            mid.sx *= 1.0 + (stretch - 1.0) * weight;
        }
    }
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut delta = (b - a) % std::f32::consts::TAU;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    a + delta * t
}

fn build_world_map(rig: &RigAsset, locals: &HashMap<u16, Transform2D>) -> HashMap<u16, Affine> {
    let mut out = HashMap::with_capacity(rig.nodes.len());
    for node in &rig.nodes {
        let _ = world_for_node(rig, locals, node.node_id, &mut out, 0);
    }
    out
}

fn world_for_node(
    rig: &RigAsset,
    locals: &HashMap<u16, Transform2D>,
    node_id: u16,
    cache: &mut HashMap<u16, Affine>,
    depth: usize,
) -> Option<Affine> {
    if let Some(world) = cache.get(&node_id).copied() {
        return Some(world);
    }
    if depth > rig.nodes.len() {
        return None;
    }
    let node = rig.nodes.iter().find(|node| node.node_id == node_id)?;
    let local = Affine::from_transform(*locals.get(&node_id)?);
    let world = match node.parent {
        Some(parent) => Affine::compose(
            world_for_node(rig, locals, parent, cache, depth + 1)?,
            local,
        ),
        None => local,
    };
    cache.insert(node_id, world);
    Some(world)
}

pub fn control_world_position(rig: &RigAsset, pose: &RigPose, control_id: u16) -> Option<Vec2> {
    let control = rig
        .controls
        .iter()
        .find(|control| control.control_id == control_id)?;
    let value = pose.controls.get(&control_id)?;
    match control.kind {
        RigControlKind::Position2D => {
            if control.target_node.is_none() {
                Some(Vec2::new(value.x, value.y))
            } else {
                control
                    .target_node
                    .and_then(|node| pose.node_world.get(&node))
                    .map(|world| world.apply(Vec2::new(0.0, 0.0)))
            }
        }
        RigControlKind::Rotation | RigControlKind::Slider | RigControlKind::Toggle => control
            .target_node
            .and_then(|node| pose.node_world.get(&node))
            .map(|world| world.apply(Vec2::new(0.0, 0.0))),
    }
}

pub fn capture_pose_values(rig: &RigAsset, frame: f32) -> Vec<RigPoseValue> {
    let pose = evaluate_rig(rig, frame, &[]);
    let mut values = Vec::new();

    for control in &rig.controls {
        let Some(value) = pose.controls.get(&control.control_id).copied() else {
            continue;
        };
        match control.kind {
            RigControlKind::Position2D => {
                values.push(RigPoseValue {
                    property: RigPropertyRef::ControlX(control.control_id),
                    value: value.x,
                });
                values.push(RigPoseValue {
                    property: RigPropertyRef::ControlY(control.control_id),
                    value: value.y,
                });
            }
            RigControlKind::Rotation | RigControlKind::Slider | RigControlKind::Toggle => {
                values.push(RigPoseValue {
                    property: RigPropertyRef::ControlValue(control.control_id),
                    value: value.value,
                });
            }
        }
    }

    // Derived properties are omitted when a master driver owns them. Applying a
    // pose then changes the master source only, avoiding contradictory keys.
    for node in &rig.nodes {
        let has_rotation_control = rig.controls.iter().any(|control| {
            control.kind == RigControlKind::Rotation && control.target_node == Some(node.node_id)
        });
        let master_driven = rig
            .drivers
            .iter()
            .any(|driver| driver.target == RigPropertyRef::NodeRotation(node.node_id));
        if let Some(local) = pose.node_local.get(&node.node_id) {
            if !has_rotation_control && !master_driven {
                values.push(RigPoseValue {
                    property: RigPropertyRef::NodeRotation(node.node_id),
                    value: local.rotation,
                });
            }
            values.push(RigPoseValue {
                property: RigPropertyRef::NodeTx(node.node_id),
                value: local.tx,
            });
            values.push(RigPoseValue {
                property: RigPropertyRef::NodeTy(node.node_id),
                value: local.ty,
            });
            values.push(RigPoseValue {
                property: RigPropertyRef::NodeScaleX(node.node_id),
                value: local.sx,
            });
            values.push(RigPoseValue {
                property: RigPropertyRef::NodeScaleY(node.node_id),
                value: local.sy,
            });
        }
    }
    for constraint in &rig.constraints {
        let id = constraint.id();
        let master_driven = rig
            .drivers
            .iter()
            .any(|driver| driver.target == RigPropertyRef::ConstraintWeight(id));
        if !master_driven {
            if let Some(weight) = pose.constraint_weights.get(&id) {
                values.push(RigPoseValue {
                    property: RigPropertyRef::ConstraintWeight(id),
                    value: *weight,
                });
            }
        }
    }

    values.sort_by_key(|value| property_sort_key(value.property));
    values
}

pub fn rest_node_worlds(rig: &RigAsset) -> HashMap<u16, Affine> {
    let locals = rig
        .nodes
        .iter()
        .map(|node| (node.node_id, node.rest))
        .collect::<HashMap<_, _>>();
    build_world_map(rig, &locals)
}

pub fn deformer_for_instance(rig: &RigAsset, instance_id: u32) -> Option<&RigDeformer> {
    rig.deformers
        .iter()
        .find(|deformer| deformer.instance_id() == instance_id)
}

pub fn deform_vector_for_instance(
    rig: &RigAsset,
    pose: &RigPose,
    instance_id: u32,
    vector: &VectorAsset,
) -> Option<VectorAsset> {
    let deformer = rig.deformers.iter().find(|deformer| {
        deformer.instance_id() == instance_id && deformer.asset_id() == vector.asset_id
    })?;
    let mut output = vector.clone();
    match deformer {
        RigDeformer::Skin {
            bind_transform,
            bones,
            anchors,
            ..
        } => {
            let inverse_bind = bind_transform.inverse()?;
            let bone_binds = bones
                .iter()
                .map(|bone| (bone.node_id, bone.inverse_rest_world))
                .collect::<HashMap<_, _>>();
            let weight_map = anchors
                .iter()
                .map(|anchor| {
                    (
                        (anchor.path_index, anchor.anchor_index),
                        anchor.weights.as_slice(),
                    )
                })
                .collect::<HashMap<_, _>>();
            for (path_index, path) in output.paths.iter_mut().enumerate() {
                for (anchor_index, anchor) in path.anchors.iter_mut().enumerate() {
                    let Ok(path_index) = u16::try_from(path_index) else {
                        return None;
                    };
                    let Ok(anchor_index) = u16::try_from(anchor_index) else {
                        return None;
                    };
                    let weights = weight_map.get(&(path_index, anchor_index)).copied()?;
                    let deform = |point: Vec2| {
                        skin_point(
                            point,
                            *bind_transform,
                            inverse_bind,
                            weights,
                            &bone_binds,
                            pose,
                        )
                    };
                    anchor.point = deform(anchor.point)?;
                    anchor.in_handle = anchor.in_handle.and_then(deform);
                    anchor.out_handle = anchor.out_handle.and_then(deform);
                }
            }
        }
        RigDeformer::Bend {
            bind_transform,
            axis_start,
            axis_end,
            start_control,
            middle_control,
            end_control,
            ..
        } => {
            let inverse_bind = bind_transform.inverse()?;
            let p0 = control_position(pose, *start_control)?;
            let p1 = control_position(pose, *middle_control)?;
            let p2 = control_position(pose, *end_control)?;
            let deform = |point: Vec2| {
                bend_point(
                    point,
                    *axis_start,
                    *axis_end,
                    *bind_transform,
                    inverse_bind,
                    [p0, p1, p2],
                )
            };
            for path in &mut output.paths {
                for anchor in &mut path.anchors {
                    anchor.point = deform(anchor.point)?;
                    anchor.in_handle = anchor.in_handle.and_then(deform);
                    anchor.out_handle = anchor.out_handle.and_then(deform);
                }
            }
            // A sparse linear segment has no control points for a bend to act on.
            // Keep the canonical anchors untouched, but synthesize transient cubic
            // handles from deformed 1/4 and 3/4 samples when the segment actually
            // curves. This avoids long chord artifacts without adding anchors.
            for (rest_path, path) in vector.paths.iter().zip(&mut output.paths) {
                let count = rest_path.anchors.len();
                if count < 2 {
                    continue;
                }
                let segment_count = if rest_path.closed { count } else { count - 1 };
                for index in 0..segment_count {
                    let next = (index + 1) % count;
                    if rest_path.anchors[index].out_handle.is_some()
                        || rest_path.anchors[next].in_handle.is_some()
                    {
                        continue;
                    }
                    let a = rest_path.anchors[index].point;
                    let b = rest_path.anchors[next].point;
                    let quarter = Vec2::new(a.x + (b.x - a.x) * 0.25, a.y + (b.y - a.y) * 0.25);
                    let three_quarter =
                        Vec2::new(a.x + (b.x - a.x) * 0.75, a.y + (b.y - a.y) * 0.75);
                    let q1 = deform(quarter)?;
                    let q3 = deform(three_quarter)?;
                    let d0 = path.anchors[index].point;
                    let d3 = path.anchors[next].point;
                    let linear_q1 =
                        Vec2::new(d0.x + (d3.x - d0.x) * 0.25, d0.y + (d3.y - d0.y) * 0.25);
                    let linear_q3 =
                        Vec2::new(d0.x + (d3.x - d0.x) * 0.75, d0.y + (d3.y - d0.y) * 0.75);
                    let curve_error = (q1.x - linear_q1.x).abs()
                        + (q1.y - linear_q1.y).abs()
                        + (q3.x - linear_q3.x).abs()
                        + (q3.y - linear_q3.y).abs();
                    if curve_error <= 1.0e-5 {
                        continue;
                    }
                    let c1 = Vec2::new(
                        -10.0 * d0.x / 9.0 + d3.x / 3.0 + 8.0 * q1.x / 3.0 - 8.0 * q3.x / 9.0,
                        -10.0 * d0.y / 9.0 + d3.y / 3.0 + 8.0 * q1.y / 3.0 - 8.0 * q3.y / 9.0,
                    );
                    let c2 = Vec2::new(
                        d0.x / 3.0 - 10.0 * d3.x / 9.0 - 8.0 * q1.x / 9.0 + 8.0 * q3.x / 3.0,
                        d0.y / 3.0 - 10.0 * d3.y / 9.0 - 8.0 * q1.y / 9.0 + 8.0 * q3.y / 3.0,
                    );
                    if [c1.x, c1.y, c2.x, c2.y].into_iter().all(f32::is_finite) {
                        path.anchors[index].out_handle = Some(c1);
                        path.anchors[next].in_handle = Some(c2);
                    }
                }
            }
        }
        RigDeformer::Cage {
            bind_transform,
            rest_min,
            rest_max,
            controls,
            ..
        } => {
            let inverse_bind = bind_transform.inverse()?;
            let corners = [
                control_position(pose, controls[0])?,
                control_position(pose, controls[1])?,
                control_position(pose, controls[2])?,
                control_position(pose, controls[3])?,
            ];
            for path in &mut output.paths {
                for anchor in &mut path.anchors {
                    let deform = |point: Vec2| {
                        cage_point(point, *rest_min, *rest_max, inverse_bind, corners)
                    };
                    anchor.point = deform(anchor.point)?;
                    anchor.in_handle = anchor.in_handle.and_then(deform);
                    anchor.out_handle = anchor.out_handle.and_then(deform);
                }
            }
        }
    }
    Some(output)
}

fn control_position(pose: &RigPose, control_id: u16) -> Option<Vec2> {
    pose.controls
        .get(&control_id)
        .map(|value| Vec2::new(value.x, value.y))
}

fn skin_point(
    point: Vec2,
    bind_transform: Affine,
    inverse_bind: Affine,
    weights: &[RigSkinWeight],
    bone_binds: &HashMap<u16, Affine>,
    pose: &RigPose,
) -> Option<Vec2> {
    let rest_world = bind_transform.apply(point);
    let mut x = 0.0_f32;
    let mut y = 0.0_f32;
    let mut sum = 0.0_f32;
    for weight in weights {
        let inverse_rest = bone_binds.get(&weight.node_id)?;
        let current_world = pose.node_world.get(&weight.node_id)?;
        let bone_local = inverse_rest.apply(rest_world);
        let deformed = current_world.apply(bone_local);
        x += deformed.x * weight.weight;
        y += deformed.y * weight.weight;
        sum += weight.weight;
    }
    if !x.is_finite() || !y.is_finite() || sum <= 1.0e-6 {
        return None;
    }
    Some(inverse_bind.apply(Vec2::new(x / sum, y / sum)))
}

fn bend_point(
    point: Vec2,
    axis_start: Vec2,
    axis_end: Vec2,
    bind_transform: Affine,
    inverse_bind: Affine,
    curve: [Vec2; 3],
) -> Option<Vec2> {
    let [p0, p1, p2] = curve;
    let dx = axis_end.x - axis_start.x;
    let dy = axis_end.y - axis_start.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= 1.0e-6 {
        return None;
    }
    let ux = dx / length;
    let uy = dy / length;
    let vx = -uy;
    let vy = ux;
    let rel_x = point.x - axis_start.x;
    let rel_y = point.y - axis_start.y;
    let t = ((rel_x * ux + rel_y * uy) / length).clamp(0.0, 1.0);
    let lateral = rel_x * vx + rel_y * vy;
    let omt = 1.0 - t;
    let curve = Vec2::new(
        omt * omt * p0.x + 2.0 * omt * t * p1.x + t * t * p2.x,
        omt * omt * p0.y + 2.0 * omt * t * p1.y + t * t * p2.y,
    );
    let tangent = Vec2::new(
        2.0 * omt * (p1.x - p0.x) + 2.0 * t * (p2.x - p1.x),
        2.0 * omt * (p1.y - p0.y) + 2.0 * t * (p2.y - p1.y),
    );
    let tangent_len = (tangent.x * tangent.x + tangent.y * tangent.y).sqrt();
    if tangent_len <= 1.0e-6 {
        return None;
    }
    let normal = Vec2::new(-tangent.y / tangent_len, tangent.x / tangent_len);
    let rest_origin = bind_transform.apply(axis_start);
    let rest_perp = bind_transform.apply(Vec2::new(axis_start.x + vx, axis_start.y + vy));
    let perp_scale =
        ((rest_perp.x - rest_origin.x).powi(2) + (rest_perp.y - rest_origin.y).powi(2)).sqrt();
    let world = Vec2::new(
        curve.x + normal.x * lateral * perp_scale,
        curve.y + normal.y * lateral * perp_scale,
    );
    if !world.x.is_finite() || !world.y.is_finite() {
        return None;
    }
    Some(inverse_bind.apply(world))
}

fn cage_point(
    point: Vec2,
    rest_min: Vec2,
    rest_max: Vec2,
    inverse_bind: Affine,
    corners: [Vec2; 4],
) -> Option<Vec2> {
    let width = rest_max.x - rest_min.x;
    let height = rest_max.y - rest_min.y;
    if width.abs() <= 1.0e-6 || height.abs() <= 1.0e-6 {
        return None;
    }
    let u = (point.x - rest_min.x) / width;
    let v = (point.y - rest_min.y) / height;
    let top = Vec2::new(
        corners[0].x + (corners[1].x - corners[0].x) * u,
        corners[0].y + (corners[1].y - corners[0].y) * u,
    );
    let bottom = Vec2::new(
        corners[3].x + (corners[2].x - corners[3].x) * u,
        corners[3].y + (corners[2].y - corners[3].y) * u,
    );
    let world = Vec2::new(
        top.x + (bottom.x - top.x) * v,
        top.y + (bottom.y - top.y) * v,
    );
    if !world.x.is_finite() || !world.y.is_finite() {
        return None;
    }
    Some(inverse_bind.apply(world))
}

pub fn normalized_nearest_bone_weights(
    rig: &RigAsset,
    bind_transform: Affine,
    point: Vec2,
    max_influences: usize,
) -> Vec<RigSkinWeight> {
    let worlds = rest_node_worlds(rig);
    let world_point = bind_transform.apply(point);
    let mut candidates = rig
        .nodes
        .iter()
        .filter_map(|node| {
            let world = worlds.get(&node.node_id)?;
            let a = world.apply(Vec2::new(0.0, 0.0));
            let b = world.apply(Vec2::new(node.length.max(0.001), 0.0));
            let distance = point_segment_distance(world_point, a, b).max(0.01);
            Some((node.node_id, 1.0 / (distance * distance)))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
    candidates.truncate(max_influences.clamp(1, 4));
    let sum = candidates.iter().map(|(_, weight)| *weight).sum::<f32>();
    if sum <= 1.0e-9 || !sum.is_finite() {
        return Vec::new();
    }
    candidates
        .into_iter()
        .map(|(node_id, weight)| RigSkinWeight {
            node_id,
            weight: weight / sum,
        })
        .collect()
}

fn point_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= 1.0e-12 {
        return ((point.x - a.x).powi(2) + (point.y - a.y).powi(2)).sqrt();
    }
    let t = (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0);
    let x = a.x + dx * t;
    let y = a.y + dy * t;
    ((point.x - x).powi(2) + (point.y - y).powi(2)).sqrt()
}

pub fn build_skin_anchor_weights(
    rig: &RigAsset,
    vector: &VectorAsset,
    bind_transform: Affine,
    max_influences: usize,
) -> Vec<RigSkinAnchorWeights> {
    vector
        .paths
        .iter()
        .enumerate()
        .flat_map(|(path_index, path)| {
            path.anchors
                .iter()
                .enumerate()
                .filter_map(move |(anchor_index, anchor)| {
                    let path_index = u16::try_from(path_index).ok()?;
                    let anchor_index = u16::try_from(anchor_index).ok()?;
                    let weights = normalized_nearest_bone_weights(
                        rig,
                        bind_transform,
                        anchor.point,
                        max_influences,
                    );
                    (!weights.is_empty()).then_some(RigSkinAnchorWeights {
                        path_index,
                        anchor_index,
                        weights,
                    })
                })
        })
        .collect()
}

pub fn upsert_channel_key(
    rig: &mut RigAsset,
    property: RigPropertyRef,
    frame: u16,
    value: f32,
    easing: Easing,
) {
    if let Some(channel) = rig
        .channels
        .iter_mut()
        .find(|channel| channel.property == property)
    {
        if let Some(key) = channel.keys.iter_mut().find(|key| key.frame == frame) {
            key.value = value;
            key.easing = easing;
        } else {
            channel.keys.push(RigKey {
                frame,
                value,
                easing,
            });
            channel.keys.sort_by_key(|key| key.frame);
        }
        return;
    }
    rig.channels.push(crate::v2::RigChannel {
        property,
        keys: vec![RigKey {
            frame,
            value,
            easing,
        }],
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{
        Anchor, Path, Rgba, RigChannel, RigControl, RigDeformer, RigMirrorPair, RigNode,
        RigPoseBlendMode, RigPoseDriver, RigPosePreset, RigSkinBoneBind, RigVariantChoice,
        RigVariantSet, VectorAsset,
    };

    fn chain() -> RigAsset {
        RigAsset {
            asset_id: 50,
            owner_q0rg_id: 1,
            nodes: vec![
                RigNode {
                    node_id: 1,
                    name: "upper".into(),
                    parent: None,
                    rest: Transform2D::IDENTITY,
                    length: 10.0,
                    binding: None,
                },
                RigNode {
                    node_id: 2,
                    name: "lower".into(),
                    parent: Some(1),
                    rest: Transform2D {
                        tx: 10.0,
                        ..Transform2D::IDENTITY
                    },
                    length: 10.0,
                    binding: None,
                },
                RigNode {
                    node_id: 3,
                    name: "tip".into(),
                    parent: Some(2),
                    rest: Transform2D {
                        tx: 10.0,
                        ..Transform2D::IDENTITY
                    },
                    length: 0.0,
                    binding: None,
                },
            ],
            controls: vec![RigControl {
                control_id: 1,
                name: "hand".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 20.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            }],
            constraints: vec![RigConstraint::TwoBoneIk {
                constraint_id: 1,
                root_node: 1,
                mid_node: 2,
                tip_node: 3,
                target_control: 1,
                pole_control: None,
                weight: 1.0,
                allow_stretch: false,
                max_stretch: 1.0,
            }],
            channels: Vec::new(),
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            pose_drivers: Vec::new(),
            mirror_pairs: Vec::new(),
            variants: Vec::new(),
        }
    }

    #[test]
    fn fk_world_chain_is_exact_at_rest() {
        let mut rig = chain();
        rig.constraints.clear();
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let tip = pose.node_world[&3].apply(Vec2::new(0.0, 0.0));
        assert!((tip.x - 20.0).abs() < 1.0e-5);
        assert!(tip.y.abs() < 1.0e-5);
    }

    #[test]
    fn two_bone_ik_reaches_reachable_target() {
        let rig = chain();
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 12.0,
                y: 8.0,
            }],
        );
        let tip = pose.node_world[&3].apply(Vec2::new(0.0, 0.0));
        assert!((tip.x - 12.0).abs() < 1.0e-3, "tip x={}", tip.x);
        assert!((tip.y - 8.0).abs() < 1.0e-3, "tip y={}", tip.y);
    }

    #[test]
    fn runtime_override_wins_over_timeline_channel() {
        let mut rig = chain();
        rig.channels = vec![RigChannel {
            property: RigPropertyRef::ControlX(1),
            keys: vec![RigKey {
                frame: 0,
                value: 5.0,
                easing: Easing::Linear,
            }],
        }];
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 9.0,
                y: 3.0,
            }],
        );
        assert_eq!(pose.controls[&1].x, 9.0);
        assert_eq!(pose.controls[&1].y, 3.0);
    }

    #[test]
    fn rig_channel_interpolates_with_existing_easing_model() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.channels.push(RigChannel {
            property: RigPropertyRef::NodeRotation(1),
            keys: vec![
                RigKey {
                    frame: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                RigKey {
                    frame: 10,
                    value: 1.0,
                    easing: Easing::Linear,
                },
            ],
        });
        let pose = evaluate_rig(&rig, 5.0, &[]);
        let world = pose.node_world[&1];
        assert!((world.a11 - 0.5f32.cos()).abs() < 1.0e-4);
        assert!((world.a21 - 0.5f32.sin()).abs() < 1.0e-4);
    }

    #[test]
    fn constraint_weight_channel_blends_fk_to_ik() {
        let mut rig = chain();
        rig.channels.push(RigChannel {
            property: RigPropertyRef::ConstraintWeight(1),
            keys: vec![
                RigKey {
                    frame: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                RigKey {
                    frame: 10,
                    value: 1.0,
                    easing: Easing::Linear,
                },
            ],
        });
        let fk = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 12.0,
                y: 8.0,
            }],
        );
        let ik = evaluate_rig(
            &rig,
            10.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 12.0,
                y: 8.0,
            }],
        );
        let fk_tip = fk.node_world[&3].apply(Vec2::new(0.0, 0.0));
        let ik_tip = ik.node_world[&3].apply(Vec2::new(0.0, 0.0));
        assert!((fk_tip.x - 20.0).abs() < 1.0e-4);
        assert!(fk_tip.y.abs() < 1.0e-4);
        assert!((ik_tip.x - 12.0).abs() < 1.0e-3);
        assert!((ik_tip.y - 8.0).abs() < 1.0e-3);
        assert_eq!(fk.constraint_weights[&1], 0.0);
        assert_eq!(ik.constraint_weights[&1], 1.0);
    }

    #[test]
    fn master_slider_drives_multiple_properties_deterministically() {
        let mut rig = chain();
        rig.controls.push(crate::v2::RigControl {
            control_id: 9,
            name: "master".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.5,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.drivers = vec![
            crate::v2::RigDriver {
                driver_id: 1,
                source_control: 9,
                source_min: 0.0,
                source_max: 1.0,
                target: RigPropertyRef::NodeRotation(3),
                target_min: -1.0,
                target_max: 1.0,
            },
            crate::v2::RigDriver {
                driver_id: 2,
                source_control: 9,
                source_min: 0.0,
                source_max: 1.0,
                target: RigPropertyRef::ConstraintWeight(1),
                target_min: 0.0,
                target_max: 1.0,
            },
        ];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        assert!(pose.node_local[&3].rotation.abs() < 1.0e-5);
        assert!((pose.constraint_weights[&1] - 0.5).abs() < 1.0e-5);

        let full = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Value {
                control_id: 9,
                value: 1.0,
            }],
        );
        assert!((full.node_local[&3].rotation - 1.0).abs() < 1.0e-5);
        assert!((full.constraint_weights[&1] - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn pose_capture_keeps_controls_and_omits_master_derived_targets() {
        let mut rig = chain();
        rig.controls.push(crate::v2::RigControl {
            control_id: 9,
            name: "master".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.25,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.drivers.push(crate::v2::RigDriver {
            driver_id: 1,
            source_control: 9,
            source_min: 0.0,
            source_max: 1.0,
            target: RigPropertyRef::ConstraintWeight(1),
            target_min: 0.0,
            target_max: 1.0,
        });
        let values = capture_pose_values(&rig, 0.0);
        assert!(values.iter().any(|value| {
            value.property == RigPropertyRef::ControlValue(9) && (value.value - 0.25).abs() < 1.0e-5
        }));
        assert!(!values
            .iter()
            .any(|value| value.property == RigPropertyRef::ConstraintWeight(1)));
    }

    #[test]
    fn node_transform_channels_drive_translation_and_scale() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.channels = vec![
            RigChannel {
                property: RigPropertyRef::NodeTx(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 7.0,
                    easing: Easing::Linear,
                }],
            },
            RigChannel {
                property: RigPropertyRef::NodeTy(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: -3.0,
                    easing: Easing::Linear,
                }],
            },
            RigChannel {
                property: RigPropertyRef::NodeScaleX(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 1.5,
                    easing: Easing::Linear,
                }],
            },
            RigChannel {
                property: RigPropertyRef::NodeScaleY(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 0.75,
                    easing: Easing::Linear,
                }],
            },
        ];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let local = pose.node_local[&1];
        assert!((local.tx - 7.0).abs() < 1.0e-6);
        assert!((local.ty + 3.0).abs() < 1.0e-6);
        assert!((local.sx - 1.5).abs() < 1.0e-6);
        assert!((local.sy - 0.75).abs() < 1.0e-6);
    }

    #[test]
    fn position_limit_clamps_local_translation() {
        let mut rig = chain();
        rig.constraints = vec![RigConstraint::PositionLimit {
            constraint_id: 2,
            node_id: 1,
            min_x: -2.0,
            max_x: 2.0,
            min_y: -4.0,
            max_y: 4.0,
        }];
        rig.channels = vec![
            RigChannel {
                property: RigPropertyRef::NodeTx(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: 50.0,
                    easing: Easing::Linear,
                }],
            },
            RigChannel {
                property: RigPropertyRef::NodeTy(1),
                keys: vec![RigKey {
                    frame: 0,
                    value: -50.0,
                    easing: Easing::Linear,
                }],
            },
        ];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        assert!((pose.node_local[&1].tx - 2.0).abs() < 1.0e-6);
        assert!((pose.node_local[&1].ty + 4.0).abs() < 1.0e-6);
    }

    #[test]
    fn aim_constraint_faces_target_control_without_temporal_state() {
        let mut rig = chain();
        rig.constraints = vec![RigConstraint::Aim {
            constraint_id: 2,
            node_id: 1,
            target_control: 1,
            angle_offset: 0.0,
            weight: 1.0,
        }];
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 10.0,
                y: 10.0,
            }],
        );
        assert!((pose.node_local[&1].rotation - std::f32::consts::FRAC_PI_4).abs() < 1.0e-5);
        let second = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 10.0,
                y: 10.0,
            }],
        );
        assert_eq!(
            pose.node_local, second.node_local,
            "aim must be stateless/deterministic"
        );
    }

    #[test]
    fn transform_constraint_matches_target_node_in_parent_local_space() {
        let mut rig = chain();
        rig.constraints = vec![RigConstraint::Transform {
            constraint_id: 2,
            node_id: 3,
            target_node: 1,
            position_weight: 1.0,
            rotation_weight: 1.0,
        }];
        rig.channels = vec![RigChannel {
            property: RigPropertyRef::NodeRotation(1),
            keys: vec![RigKey {
                frame: 0,
                value: 0.4,
                easing: Easing::Linear,
            }],
        }];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let source = pose.node_world[&1].apply(Vec2::new(0.0, 0.0));
        let driven = pose.node_world[&3].apply(Vec2::new(0.0, 0.0));
        assert!((source.x - driven.x).abs() < 1.0e-4);
        assert!((source.y - driven.y).abs() < 1.0e-4);
        let source_angle = pose.node_world[&1].a21.atan2(pose.node_world[&1].a11);
        let driven_angle = pose.node_world[&3].a21.atan2(pose.node_world[&3].a11);
        assert!((source_angle - driven_angle).abs() < 1.0e-4);
    }

    #[test]
    fn distance_constraint_keeps_node_inside_authored_band() {
        let mut rig = chain();
        rig.constraints = vec![RigConstraint::Distance {
            constraint_id: 2,
            node_id: 1,
            target_control: 1,
            min_distance: 5.0,
            max_distance: 5.0,
            weight: 1.0,
        }];
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 1,
                x: 20.0,
                y: 0.0,
            }],
        );
        let origin = pose.node_world[&1].apply(Vec2::new(0.0, 0.0));
        assert!((origin.x - 15.0).abs() < 1.0e-4, "origin={origin:?}");
        assert!(origin.y.abs() < 1.0e-4);
    }

    #[test]
    fn toggle_control_can_drive_node_property() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls.push(RigControl {
            control_id: 9,
            name: "flip".into(),
            kind: RigControlKind::Toggle,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 1.0,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.drivers.push(crate::v2::RigDriver {
            driver_id: 9,
            source_control: 9,
            source_min: 0.0,
            source_max: 1.0,
            target: RigPropertyRef::NodeTy(1),
            target_min: 0.0,
            target_max: 12.0,
        });
        let pose = evaluate_rig(&rig, 0.0, &[]);
        assert!((pose.node_local[&1].ty - 12.0).abs() < 1.0e-6);
    }

    fn deformer_test_vector() -> VectorAsset {
        VectorAsset {
            asset_id: 90,
            paths: vec![
                Path {
                    closed: true,
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(0.0, -2.0),
                            in_handle: Some(Vec2::new(-1.0, -2.0)),
                            out_handle: Some(Vec2::new(1.0, -2.0)),
                        },
                        Anchor {
                            point: Vec2::new(20.0, -2.0),
                            in_handle: Some(Vec2::new(19.0, -2.0)),
                            out_handle: Some(Vec2::new(21.0, -2.0)),
                        },
                        Anchor {
                            point: Vec2::new(20.0, 2.0),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(0.0, 2.0),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                },
                Path {
                    closed: true,
                    anchors: vec![
                        Anchor {
                            point: Vec2::new(8.0, -0.5),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(12.0, -0.5),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(12.0, 0.5),
                            in_handle: None,
                            out_handle: None,
                        },
                        Anchor {
                            point: Vec2::new(8.0, 0.5),
                            in_handle: None,
                            out_handle: None,
                        },
                    ],
                },
            ],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        }
    }

    fn skin_for_vector(rig: &RigAsset, vector: &VectorAsset) -> RigDeformer {
        let worlds = rest_node_worlds(rig);
        RigDeformer::Skin {
            deformer_id: 1,
            instance_id: 77,
            asset_id: vector.asset_id,
            bind_transform: Affine::IDENTITY,
            bones: rig
                .nodes
                .iter()
                .map(|node| RigSkinBoneBind {
                    node_id: node.node_id,
                    inverse_rest_world: worlds[&node.node_id].inverse().unwrap(),
                })
                .collect(),
            anchors: build_skin_anchor_weights(rig, vector, Affine::IDENTITY, 2),
        }
    }

    #[test]
    fn skin_rest_pose_reproduces_vector_exactly_and_keeps_hole_topology() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls.clear();
        let vector = deformer_test_vector();
        rig.deformers = vec![skin_for_vector(&rig, &vector)];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let deformed = deform_vector_for_instance(&rig, &pose, 77, &vector).unwrap();
        assert_eq!(deformed, vector);
        assert_eq!(deformed.paths.len(), 2);
        assert!(deformed.paths.iter().all(|path| path.closed));
        assert_eq!(
            deformed.paths[0].anchors[0].in_handle,
            vector.paths[0].anchors[0].in_handle
        );
        assert_eq!(
            deformed.paths[0].anchors[0].out_handle,
            vector.paths[0].anchors[0].out_handle
        );
    }

    #[test]
    fn skin_bends_points_and_bezier_handles_without_changing_anchor_counts() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls.clear();
        let vector = deformer_test_vector();
        rig.deformers = vec![skin_for_vector(&rig, &vector)];
        rig.channels.push(RigChannel {
            property: RigPropertyRef::NodeRotation(2),
            keys: vec![RigKey {
                frame: 0,
                value: std::f32::consts::FRAC_PI_2,
                easing: Easing::Linear,
            }],
        });
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let deformed = deform_vector_for_instance(&rig, &pose, 77, &vector).unwrap();
        assert_eq!(deformed.paths.len(), vector.paths.len());
        for (before, after) in vector.paths.iter().zip(&deformed.paths) {
            assert_eq!(after.closed, before.closed);
            assert_eq!(after.anchors.len(), before.anchors.len());
        }
        let moved = deformed.paths[0].anchors[1].point;
        assert!((moved.y - vector.paths[0].anchors[1].point.y).abs() > 0.1);
        let handle = deformed.paths[0].anchors[1].in_handle.unwrap();
        assert!(handle.x.is_finite() && handle.y.is_finite());
        assert_ne!(handle, vector.paths[0].anchors[1].in_handle.unwrap());
    }

    #[test]
    fn auto_skin_weights_are_positive_bounded_and_normalized() {
        let mut rig = chain();
        rig.constraints.clear();
        let vector = deformer_test_vector();
        let anchors = build_skin_anchor_weights(&rig, &vector, Affine::IDENTITY, 2);
        assert_eq!(
            anchors.len(),
            vector
                .paths
                .iter()
                .map(|path| path.anchors.len())
                .sum::<usize>()
        );
        for anchor in anchors {
            assert!((1..=2).contains(&anchor.weights.len()));
            assert!(anchor
                .weights
                .iter()
                .all(|weight| weight.weight > 0.0 && weight.weight.is_finite()));
            let sum = anchor
                .weights
                .iter()
                .map(|weight| weight.weight)
                .sum::<f32>();
            assert!((sum - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn bend_deformer_preserves_topology_and_moves_middle_without_moving_end_controls() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls = vec![
            RigControl {
                control_id: 10,
                name: "start".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 0.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
            RigControl {
                control_id: 11,
                name: "bend".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 10.0,
                rest_y: 8.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
            RigControl {
                control_id: 12,
                name: "end".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 20.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
        ];
        let vector = deformer_test_vector();
        rig.deformers = vec![RigDeformer::Bend {
            deformer_id: 1,
            instance_id: 77,
            asset_id: vector.asset_id,
            bind_transform: Affine::IDENTITY,
            axis_start: Vec2::new(0.0, 0.0),
            axis_end: Vec2::new(20.0, 0.0),
            start_control: 10,
            middle_control: 11,
            end_control: 12,
        }];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        let deformed = deform_vector_for_instance(&rig, &pose, 77, &vector).unwrap();
        assert_eq!(
            deformed
                .paths
                .iter()
                .map(|p| p.anchors.len())
                .collect::<Vec<_>>(),
            vector
                .paths
                .iter()
                .map(|p| p.anchors.len())
                .collect::<Vec<_>>()
        );
        assert!(deformed.paths.iter().all(|path| path.closed));
        assert!(deformed.paths[1]
            .anchors
            .iter()
            .any(|anchor| anchor.point.y.abs() > 1.0));
    }

    #[test]
    fn cage_rest_quad_is_exact_and_corner_move_is_non_destructive() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls = vec![
            RigControl {
                control_id: 20,
                name: "tl".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 0.0,
                rest_y: -2.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
            RigControl {
                control_id: 21,
                name: "tr".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 20.0,
                rest_y: -2.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
            RigControl {
                control_id: 22,
                name: "br".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 20.0,
                rest_y: 2.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
            RigControl {
                control_id: 23,
                name: "bl".into(),
                kind: RigControlKind::Position2D,
                target_node: None,
                rest_x: 0.0,
                rest_y: 2.0,
                rest_value: 0.0,
                min_value: -1000.0,
                max_value: 1000.0,
                public_in_simple: true,
            },
        ];
        let vector = deformer_test_vector();
        rig.deformers = vec![RigDeformer::Cage {
            deformer_id: 1,
            instance_id: 77,
            asset_id: vector.asset_id,
            bind_transform: Affine::IDENTITY,
            rest_min: Vec2::new(0.0, -2.0),
            rest_max: Vec2::new(20.0, 2.0),
            controls: [20, 21, 22, 23],
        }];
        let rest = evaluate_rig(&rig, 0.0, &[]);
        assert_eq!(
            deform_vector_for_instance(&rig, &rest, 77, &vector).unwrap(),
            vector
        );
        let moved = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Position {
                control_id: 21,
                x: 24.0,
                y: -8.0,
            }],
        );
        let deformed = deform_vector_for_instance(&rig, &moved, 77, &vector).unwrap();
        assert_eq!(deformed.paths.len(), vector.paths.len());
        assert_eq!(
            deformed.paths[0].anchors.len(),
            vector.paths[0].anchors.len()
        );
        assert!(deformed.paths[0].anchors[1].point.y < -2.0);
        assert!(deformed.paths[0].anchors[1].out_handle.is_some());
    }

    #[test]
    fn pose_driver_runs_after_runtime_source_override_and_before_direct_control_mapping() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.controls.push(RigControl {
            control_id: 9,
            name: "pose master".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: 0.0,
            max_value: 1.0,
            public_in_simple: true,
        });
        rig.controls.push(RigControl {
            control_id: 10,
            name: "root dial".into(),
            kind: RigControlKind::Rotation,
            target_node: Some(1),
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: -3.2,
            max_value: 3.2,
            public_in_simple: true,
        });
        rig.poses.push(RigPosePreset {
            pose_id: 1,
            name: "turned".into(),
            values: vec![RigPoseValue {
                property: RigPropertyRef::ControlValue(10),
                value: 1.0,
            }],
        });
        rig.pose_drivers.push(RigPoseDriver {
            driver_id: 1,
            source_control: 9,
            pose_id: 1,
            source_min: 0.0,
            source_max: 1.0,
            weight_min: 0.0,
            weight_max: 1.0,
            mode: RigPoseBlendMode::Override,
        });
        let rest = evaluate_rig(&rig, 0.0, &[]);
        assert!(rest.node_local[&1].rotation.abs() < 1.0e-6);
        let driven = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Value {
                control_id: 9,
                value: 0.5,
            }],
        );
        assert!((driven.controls[&10].value - 0.5).abs() < 1.0e-6);
        assert!((driven.node_local[&1].rotation - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn multiple_pose_drivers_are_deterministic_by_driver_id_and_additive_uses_rest_delta() {
        let mut rig = chain();
        rig.constraints.clear();
        for (id, value) in [(8, 1.0), (9, 1.0)] {
            rig.controls.push(RigControl {
                control_id: id,
                name: format!("master{id}"),
                kind: RigControlKind::Slider,
                target_node: None,
                rest_x: 0.0,
                rest_y: 0.0,
                rest_value: value,
                min_value: 0.0,
                max_value: 1.0,
                public_in_simple: true,
            });
        }
        rig.poses = vec![
            RigPosePreset {
                pose_id: 1,
                name: "override".into(),
                values: vec![RigPoseValue {
                    property: RigPropertyRef::NodeTy(1),
                    value: 10.0,
                }],
            },
            RigPosePreset {
                pose_id: 2,
                name: "add".into(),
                values: vec![RigPoseValue {
                    property: RigPropertyRef::NodeTy(1),
                    value: 4.0,
                }],
            },
        ];
        // Store them backwards to prove declaration order is irrelevant.
        rig.pose_drivers = vec![
            RigPoseDriver {
                driver_id: 20,
                source_control: 9,
                pose_id: 2,
                source_min: 0.0,
                source_max: 1.0,
                weight_min: 0.0,
                weight_max: 0.5,
                mode: RigPoseBlendMode::Additive,
            },
            RigPoseDriver {
                driver_id: 10,
                source_control: 8,
                pose_id: 1,
                source_min: 0.0,
                source_max: 1.0,
                weight_min: 0.0,
                weight_max: 0.5,
                mode: RigPoseBlendMode::Override,
            },
        ];
        let pose = evaluate_rig(&rig, 0.0, &[]);
        // rest ty=0. override 50% -> 5, then additive (4-0)*50% -> 7.
        assert!((pose.node_local[&1].ty - 7.0).abs() < 1.0e-6);
    }

    #[test]
    fn mirror_pose_uses_explicit_pair_mapping_in_both_directions() {
        let mut rig = chain();
        rig.poses.push(RigPosePreset {
            pose_id: 1,
            name: "left".into(),
            values: vec![
                RigPoseValue {
                    property: RigPropertyRef::NodeRotation(1),
                    value: 0.4,
                },
                RigPoseValue {
                    property: RigPropertyRef::NodeRotation(2),
                    value: -0.2,
                },
                RigPoseValue {
                    property: RigPropertyRef::NodeTy(3),
                    value: 6.0,
                },
            ],
        });
        rig.mirror_pairs.push(RigMirrorPair {
            left: RigPropertyRef::NodeRotation(1),
            right: RigPropertyRef::NodeRotation(2),
            multiplier: -1.0,
            offset: 0.0,
        });
        let mirrored = mirror_pose_values(&rig, 1).unwrap();
        assert!(mirrored.iter().any(
            |v| v.property == RigPropertyRef::NodeRotation(2) && (v.value + 0.4).abs() < 1.0e-6
        ));
        assert!(mirrored.iter().any(
            |v| v.property == RigPropertyRef::NodeRotation(1) && (v.value - 0.2).abs() < 1.0e-6
        ));
        assert!(mirrored
            .iter()
            .any(|v| v.property == RigPropertyRef::NodeTy(3) && (v.value - 6.0).abs() < 1.0e-6));
    }

    #[test]
    fn variant_target_uses_scalar_control_rounding_and_clamping() {
        let mut rig = chain();
        rig.controls.push(RigControl {
            control_id: 9,
            name: "mouth".into(),
            kind: RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: 0.0,
            max_value: 2.0,
            public_in_simple: true,
        });
        rig.variants.push(RigVariantSet {
            variant_id: 1,
            name: "mouths".into(),
            instance_id: 77,
            source_control: 9,
            choices: vec![
                RigVariantChoice {
                    name: "rest".into(),
                    target: Target::Asset(1),
                },
                RigVariantChoice {
                    name: "a".into(),
                    target: Target::Asset(2),
                },
                RigVariantChoice {
                    name: "o".into(),
                    target: Target::Q0rg(3),
                },
            ],
        });
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Value {
                control_id: 9,
                value: 1.6,
            }],
        );
        assert_eq!(
            resolved_variant_target(&rig, &pose, 77, Target::Asset(99)),
            Target::Q0rg(3)
        );
        let low = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Value {
                control_id: 9,
                value: -50.0,
            }],
        );
        assert_eq!(
            resolved_variant_target(&rig, &low, 77, Target::Asset(99)),
            Target::Asset(1)
        );
    }

    #[test]
    fn explicit_runtime_pose_blends_without_mutating_authored_rig() {
        let mut rig = chain();
        rig.constraints.clear();
        rig.poses.push(RigPosePreset {
            pose_id: 40,
            name: "runtime pose".into(),
            values: vec![RigPoseValue {
                property: RigPropertyRef::NodeTy(1),
                value: 12.0,
            }],
        });
        let authored = rig.clone();
        let pose = evaluate_rig(
            &rig,
            0.0,
            &[RigControlOverride::Pose {
                pose_id: 40,
                weight: 0.25,
            }],
        );
        assert!((pose.node_local[&1].ty - 3.0).abs() < 1.0e-6);
        assert_eq!(rig, authored, "runtime pose must stay ephemeral");
    }
}
