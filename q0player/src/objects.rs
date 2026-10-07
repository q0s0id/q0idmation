//! Per-placement q0rg runtimes. Authored symbols remain shared and immutable.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use q0s_format::q0lang::runtime::{
    ExecutionReport, Runtime, RuntimeAction, RuntimeDiagnostic, RuntimeValue, TimelineTarget,
};
use q0s_format::raster::{
    active_placement_states_at, runtime_instance_metrics_with_cache, RuntimeBoundsCache,
};
use q0s_format::rig::RigControlOverride;
use q0s_format::runtime_scene::{PlaybackRequest, RuntimeSceneState};
use q0s_format::transform::Affine;
use q0s_format::v2::{InstanceKey, ProjectV2, Target, Vec2, MAX_Q0RG_NESTING_DEPTH};

use crate::{Inner, Player, MAX_FRAME_SCRIPT_TRANSITIONS};

const MAX_ACTIVE_OBJECTS: usize = 4096;
pub(crate) const OBJECT_INSTRUCTION_BUDGET: usize = 100_000;
pub(crate) const MAX_RUNTIME_DIAGNOSTICS: usize = 256;

#[derive(Default)]
pub(crate) struct ObjectWorld {
    instances: HashMap<InstanceKey, ObjectInstance>,
}

struct ObjectInstance {
    symbol: u16,
    name: String,
    frame: u16,
    playing: bool,
    runtime: Option<Runtime>,
    children: ObjectWorld,
    failed: bool,
    hovered: bool,
    captured: bool,
    click_armed: bool,
}

impl ObjectWorld {
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn runtime_at(&self, path: &[InstanceKey]) -> Option<&Runtime> {
        let (first, rest) = path.split_first()?;
        let object = self.instances.get(first)?;
        if rest.is_empty() {
            object.runtime.as_ref()
        } else {
            object.children.runtime_at(rest)
        }
    }
    pub(crate) fn has_active_scripts(&self) -> bool {
        self.instances
            .values()
            .any(|object| object.runtime.is_some() || object.children.has_active_scripts())
    }

    pub(crate) fn playback(&self, key: InstanceKey) -> Option<(u16, bool)> {
        self.instances
            .get(&key)
            .filter(|object| object.runtime.is_some())
            .map(|object| (object.frame, object.playing))
    }

    fn sync_playback_globals(&self, project: &ProjectV2, owner: u16, runtime: &mut Runtime) {
        for (key, name) in &project.runtime.instance_names {
            if key.q0rg_id != owner {
                continue;
            }
            if let Some((frame, playing)) = self.playback(*key) {
                runtime.vars.insert(
                    format!("{name}.frame"),
                    RuntimeValue::Number(f64::from(frame)),
                );
                runtime
                    .vars
                    .insert(format!("{name}.playing"), RuntimeValue::Bool(playing));
            }
        }
    }

    fn collect_playback_writes(
        &self,
        project: &ProjectV2,
        owner: u16,
        runtime: &Runtime,
        scene: &mut RuntimeSceneState,
    ) {
        for (key, name) in &project.runtime.instance_names {
            if key.q0rg_id != owner {
                continue;
            }
            let Some((frame, playing)) = self.playback(*key) else {
                continue;
            };
            let next_frame = number(runtime, &format!("{name}.frame"))
                .filter(|value| {
                    *value >= 0.0 && *value <= f64::from(u16::MAX) && value.fract() == 0.0
                })
                .map(|value| value as u16)
                .filter(|value| *value != frame);
            let next_playing =
                boolean(runtime, &format!("{name}.playing")).filter(|value| *value != playing);
            scene.request_playback(
                *key,
                PlaybackRequest {
                    frame: next_frame,
                    playing: next_playing,
                },
            );
        }
    }
}

struct ObjectPass<'a> {
    project: &'a ProjectV2,
    bounds: &'a RuntimeBoundsCache,
    globals: &'a BTreeMap<String, RuntimeValue>,
    instructions: usize,
    transitions: usize,
    remaining_objects: usize,
    diagnostics: Vec<RuntimeDiagnostic>,
    scene_actions: Vec<RuntimeAction>,
    advance: bool,
    event: Option<(&'a str, &'a [RuntimeValue])>,
}

impl ObjectPass<'_> {
    fn diagnostic(&mut self, name: &str, message: impl Into<String>) {
        if self.diagnostics.len() < MAX_RUNTIME_DIAGNOSTICS {
            self.diagnostics.push(RuntimeDiagnostic {
                line: 0,
                message: format!("q0rg `{name}`: {}", message.into()),
            });
        }
    }

    fn visit(
        &mut self,
        world: &mut ObjectWorld,
        scene: &mut RuntimeSceneState,
        owner: u16,
        frame: u16,
        parent: Affine,
        depth: u8,
    ) {
        if depth >= MAX_Q0RG_NESTING_DEPTH {
            return;
        }
        let Some(q0rg) = self.project.q0rgs.iter().find(|q| q.q0rg_id == owner) else {
            return;
        };
        let rig_pose = q0s_format::rig::rig_for_q0rg(self.project, owner).map(|rig| {
            q0s_format::rig::evaluate_rig(
                rig,
                f32::from(frame),
                scene
                    .rig_overrides
                    .get(&owner)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        });
        let mut active = Vec::new();
        for layer in &q0rg.layers {
            if !self.project.layer_is_visible(owner, layer.layer_id) {
                continue;
            }
            for state in active_placement_states_at(layer, frame) {
                let placement = &layer.placements[state.index];
                let target = match (
                    q0s_format::rig::rig_for_q0rg(self.project, owner),
                    rig_pose.as_ref(),
                ) {
                    (Some(rig), Some(pose)) => q0s_format::rig::resolved_variant_target(
                        rig,
                        pose,
                        placement.instance_id,
                        placement.target,
                    ),
                    _ => placement.target,
                };
                if let Target::Q0rg(symbol) = target {
                    let authored = rig_pose
                        .as_ref()
                        .and_then(|pose| pose.binding_transform(placement.instance_id))
                        .unwrap_or_else(|| Affine::from_transform(state.transform));
                    active.push((
                        InstanceKey::new(owner, placement.instance_id),
                        symbol,
                        authored,
                    ));
                }
            }
        }
        let active_keys: HashSet<_> = active.iter().map(|(key, _, _)| *key).collect();
        world.instances.retain(|key, _| {
            if active_keys.contains(key) {
                true
            } else {
                scene.clear_instance(*key);
                false
            }
        });
        for (key, symbol, authored) in active {
            if self.remaining_objects == 0 {
                self.diagnostic("scene", "active q0rg limit exceeded (4096)");
                break;
            }
            self.remaining_objects -= 1;
            let Some(definition) = self.project.q0rgs.iter().find(|q| q.q0rg_id == symbol) else {
                continue;
            };
            let scripted = !definition.script.trim().is_empty()
                || self
                    .project
                    .runtime
                    .frame_scripts
                    .iter()
                    .any(|script| script.q0rg_id == symbol && !script.source.trim().is_empty());
            let fresh = !world
                .instances
                .get(&key)
                .is_some_and(|object| object.symbol == symbol);
            if fresh {
                if world.instances.contains_key(&key) {
                    scene.clear_instance(key);
                }
                world.instances.insert(
                    key,
                    ObjectInstance {
                        symbol,
                        name: self
                            .project
                            .runtime
                            .instance_names
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| format!("{}#{}", definition.name, key.instance_id)),
                        frame: 0,
                        playing: true,
                        runtime: scripted.then(Runtime::new),
                        children: ObjectWorld::default(),
                        failed: false,
                        hovered: false,
                        captured: false,
                        click_armed: false,
                    },
                );
            }
            let object = world
                .instances
                .get_mut(&key)
                .expect("active object inserted");
            let request = scene.take_playback_request(key);
            let old_frame = object.frame;
            if let Some(playing) = request.playing {
                object.playing = playing;
            }
            if let Some(frame) = request.frame {
                object.frame = frame.min(definition.frame_count.saturating_sub(1));
            }
            if !scripted {
                object.frame = frame % definition.frame_count.max(1);
            } else if self.advance && !fresh && object.playing && request.frame.is_none() {
                object.frame = (object.frame + 1) % definition.frame_count.max(1);
            }
            let mut local = scene.effective_affine(key, authored);
            let world_transform = Affine::compose(parent, local);
            let metrics = runtime_instance_metrics_with_cache(
                self.project,
                owner,
                frame,
                key.instance_id,
                scene,
                self.bounds,
            );
            let pointer = match (self.globals.get("mouse.x"), self.globals.get("mouse.y")) {
                (Some(RuntimeValue::Number(x)), Some(RuntimeValue::Number(y))) => {
                    Some(Vec2::new(*x as f32, *y as f32))
                }
                _ => None,
            };
            let parent_pointer =
                pointer.and_then(|point| parent.inverse().map(|inverse| inverse.apply(point)));
            let hovered = matches!(
                self.globals.get("mouse.inside"),
                Some(RuntimeValue::Bool(true))
            ) && metrics.zip(parent_pointer).is_some_and(|(bounds, point)| {
                point.x >= bounds.left
                    && point.x <= bounds.right
                    && point.y >= bounds.top
                    && point.y <= bounds.bottom
            });
            let local_pointer = pointer.and_then(|point| {
                world_transform
                    .inverse()
                    .map(|inverse| inverse.apply(point))
            });
            let mut child = scene.take_child(key);
            let playing_before = object.playing;
            if let Some(mut runtime) = object.runtime.take() {
                sync_globals(
                    &mut runtime,
                    self.globals,
                    self.project,
                    symbol,
                    object,
                    key,
                    local,
                    &child.scene,
                    metrics,
                    self.bounds,
                    hovered,
                    local_pointer,
                );
                let mut jump = None;
                if fresh {
                    let report =
                        runtime.execute_source_with_budget(&definition.script, self.instructions);
                    jump = self.apply_report(object, &mut child.scene, report);
                }
                if let Some((event, _args)) = self.event {
                    if !object.failed {
                        let event_args = [RuntimeValue::String(object.name.clone())];
                        if event == "mouse.move" && hovered != object.hovered {
                            let report = runtime.dispatch_event(
                                if hovered { "mouse.over" } else { "mouse.out" },
                                &event_args,
                                self.instructions,
                            );
                            jump = self.apply_report(object, &mut child.scene, report).or(jump);
                        }
                        if event == "mouse.down" {
                            object.captured = hovered;
                        }
                        let deliver = match event {
                            "mouse.click" => hovered && object.click_armed,
                            "mouse.double" | "mouse.wheel" => hovered,
                            "mouse.down" => hovered,
                            "mouse.up" => hovered || object.captured,
                            "mouse.over" => hovered && !object.hovered,
                            "mouse.out" => !hovered && object.hovered,
                            _ => true,
                        };
                        if deliver {
                            let report =
                                runtime.dispatch_event(event, &event_args, self.instructions);
                            jump = self.apply_report(object, &mut child.scene, report).or(jump);
                        }
                        if event == "mouse.up" {
                            object.click_armed = hovered && object.captured;
                            object.captured = false;
                        }
                        if event == "mouse.click" {
                            object.click_armed = false;
                        }
                    }
                }
                let interactive = runtime
                    .imported_libraries
                    .iter()
                    .any(|name| matches!(name.as_str(), "q0.object" | "q0.input" | "q0.mouse"));
                let should_run_frame = fresh
                    || request.frame.is_some()
                    || object.frame != old_frame
                    || (self.advance && interactive);
                if should_run_frame || jump.is_some() {
                    let mut enter = true;
                    while enter && !object.failed {
                        if let Some(frame) = jump.take() {
                            object.frame = frame;
                        }
                        runtime.vars.insert(
                            "self.frame".into(),
                            RuntimeValue::Number(f64::from(object.frame)),
                        );
                        runtime.vars.insert(
                            "frame".into(),
                            RuntimeValue::Number(f64::from(object.frame)),
                        );
                        let mut scripts: Vec<_> = self
                            .project
                            .runtime
                            .frame_scripts
                            .iter()
                            .enumerate()
                            .filter(|(_, script)| {
                                script.q0rg_id == symbol && script.frame == object.frame
                            })
                            .map(|(index, script)| (script.layer_id, index, script.source.as_str()))
                            .collect();
                        scripts.sort_by_key(|(layer, index, _)| (*layer, *index));
                        for (_, _, source) in scripts {
                            let report =
                                runtime.execute_source_with_budget(source, self.instructions);
                            jump = self.apply_report(object, &mut child.scene, report);
                            if jump.is_some() || object.failed {
                                break;
                            }
                        }
                        enter = jump.is_some();
                    }
                }
                if let (Some(x), Some(y)) = (number(&runtime, "self.x"), number(&runtime, "self.y"))
                {
                    if scene.set_position(key, x as f32, y as f32) {
                        local.tx = x as f32;
                        local.ty = y as f32;
                    }
                }
                if let Some(playing) = boolean(&runtime, "self.playing") {
                    // Explicit gorun!/gostop! takes precedence over the injected
                    // value, unless the script actually changed that property.
                    if playing != playing_before {
                        object.playing = playing;
                    }
                }
                write_named_positions(&runtime, self.project, symbol, &mut child.scene);
                object.children.collect_playback_writes(
                    self.project,
                    symbol,
                    &runtime,
                    &mut child.scene,
                );
                object.runtime = Some(runtime);
            }
            object.hovered = hovered;
            self.visit(
                &mut object.children,
                &mut child.scene,
                symbol,
                object.frame,
                Affine::compose(parent, local),
                depth + 1,
            );
            child.frame = object.frame;
            scene.set_child(key, child);
        }
    }

    fn apply_report(
        &mut self,
        object: &mut ObjectInstance,
        scene: &mut RuntimeSceneState,
        report: ExecutionReport,
    ) -> Option<u16> {
        self.instructions = self
            .instructions
            .saturating_sub(report.instructions_executed);
        for diagnostic in report.diagnostics {
            if diagnostic.message.contains("budget exceeded")
                || diagnostic.message.contains("limit exceeded")
            {
                object.failed = true;
                object.playing = false;
            }
            if self.diagnostics.len() < MAX_RUNTIME_DIAGNOSTICS {
                self.diagnostics.push(RuntimeDiagnostic {
                    line: diagnostic.line,
                    message: format!("q0rg `{}`: {}", object.name, diagnostic.message),
                });
            }
        }
        let mut jump = None;
        for action in report.actions {
            match action {
                ref timeline_action @ (RuntimeAction::GoRun(ref target)
                | RuntimeAction::GoStop(ref target)) => {
                    let playing = matches!(timeline_action, RuntimeAction::GoRun(_));
                    let frame = match &target {
                        TimelineTarget::Frame(frame) => Some(*frame),
                        TimelineTarget::Label(label) => self
                            .project
                            .runtime
                            .resolve_frame_label(object.symbol, label)
                            .or_else(|| label.parse().ok()),
                    };
                    if let Some(frame) = frame {
                        if self.transitions == 0 {
                            object.failed = true;
                            object.playing = false;
                            self.diagnostic(
                                &object.name,
                                "object frame transition budget exceeded",
                            );
                            break;
                        }
                        self.transitions -= 1;
                        object.playing = playing;
                        let count = self
                            .project
                            .q0rgs
                            .iter()
                            .find(|q| q.q0rg_id == object.symbol)
                            .map_or(1, |q| q.frame_count.max(1));
                        jump = Some(frame.min(count - 1));
                    } else {
                        self.diagnostic(&object.name, format!("unknown frame label: {target:?}"));
                    }
                }
                RuntimeAction::SceneSwitch { .. } => self.scene_actions.push(action),
                RuntimeAction::ShellCommand { .. } => {}
                action => {
                    if let Err(message) =
                        apply_rig_action(self.project, object.symbol, scene, action)
                    {
                        self.diagnostic(&object.name, message);
                    }
                }
            }
        }
        jump
    }
}

fn number(runtime: &Runtime, name: &str) -> Option<f64> {
    match runtime.vars.get(name) {
        Some(RuntimeValue::Number(value)) if value.is_finite() => Some(*value),
        _ => None,
    }
}
fn boolean(runtime: &Runtime, name: &str) -> Option<bool> {
    boolean_from(&runtime.vars, name)
}
fn boolean_from(vars: &BTreeMap<String, RuntimeValue>, name: &str) -> Option<bool> {
    match vars.get(name) {
        Some(RuntimeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_globals(
    runtime: &mut Runtime,
    globals: &BTreeMap<String, RuntimeValue>,
    project: &ProjectV2,
    owner: u16,
    object: &ObjectInstance,
    key: InstanceKey,
    local: Affine,
    scene: &RuntimeSceneState,
    metrics: Option<q0s_format::raster::RuntimeInstanceMetrics>,
    bounds: &RuntimeBoundsCache,
    hovered: bool,
    local_pointer: Option<Vec2>,
) {
    for (name, value) in globals {
        runtime.vars.insert(name.clone(), value.clone());
    }
    for (name, value) in [
        ("self.x", f64::from(local.tx)),
        ("self.y", f64::from(local.ty)),
        ("self.frame", f64::from(object.frame)),
        ("frame", f64::from(object.frame)),
        ("self.id", f64::from(key.instance_id)),
    ] {
        runtime
            .vars
            .insert(name.into(), RuntimeValue::Number(value));
    }
    runtime.vars.insert(
        "self.name".into(),
        RuntimeValue::String(object.name.clone()),
    );
    runtime
        .vars
        .insert("self.playing".into(), RuntimeValue::Bool(object.playing));
    runtime
        .vars
        .insert("self.hovered".into(), RuntimeValue::Bool(hovered));
    if let Some(point) = local_pointer {
        runtime.vars.insert(
            "self.mouse_x".into(),
            RuntimeValue::Number(f64::from(point.x)),
        );
        runtime.vars.insert(
            "self.mouse_y".into(),
            RuntimeValue::Number(f64::from(point.y)),
        );
    }
    if let Some(metrics) = metrics {
        insert_metrics(runtime, "self", metrics, false);
    }
    for (key, name) in &project.runtime.instance_names {
        if key.q0rg_id != owner {
            continue;
        }
        if let Some(metrics) = runtime_instance_metrics_with_cache(
            project,
            owner,
            object.frame,
            key.instance_id,
            scene,
            bounds,
        ) {
            insert_metrics(runtime, name, metrics, true);
        }
    }
    object
        .children
        .sync_playback_globals(project, owner, runtime);
}

fn insert_metrics(
    runtime: &mut Runtime,
    name: &str,
    metrics: q0s_format::raster::RuntimeInstanceMetrics,
    position: bool,
) {
    for (suffix, value) in [
        ("x", metrics.x),
        ("y", metrics.y),
        ("left", metrics.left),
        ("right", metrics.right),
        ("top", metrics.top),
        ("bottom", metrics.bottom),
        ("width", metrics.width()),
        ("height", metrics.height()),
    ] {
        if !position && matches!(suffix, "x" | "y") {
            continue;
        }
        runtime.vars.insert(
            format!("{name}.{suffix}"),
            RuntimeValue::Number(f64::from(value)),
        );
    }
}

fn write_named_positions(
    runtime: &Runtime,
    project: &ProjectV2,
    owner: u16,
    scene: &mut RuntimeSceneState,
) {
    for (key, name) in &project.runtime.instance_names {
        if key.q0rg_id == owner {
            if let (Some(x), Some(y)) = (
                number(runtime, &format!("{name}.x")),
                number(runtime, &format!("{name}.y")),
            ) {
                scene.set_position(*key, x as f32, y as f32);
            }
        }
    }
}

fn apply_rig_action(
    project: &ProjectV2,
    owner: u16,
    scene: &mut RuntimeSceneState,
    action: RuntimeAction,
) -> Result<(), String> {
    let rig = q0s_format::rig::rig_for_q0rg(project, owner).ok_or("symbol has no rig")?;
    let control_id = |name: &str| {
        rig.controls
            .iter()
            .find(|control| control.name == name && control.public_in_simple)
            .map(|control| control.control_id)
            .ok_or_else(|| format!("rig control `{name}` is missing or private"))
    };
    let pose_id = |name: &str| {
        rig.poses
            .iter()
            .find(|pose| pose.name == name)
            .map(|pose| pose.pose_id)
            .ok_or_else(|| format!("rig pose `{name}` is missing"))
    };
    let overrides = scene.rig_overrides.entry(owner).or_default();
    match action {
        RuntimeAction::RigSetPosition { control, x, y } => {
            let control_id = control_id(&control)?;
            if !(x as f32).is_finite() || !(y as f32).is_finite() {
                return Err("rig position must be finite".into());
            }
            overrides.retain(|entry| !matches!(entry, RigControlOverride::Position { control_id: id, .. } if *id == control_id));
            overrides.push(RigControlOverride::Position {
                control_id,
                x: x as f32,
                y: y as f32,
            });
        }
        RuntimeAction::RigSetValue { control, value } => {
            let control_id = control_id(&control)?;
            if !(value as f32).is_finite() {
                return Err("rig value must be finite".into());
            }
            overrides.retain(|entry| !matches!(entry, RigControlOverride::Value { control_id: id, .. } if *id == control_id));
            overrides.push(RigControlOverride::Value {
                control_id,
                value: value as f32,
            });
        }
        RuntimeAction::RigReset { control } => {
            let id = control_id(&control)?;
            overrides.retain(|entry| !matches!(entry, RigControlOverride::Position { control_id, .. } | RigControlOverride::Value { control_id, .. } if *control_id == id));
        }
        RuntimeAction::RigSetPose { pose, weight } => {
            let pose_id = pose_id(&pose)?;
            if !weight.is_finite() {
                return Err("rig pose weight must be finite".into());
            }
            overrides.retain(|entry| !matches!(entry, RigControlOverride::Pose { pose_id: id, .. } if *id == pose_id));
            overrides.push(RigControlOverride::Pose {
                pose_id,
                weight: weight.clamp(0.0, 1.0) as f32,
            });
        }
        RuntimeAction::RigResetPose { pose } => {
            let id = pose_id(&pose)?;
            overrides.retain(|entry| !matches!(entry, RigControlOverride::Pose { pose_id, .. } if *pose_id == id));
        }
        _ => {}
    }
    Ok(())
}

impl Player {
    pub fn has_autonomous_objects(&self) -> bool {
        self.objects.has_active_scripts()
    }
    pub(crate) fn update_objects(&mut self, advance: bool) {
        self.object_pass(advance, None);
    }

    pub(crate) fn dispatch_object_pointer(&mut self, event: &str, args: &[RuntimeValue]) {
        self.object_pass(false, Some((event, args)));
    }

    fn object_pass(&mut self, advance: bool, event: Option<(&str, &[RuntimeValue])>) {
        let (project, owner, frame) = match &self.inner {
            Inner::V2 {
                project,
                entry_q0rg_id,
                frame_index,
                ..
            } => (Arc::clone(project), *entry_q0rg_id, *frame_index),
            _ => return,
        };
        self.sync_q0lang_host_globals();
        let globals = self
            .q0lang
            .as_ref()
            .map(|runtime| {
                runtime
                    .vars
                    .iter()
                    .filter(|(name, _)| {
                        name.starts_with("mouse.")
                            || name.starts_with("input.")
                            || name.starts_with("stage.")
                            || name.starts_with("time.")
                    })
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut pass = ObjectPass {
            project: &project,
            bounds: &self
                .runtime_bounds_cache
                .as_ref()
                .expect("v2 bounds initialized by host sync")
                .1,
            globals: &globals,
            instructions: OBJECT_INSTRUCTION_BUDGET,
            transitions: MAX_FRAME_SCRIPT_TRANSITIONS,
            remaining_objects: MAX_ACTIVE_OBJECTS,
            diagnostics: Vec::new(),
            scene_actions: Vec::new(),
            advance,
            event,
        };
        self.scene_runtime.rig_overrides = self.rig_runtime_overrides.clone();
        pass.visit(
            &mut self.objects,
            &mut self.scene_runtime,
            owner,
            frame,
            Affine::IDENTITY,
            0,
        );
        let available =
            MAX_RUNTIME_DIAGNOSTICS.saturating_sub(self.initial_script_diagnostics.len());
        self.initial_script_diagnostics
            .extend(pass.diagnostics.into_iter().take(available));
        if !pass.scene_actions.is_empty() {
            self.apply_q0lang_actions_with_budget(&pass.scene_actions, &mut pass.transitions);
        }
    }

    pub(crate) fn sync_object_playback_globals(&mut self) {
        if let Inner::V2 {
            project,
            entry_q0rg_id,
            ..
        } = &self.inner
        {
            if let Some(runtime) = &mut self.q0lang {
                self.objects
                    .sync_playback_globals(project, *entry_q0rg_id, runtime);
            }
        }
    }

    pub(crate) fn apply_object_playback_writes(&mut self) {
        if let Inner::V2 {
            project,
            entry_q0rg_id,
            ..
        } = &self.inner
        {
            if let Some(runtime) = &self.q0lang {
                self.objects.collect_playback_writes(
                    project,
                    *entry_q0rg_id,
                    runtime,
                    &mut self.scene_runtime,
                );
            }
        }
    }
}
