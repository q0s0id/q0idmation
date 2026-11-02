//! q0.layer and q0.window in the player: the questions scripts ask about
//! what layers draw, layers that stop instances, and what scripts ask of
//! the window.

use std::collections::BTreeMap;
use std::sync::Arc;

use q0s_format::collide::{self, Bounds, CollisionCache, Drawing, Region};
use q0s_format::q0lang::runtime::{
    layer_ref, HostQuery, LayerCommand, LayerRef, RuntimeDiagnostic, RuntimeValue, WindowCommand,
};
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::v2::{InstanceKey, ProjectV2, Vec2};

use crate::{Inner, Player};

/// Window requests kept for the app to pick up; a script that asks every
/// frame with no window to answer must not grow this forever.
const MAX_WINDOW_REQUESTS: usize = 256;

/// The window as the app last saw it, in screen points. Scripts read it as
/// `window.*` and `screen.*`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowInfo {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub screen_width: f32,
    pub screen_height: f32,
    pub fullscreen: bool,
    pub ui: bool,
}

/// Answers `q0layer.*(...)` for a script of the movie's own timeline.
pub(crate) struct LayerHost<'a> {
    pub project: &'a ProjectV2,
    pub owner: u16,
    pub frame: u16,
    pub scene: &'a RuntimeSceneState,
    pub cache: &'a mut CollisionCache,
}

fn number(vars: &BTreeMap<String, RuntimeValue>, name: &str) -> Option<f32> {
    match vars.get(name) {
        Some(RuntimeValue::Number(value)) if value.is_finite() => Some(*value as f32),
        _ => None,
    }
}

fn number_arg(value: &RuntimeValue, function: &str) -> Result<f32, String> {
    match value {
        RuntimeValue::Number(value) if value.is_finite() => Ok(*value as f32),
        other => Err(format!(
            "{function} wants a number here, not {}",
            other.kind_name()
        )),
    }
}

/// The layers `layer` names in timeline `owner`: one, or all in a folder.
pub(crate) fn layer_ids(project: &ProjectV2, owner: u16, layer: &LayerRef) -> Vec<u16> {
    match layer {
        LayerRef::Name(name) => collide::find_layers(project, owner, Some(name), None),
        LayerRef::Row(row) => collide::find_layers(project, owner, None, Some(*row)),
    }
}

fn the_layer(project: &ProjectV2, owner: u16, layer: &LayerRef) -> Option<u16> {
    match layer {
        LayerRef::Name(name) => collide::find_layer(project, owner, Some(name), None),
        LayerRef::Row(row) => collide::find_layer(project, owner, None, Some(*row)),
    }
}

fn instance_named(project: &ProjectV2, owner: u16, name: &str) -> Option<u32> {
    project
        .runtime
        .instance_names
        .iter()
        .find(|(key, instance)| key.q0rg_id == owner && instance.as_str() == name)
        .map(|(key, _)| key.instance_id)
}

impl LayerHost<'_> {
    fn layers(&self, value: &RuntimeValue, function: &str) -> Result<Vec<u16>, String> {
        let layer = layer_ref(value).ok_or_else(|| {
            format!(
                "{function}: name a layer, or give its row in the timeline counted from the top"
            )
        })?;
        let ids = layer_ids(self.project, self.owner, &layer);
        if ids.is_empty() {
            Err(format!("{function}: there is no layer {layer}"))
        } else {
            Ok(ids)
        }
    }

    /// What `value` names, drawn where the script has put it by now, and the
    /// instance to leave out of the layer it asks about.
    fn thing(
        &mut self,
        value: &RuntimeValue,
        vars: &BTreeMap<String, RuntimeValue>,
        function: &str,
    ) -> Result<(Drawing, u32), String> {
        let name = match value {
            RuntimeValue::String(name) | RuntimeValue::Ident(name) | RuntimeValue::Raw(name) => {
                name.as_str()
            }
            _ => return Err(format!("{function}: name an instance or a layer")),
        };
        if let Some(instance) = instance_named(self.project, self.owner, name) {
            let mut drawing = collide::instance_drawing(
                self.project,
                self.owner,
                self.frame,
                instance,
                self.scene,
                self.cache,
            );
            // Writes to `name.x` land after the script, so a script that
            // moved something asks about where it moved it to.
            if let Some(origin) =
                collide::instance_origin(self.project, self.owner, self.frame, instance, self.scene)
            {
                let x = number(vars, &format!("{name}.x")).unwrap_or(origin.x);
                let y = number(vars, &format!("{name}.y")).unwrap_or(origin.y);
                drawing = drawing.translated(x - origin.x, y - origin.y);
            }
            return Ok((drawing, instance));
        }
        let ids = collide::find_layers(self.project, self.owner, Some(name), None);
        if !ids.is_empty() {
            let drawing = collide::layer_drawing(
                self.project,
                self.owner,
                self.frame,
                &ids,
                self.scene,
                self.cache,
                None,
                0,
            );
            return Ok((drawing, 0));
        }
        Err(format!(
            "{function}: there is no instance or layer called {name:?}"
        ))
    }

    fn drawing(&mut self, ids: &[u16], within: Bounds, skip: u32) -> Drawing {
        collide::layer_drawing(
            self.project,
            self.owner,
            self.frame,
            ids,
            self.scene,
            self.cache,
            Some(within),
            skip,
        )
    }
}

impl HostQuery for LayerHost<'_> {
    fn query(
        &mut self,
        name: &str,
        args: &[RuntimeValue],
        vars: &BTreeMap<String, RuntimeValue>,
    ) -> Result<RuntimeValue, String> {
        match (name, args) {
            ("q0layer.touches" | "q0layer.which", [layer, thing]) => {
                let ids = self.layers(layer, name)?;
                let (thing, skip) = self.thing(thing, vars, name)?;
                let met = match thing.bounds() {
                    Some(reach) => {
                        let drawing = self.drawing(&ids, reach.expand(1.0), skip);
                        drawing.first_met_by(&thing).map(str::to_string)
                    }
                    None => None,
                };
                Ok(if name == "q0layer.touches" {
                    RuntimeValue::Bool(met.is_some())
                } else {
                    met.map_or(RuntimeValue::Null, RuntimeValue::String)
                })
            }
            ("q0layer.hit", [layer, x, y]) => {
                let ids = self.layers(layer, name)?;
                let point = Vec2::new(number_arg(x, name)?, number_arg(y, name)?);
                let drawing = self.drawing(
                    &ids,
                    Bounds {
                        min: point,
                        max: point,
                    },
                    0,
                );
                Ok(RuntimeValue::Bool(drawing.object_at(point).is_some()))
            }
            ("q0layer.circle", [layer, x, y, radius]) => {
                let ids = self.layers(layer, name)?;
                let centre = Vec2::new(number_arg(x, name)?, number_arg(y, name)?);
                let radius = number_arg(radius, name)?.max(0.0);
                let reach = Bounds {
                    min: centre,
                    max: centre,
                }
                .expand(radius);
                let drawing = self.drawing(&ids, reach, 0);
                Ok(RuntimeValue::Bool(
                    drawing.object_in_circle(centre, radius).is_some(),
                ))
            }
            ("q0layer.box", [layer, left, top, width, height]) => {
                let ids = self.layers(layer, name)?;
                let (left, top) = (number_arg(left, name)?, number_arg(top, name)?);
                let (width, height) = (number_arg(width, name)?, number_arg(height, name)?);
                let Some(region) = Region::rect(left, top, width, height) else {
                    return Ok(RuntimeValue::Bool(false));
                };
                let thing = Drawing {
                    regions: vec![region],
                    objects: vec!["box".to_string()],
                };
                let reach = thing.bounds().expect("one region").expand(1.0);
                let drawing = self.drawing(&ids, reach, 0);
                Ok(RuntimeValue::Bool(drawing.meets(&thing)))
            }
            ("q0layer.visible", [layer]) => {
                let layer = layer_ref(layer).ok_or_else(|| format!("{name}: name a layer"))?;
                let id = the_layer(self.project, self.owner, &layer)
                    .ok_or_else(|| format!("{name}: there is no layer {layer}"))?;
                Ok(RuntimeValue::Bool(
                    self.scene.layer_look(self.project, self.owner, id).visible,
                ))
            }
            ("q0layer.count", [layer]) => {
                let ids = self.layers(layer, name)?;
                Ok(RuntimeValue::Number(collide::layer_object_count(
                    self.project,
                    self.owner,
                    self.frame,
                    &ids,
                ) as f64))
            }
            _ => Err(
                match q0s_format::q0lang::parser::HOST_FUNCTIONS
                    .iter()
                    .find(|(known, _)| *known == name)
                {
                    Some((_, wanted)) => {
                        format!("`{name}` takes {wanted} values, got {}", args.len())
                    }
                    None => format!("unknown q0.layer function `{name}`"),
                },
            ),
        }
    }
}

/// How far along a move of `distance` stays free, from 0 to 1. The move is
/// walked in steps of `step`, so a fast thing cannot jump over a thin line.
fn farthest(free: impl Fn(f32) -> bool, distance: f32, step: f32) -> f32 {
    let steps = (distance.abs() / step.max(0.5)).ceil().clamp(1.0, 256.0) as usize;
    let mut last_free = 0.0_f32;
    for index in 1..=steps {
        let t = index as f32 / steps as f32;
        if free(t) {
            last_free = t;
            continue;
        }
        let (mut low, mut high) = (last_free, t);
        for _ in 0..10 {
            let middle = (low + high) / 2.0;
            if free(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        return low;
    }
    1.0
}

/// How far along `distance` a move stopped at fraction `t` goes, kept a
/// hair from the wall it stopped at: touching counts as inside, and a thing
/// left touching a wall could walk through it next time.
fn stop_short(distance: f32, t: f32) -> f32 {
    const GAP: f32 = 0.05;
    if t >= 1.0 {
        return distance;
    }
    let along = distance * t;
    if along.abs() <= GAP {
        0.0
    } else {
        along - GAP * distance.signum()
    }
}

impl Player {
    /// The parts a script of the movie's own timeline runs with: its
    /// runtime, and the host that answers its layer questions.
    pub(crate) fn script_parts(
        &mut self,
    ) -> Option<(&mut q0s_format::q0lang::runtime::Runtime, LayerHost<'_>)> {
        let Inner::V2 {
            project,
            entry_q0rg_id,
            frame_index,
            ..
        } = &self.inner
        else {
            return None;
        };
        if self
            .collision
            .as_ref()
            .is_none_or(|(source, _)| !Arc::ptr_eq(source, project))
        {
            self.collision = Some((Arc::clone(project), CollisionCache::new()));
        }
        let runtime = self.q0lang.as_mut()?;
        let cache = &mut self.collision.as_mut()?.1;
        Some((
            runtime,
            LayerHost {
                project,
                owner: *entry_q0rg_id,
                frame: *frame_index,
                scene: &self.scene_runtime,
                cache,
            },
        ))
    }

    fn layer_diagnostic(&mut self, message: String) {
        if self.initial_script_diagnostics.len() < crate::objects::MAX_RUNTIME_DIAGNOSTICS {
            self.initial_script_diagnostics
                .push(RuntimeDiagnostic { line: 0, message });
        }
    }

    pub(crate) fn apply_layer_command(&mut self, command: &LayerCommand) {
        let Inner::V2 {
            project,
            entry_q0rg_id,
            ..
        } = &self.inner
        else {
            return;
        };
        let (project, owner) = (Arc::clone(project), *entry_q0rg_id);
        let layer = match command {
            LayerCommand::Hide { layer }
            | LayerCommand::Show { layer }
            | LayerCommand::Move { layer, .. }
            | LayerCommand::Alpha { layer, .. }
            | LayerCommand::Reset { layer }
            | LayerCommand::Solid { layer, .. }
            | LayerCommand::Unsolid { layer, .. } => layer,
        };
        let Some(id) = the_layer(&project, owner, layer) else {
            self.layer_diagnostic(format!("q0.layer: there is no layer {layer}"));
            return;
        };
        match command {
            LayerCommand::Hide { .. } => self.scene_runtime.set_layer_hidden(id, true),
            LayerCommand::Show { .. } => self.scene_runtime.set_layer_hidden(id, false),
            LayerCommand::Move { x, y, .. } => self
                .scene_runtime
                .set_layer_offset(id, *x as f32, *y as f32),
            LayerCommand::Alpha { value, .. } => {
                self.scene_runtime.set_layer_alpha(id, *value as f32)
            }
            LayerCommand::Reset { .. } => self.scene_runtime.reset_layer(id),
            LayerCommand::Solid { layer, thing } => {
                if instance_named(&project, owner, thing).is_none() {
                    self.layer_diagnostic(format!(
                        "q0layer.solid!: there is no instance called {thing:?}"
                    ));
                    return;
                }
                let pair = (layer.clone(), thing.clone());
                if !self.solids.contains(&pair) {
                    self.solids.push(pair);
                }
            }
            LayerCommand::Unsolid { layer, thing } => {
                self.solids
                    .retain(|(solid, solid_thing)| !(solid == layer && solid_thing == thing));
            }
        }
    }

    /// Where an instance that a script moved to `x`, `y` may go: solid
    /// layers stop it at their edge, and it slides along them. Something
    /// already inside a solid layer may move freely, so it can get out.
    pub(crate) fn stop_at_solids(
        &mut self,
        key: InstanceKey,
        name: &str,
        x: f32,
        y: f32,
    ) -> (f32, f32) {
        let Inner::V2 {
            project,
            entry_q0rg_id,
            frame_index,
            ..
        } = &self.inner
        else {
            return (x, y);
        };
        let (project, owner, frame) = (Arc::clone(project), *entry_q0rg_id, *frame_index);
        let layers = self
            .solids
            .iter()
            .filter(|(_, thing)| thing == name)
            .flat_map(|(layer, _)| layer_ids(&project, owner, layer))
            .collect::<Vec<_>>();
        if layers.is_empty() {
            return (x, y);
        }
        let Some(origin) =
            collide::instance_origin(&project, owner, frame, key.instance_id, &self.scene_runtime)
        else {
            return (x, y);
        };
        let (dx, dy) = (x - origin.x, y - origin.y);
        if dx == 0.0 && dy == 0.0 {
            return (x, y);
        }
        if self
            .collision
            .as_ref()
            .is_none_or(|(source, _)| !Arc::ptr_eq(source, &project))
        {
            self.collision = Some((Arc::clone(&project), CollisionCache::new()));
        }
        let cache = &mut self.collision.as_mut().expect("collision cache").1;
        let thing = collide::instance_drawing(
            &project,
            owner,
            frame,
            key.instance_id,
            &self.scene_runtime,
            cache,
        );
        let Some(start) = thing.bounds() else {
            return (x, y);
        };
        let reach = start.union(start.translate(dx, dy)).expand(1.0);
        let walls = collide::layer_drawing(
            &project,
            owner,
            frame,
            &layers,
            &self.scene_runtime,
            cache,
            Some(reach),
            key.instance_id,
        );
        if walls.is_empty() || walls.meets(&thing) {
            return (x, y);
        }
        let free = |ox: f32, oy: f32| !walls.meets(&thing.translated(ox, oy));
        // Half the thing's size: no line can slip between two steps.
        let step = ((start.max.x - start.min.x).min(start.max.y - start.min.y) / 2.0).max(1.0);
        let ox = stop_short(dx, farthest(|t| free(dx * t, 0.0), dx, step));
        let oy = stop_short(dy, farthest(|t| free(ox, dy * t), dy, step));
        (origin.x + ox, origin.y + oy)
    }

    /// Tells scripts where the window is. The app calls this every frame.
    pub fn set_window_info(&mut self, info: WindowInfo) {
        self.window = info;
    }

    /// What scripts asked of the window since the last call.
    pub fn take_window_requests(&mut self) -> Vec<WindowCommand> {
        std::mem::take(&mut self.window_requests)
    }

    pub(crate) fn request_window(&mut self, command: WindowCommand) {
        // Later scripts of the same frame read where the window is going.
        match &command {
            WindowCommand::Move { x, y } => {
                self.window.x = *x as f32;
                self.window.y = *y as f32;
            }
            WindowCommand::Size { width, height } => {
                self.window.width = *width as f32;
                self.window.height = *height as f32;
            }
            WindowCommand::Fullscreen(on) => self.window.fullscreen = *on,
            WindowCommand::Ui(on) => self.window.ui = *on,
            _ => {}
        }
        if self.window_requests.len() >= MAX_WINDOW_REQUESTS {
            self.window_requests.remove(0);
        }
        self.window_requests.push(command);
    }

    pub(crate) fn window_globals(&self) -> [(&'static str, RuntimeValue); 8] {
        let number = |value: f32| RuntimeValue::Number(f64::from(value));
        [
            ("window.x", number(self.window.x)),
            ("window.y", number(self.window.y)),
            ("window.width", number(self.window.width)),
            ("window.height", number(self.window.height)),
            (
                "window.fullscreen",
                RuntimeValue::Bool(self.window.fullscreen),
            ),
            ("window.ui", RuntimeValue::Bool(self.window.ui)),
            ("screen.width", number(self.window.screen_width)),
            ("screen.height", number(self.window.screen_height)),
        ]
    }

    /// A script that wrote `window.x` and friends moves or sizes the window.
    pub(crate) fn apply_window_writes(&mut self) {
        let Some(runtime) = self.q0lang.as_ref() else {
            return;
        };
        let read = |name: &str| number(&runtime.vars, name);
        let changed =
            |value: Option<f32>, now: f32| value.filter(|value| (value - now).abs() > 0.01);
        let (x, y) = (read("window.x"), read("window.y"));
        let (width, height) = (read("window.width"), read("window.height"));
        let moved = changed(x, self.window.x).is_some() || changed(y, self.window.y).is_some();
        let sized = changed(width, self.window.width).is_some()
            || changed(height, self.window.height).is_some();
        let window = self.window;
        if moved {
            self.request_window(WindowCommand::Move {
                x: f64::from(x.unwrap_or(window.x)),
                y: f64::from(y.unwrap_or(window.y)),
            });
        }
        if sized {
            self.request_window(WindowCommand::Size {
                width: f64::from(width.unwrap_or(window.width).max(1.0)),
                height: f64::from(height.unwrap_or(window.height).max(1.0)),
            });
        }
    }
}
