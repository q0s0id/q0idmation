//! What q0ex code can reach in the editor: the `editor`, `project`, `ui`,
//! `draw`, `file`, `settings`, `config`, `clip` and `plugin` namespaces,
//! and the layers, objects, assets and timelines handed out as handles.
//!
//! Edits a handler makes become one undo step. `raw` reads and writes any
//! part of the project as plain data, which is how a plugin can change
//! things this API has no word for.

use egui::{Color32, Context, Painter, Pos2, Ui};
use q0ex::{Handle, Interp, Value};
use q0s_format::geom::{CapShape, JoinShape};
use q0s_format::v2::{
    Anchor, Asset, LayerKey, LayerKind, Path as VPath, Placement, Q0rg, Rgba, Stroke as VStroke,
    Target, Transform2D, Tween, Vec2, VectorAsset,
};

use super::{LineKind, Scene};
use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{PlacementRef, Selection};

/// Namespaces and core commands the host answers for.
const NAMESPACES: [&str; 13] = [
    "editor", "project", "ui", "draw", "file", "settings", "config", "clip", "plugin", "keys",
    "say!", "print!", "mouse",
];

pub(super) struct EditorHost<'a> {
    app: &'a mut EditorApp,
    ctx: Context,
    index: usize,
    id: String,
    name: String,
    color: Color32,
    /// The window being drawn, innermost last.
    ui: Vec<*mut Ui>,
    draw: Option<(*const Painter, StageView)>,
    snapshotted: bool,
    changed: bool,
    changed_artwork: bool,
    /// `plugin.stop!` was called.
    pub stop_requested: bool,
}

fn num(args: &[Value], index: usize, what: &str) -> Result<f64, String> {
    match args.get(index) {
        Some(Value::Num(value)) if value.is_finite() => Ok(*value),
        Some(other) => Err(format!("{what} needs a number, not {}", other.type_name())),
        None => Err(format!("{what} needs a number")),
    }
}

fn num_or(args: &[Value], index: usize, what: &str, default: f64) -> Result<f64, String> {
    match args.get(index) {
        None | Some(Value::Null) => Ok(default),
        _ => num(args, index, what),
    }
}

fn text(args: &[Value], index: usize, what: &str) -> Result<String, String> {
    match args.get(index) {
        Some(Value::Str(text)) => Ok(text.to_string()),
        Some(Value::Num(_) | Value::Bool(_)) => Ok(args[index].to_text()),
        Some(other) => Err(format!("{what} needs text, not {}", other.type_name())),
        None => Err(format!("{what} needs text")),
    }
}

fn color_of(value: &Value, what: &str) -> Result<Rgba, String> {
    let Value::Str(text) = value else {
        return Err(format!(
            "{what} needs a colour like \"#ff8800\", not {}",
            value.type_name()
        ));
    };
    let [r, g, b, a] = q0ex::stdlib::parse_color(text)
        .ok_or_else(|| format!("{what}: `{text}` is not a colour; write it like \"#ff8800\""))?;
    Ok(Rgba { r, g, b, a })
}

fn color_arg(args: &[Value], index: usize, what: &str, default: Rgba) -> Result<Rgba, String> {
    match args.get(index) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => color_of(value, what),
    }
}

fn color32(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn color_value(color: Rgba) -> Value {
    Value::str(q0ex::stdlib::color_text([
        color.r, color.g, color.b, color.a,
    ]))
}

/// `[x, y]` or `{x: .., y: ..}`.
fn point_of(value: &Value) -> Option<Vec2> {
    match value {
        Value::List(items) => {
            let items = items.borrow();
            Some(Vec2::new(
                items.first()?.as_num()? as f32,
                items.get(1)?.as_num()? as f32,
            ))
        }
        Value::Map(entries) => {
            let entries = entries.borrow();
            Some(Vec2::new(
                entries.get("x")?.as_num()? as f32,
                entries.get("y")?.as_num()? as f32,
            ))
        }
        _ => None,
    }
}

fn points_arg(args: &[Value], index: usize, what: &str) -> Result<Vec<Vec2>, String> {
    let Some(Value::List(items)) = args.get(index) else {
        return Err(format!(
            "{what} needs a list of points like [[0, 0], [10, 5]]"
        ));
    };
    items
        .borrow()
        .iter()
        .map(|item| {
            point_of(item).ok_or_else(|| format!("{what}: every point is [x, y] or {{x:, y:}}"))
        })
        .collect()
}

fn point_value(x: f32, y: f32) -> Value {
    Value::map([
        ("x".to_string(), Value::Num(f64::from(x))),
        ("y".to_string(), Value::Num(f64::from(y))),
    ])
}

fn timeline_handle(q0rg_id: u16) -> Value {
    Value::handle("timeline", vec![i64::from(q0rg_id)])
}

fn layer_handle(q0rg_id: u16, layer_id: u16) -> Value {
    Value::handle("layer", vec![i64::from(q0rg_id), i64::from(layer_id)])
}

fn object_handle(q0rg_id: u16, layer_id: u16, index: usize) -> Value {
    Value::handle(
        "object",
        vec![i64::from(q0rg_id), i64::from(layer_id), index as i64],
    )
}

fn asset_handle(asset_id: u16) -> Value {
    Value::handle("asset", vec![i64::from(asset_id)])
}

fn id16(handle: &Handle, at: usize) -> Result<u16, String> {
    handle
        .ids
        .get(at)
        .and_then(|id| u16::try_from(*id).ok())
        .ok_or_else(|| format!("this {} handle is damaged", handle.kind))
}

/// The keyframe in effect on `frame` of a layer, if the layer reaches it.
pub(super) fn active_key(q0rg: &Q0rg, layer: &q0s_format::v2::Layer, frame: u16) -> Option<u16> {
    let length = layer.timeline_frame_count.unwrap_or(q0rg.frame_count);
    if frame >= length.max(1) {
        return None;
    }
    layer
        .keyframe_frames()
        .into_iter()
        .filter(|key| *key <= frame)
        .max()
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value, String> {
    serde_json::to_value(value)
        .map(|json| Value::from_json(&json))
        .map_err(|error| format!("could not read it as data: {error}"))
}

fn from_json<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> Result<T, String> {
    serde_json::from_value(value.to_json())
        .map_err(|error| format!("{what}: the data does not fit: {error}"))
}

fn next_instance_id(project: &q0s_format::v2::ProjectV2) -> Option<u32> {
    let used = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .map(|placement| placement.instance_id)
        .collect::<std::collections::HashSet<_>>();
    (1..=u32::MAX).find(|candidate| !used.contains(candidate))
}

fn tool_by_name(name: &str) -> Option<crate::state::Tool> {
    crate::state::Tool::ALL
        .into_iter()
        .find(|tool| tool.label().eq_ignore_ascii_case(name.trim()))
}

/// Follows `a.b.0.c` into JSON.
fn json_path<'a>(json: &'a mut serde_json::Value, path: &str) -> Option<&'a mut serde_json::Value> {
    let mut at = json;
    for part in path.split('.').filter(|part| !part.is_empty()) {
        at = match at {
            serde_json::Value::Object(entries) => entries.get_mut(part)?,
            serde_json::Value::Array(items) => items.get_mut(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(at)
}

impl<'a> EditorHost<'a> {
    pub fn new(app: &'a mut EditorApp, ctx: &Context, index: usize) -> Self {
        let plugin = &app.plugins.list[index];
        let id = plugin.meta.id.clone();
        let name = plugin.meta.name.clone();
        let color = plugin.accent();
        Self {
            app,
            ctx: ctx.clone(),
            index,
            id,
            name,
            color,
            ui: Vec::new(),
            draw: None,
            snapshotted: false,
            changed: false,
            changed_artwork: false,
            stop_requested: false,
        }
    }

    pub fn scene(&mut self, scene: Scene<'_>) {
        if let Some(ui) = scene.ui {
            self.ui.push(ui as *mut Ui);
        }
        if let Some((painter, view)) = scene.draw {
            self.draw = Some((painter as *const Painter, *view));
        }
    }

    /// Settles what the handler changed: one undo step, fresh caches.
    pub fn finish(self) {
        if self.changed {
            self.app.state.mark_dirty();
        }
        if self.changed_artwork {
            self.app.textures.invalidate();
        }
    }

    /// Call before changing the project: the first change of a run saves
    /// the undo step.
    fn touch(&mut self) {
        if !self.snapshotted {
            self.app
                .history
                .snapshot_with_session(&self.app.state.project, &self.app.session);
            self.snapshotted = true;
        }
        self.changed = true;
    }

    fn touch_artwork(&mut self) {
        self.touch();
        self.changed_artwork = true;
    }

    fn ui(&mut self) -> Result<&'a mut Ui, String> {
        let pointer = *self
            .ui
            .last()
            .ok_or("ui works inside `when window` blocks")?;
        // SAFETY: the pointer comes from a `&mut Ui` that outlives this run
        // (see `Scene`), or from a nested egui closure that is still running.
        Ok(unsafe { &mut *pointer })
    }

    fn painter(&self) -> Result<(&'a Painter, StageView), String> {
        let (pointer, view) = self.draw.ok_or("draw works inside `when draw` blocks")?;
        // SAFETY: as for `ui`: the painter outlives the run.
        Ok((unsafe { &*pointer }, view))
    }

    /// Runs `func` with `ui` as the window being drawn.
    fn nested(&mut self, it: &mut Interp, ui: &mut Ui, func: &Value) -> Result<(), String> {
        self.ui.push(ui as *mut Ui);
        let outcome = it.call_value(func, vec![], self);
        self.ui.pop();
        outcome.map(|_| ()).map_err(|error| error.message)
    }

    fn line(&mut self, kind: LineKind, args: &[Value]) -> String {
        let text = args
            .iter()
            .map(Value::to_text)
            .collect::<Vec<_>>()
            .join(" ");
        self.app.plugins.log(&self.name, kind, text.clone());
        text
    }

    // -------------------------------------------------------- lookups

    fn q0rg(&self, q0rg_id: u16) -> Result<&Q0rg, String> {
        self.app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .ok_or_else(|| "that timeline is gone".to_string())
    }

    fn q0rg_mut(&mut self, q0rg_id: u16) -> Result<&mut Q0rg, String> {
        self.app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .ok_or_else(|| "that timeline is gone".to_string())
    }

    fn layer_of(&self, handle: &Handle) -> Result<(u16, u16), String> {
        let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
        self.q0rg(q0rg_id)?
            .layers
            .iter()
            .any(|layer| layer.layer_id == layer_id)
            .then_some((q0rg_id, layer_id))
            .ok_or_else(|| "that layer is gone".to_string())
    }

    fn layer_mut(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
    ) -> Result<&mut q0s_format::v2::Layer, String> {
        self.q0rg_mut(q0rg_id)?
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)
            .ok_or_else(|| "that layer is gone".to_string())
    }

    fn placement(&self, handle: &Handle) -> Result<&Placement, String> {
        let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
        let index = handle.ids.get(2).copied().unwrap_or(-1);
        self.q0rg(q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
            .and_then(|layer| layer.placements.get(usize::try_from(index).ok()?))
            .ok_or_else(|| "that object is gone".to_string())
    }

    fn placement_mut(&mut self, handle: &Handle) -> Result<&mut Placement, String> {
        let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
        let index = usize::try_from(handle.ids.get(2).copied().unwrap_or(-1))
            .map_err(|_| "that object is gone".to_string())?;
        self.layer_mut(q0rg_id, layer_id)?
            .placements
            .get_mut(index)
            .ok_or_else(|| "that object is gone".to_string())
    }

    fn asset(&self, asset_id: u16) -> Result<&Asset, String> {
        self.app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .ok_or_else(|| "that library item is gone".to_string())
    }

    /// The objects of `layer` on `frame` (0-based).
    fn objects_on(&self, q0rg_id: u16, layer_id: u16, frame: u16) -> Result<Vec<Value>, String> {
        let q0rg = self.q0rg(q0rg_id)?;
        let Some(layer) = q0rg.layers.iter().find(|layer| layer.layer_id == layer_id) else {
            return Err("that layer is gone".into());
        };
        let Some(key) = active_key(q0rg, layer, frame) else {
            return Ok(Vec::new());
        };
        Ok(layer
            .placements
            .iter()
            .enumerate()
            .filter(|(_, placement)| placement.frame == key)
            .map(|(index, _)| object_handle(q0rg_id, layer_id, index))
            .collect())
    }

    fn selection_refs(&self) -> Vec<PlacementRef> {
        crate::selection_edit::selected_object_refs(
            &self.app.state.project,
            &self.app.session.selection,
        )
    }

    fn frame_arg(&self, args: &[Value], index: usize) -> Result<u16, String> {
        match args.get(index) {
            None | Some(Value::Null) => Ok(self.app.session.current_frame),
            _ => Ok((num(args, index, "the frame")?.round() as i64 - 1)
                .clamp(0, i64::from(u16::MAX)) as u16),
        }
    }

    /// Puts a new drawing on the current layer and frame.
    fn add_drawing(
        &mut self,
        path: VPath,
        fill: Option<Rgba>,
        stroke: Option<VStroke>,
    ) -> Result<Value, String> {
        let q0rg_id = self.app.session.current_q0rg_id;
        let layer_id = self.app.session.current_layer_id;
        let frame = self.app.session.current_frame;
        let project = &self.app.state.project;
        if project.layer_is_folder(q0rg_id, layer_id) {
            return Err("the current layer is a folder; pick a drawing layer".into());
        }
        if project.layer_is_locked(q0rg_id, layer_id) {
            return Err("the current layer is locked".into());
        }
        self.touch_artwork();
        crate::tools::materialize_layer_keyframe_for_edit(
            &mut self.app.state.project,
            q0rg_id,
            layer_id,
            frame,
        )
        .ok_or("this frame cannot be drawn on")?;
        let asset_id = crate::tools::next_asset_id_pub(&self.app.state.project);
        self.app
            .state
            .project
            .assets
            .push(Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![path],
                fill,
                stroke,
            }));
        let layer = self.layer_mut(q0rg_id, layer_id)?;
        layer.push_placement(Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        });
        let index = layer.placements.len() - 1;
        Ok(object_handle(q0rg_id, layer_id, index))
    }

    fn stroke_style(
        &self,
        args: &[Value],
        color_at: usize,
        width_at: usize,
    ) -> Result<VStroke, String> {
        Ok(VStroke {
            color: color_arg(
                args,
                color_at,
                "the line colour",
                self.app.session.stroke_color,
            )?,
            width: num_or(
                args,
                width_at,
                "the line width",
                f64::from(self.app.session.stroke_width),
            )?
            .max(0.1) as f32,
            cap: self.app.session.brush_cap,
            join: self.app.session.brush_join,
        })
    }

    fn config_value(&self, key: &str) -> Option<Value> {
        let plugin = &self.app.plugins.list[self.index];
        let decl = plugin.settings.iter().find(|decl| decl.key == key)?;
        if let Some(stored) = self
            .app
            .settings
            .plugins
            .values
            .get(&self.id)
            .and_then(|values| values.get(key))
        {
            return Some(Value::from_json(stored));
        }
        Some(super::manager::default_value(decl))
    }

    fn set_config(&mut self, key: &str, value: Value) -> Result<(), String> {
        let known = self.app.plugins.list[self.index]
            .settings
            .iter()
            .any(|decl| decl.key == key);
        if !known {
            return Err(format!("this plugin has no setting `{key}`"));
        }
        self.app
            .settings
            .plugins
            .values
            .entry(self.id.clone())
            .or_default()
            .insert(key.to_string(), value.to_json());
        self.app
            .plugins
            .settings_dirty
            .get_or_insert(std::time::Instant::now());
        Ok(())
    }
}

impl q0ex::Host for EditorHost<'_> {
    fn has_namespace(&self, name: &str) -> bool {
        NAMESPACES.contains(&name)
    }

    fn property(&mut self, _: &mut Interp, path: &str) -> Option<Result<Value, String>> {
        if let Some(key) = path.strip_prefix("config.") {
            return Some(
                self.config_value(key)
                    .ok_or_else(|| format!("this plugin has no setting `{key}`")),
            );
        }
        let meta = &self.app.plugins.list[self.index].meta;
        Some(Ok(match path {
            "plugin.name" => Value::str(&meta.name),
            "plugin.id" => Value::str(&meta.id),
            "plugin.version" => Value::str(&meta.version),
            "plugin.author" => Value::str(&meta.author),
            "mouse.x" | "mouse.y" => match self.app.session.stage_cursor {
                Some(at) => Value::Num(f64::from(if path == "mouse.x" { at.x } else { at.y })),
                None => Value::Null,
            },
            _ => return None,
        }))
    }

    fn call(&mut self, it: &mut Interp, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let a = &args;
        Ok(match name {
            // ------------------------------------------------------ core
            "say!" => {
                let text = self.line(LineKind::Say, a);
                self.app.plugins.toast(&self.name, self.color, text);
                Value::Null
            }
            "print!" => {
                self.line(LineKind::Print, a);
                Value::Null
            }
            // ---------------------------------------------------- config
            name if name.starts_with("config.") && name.ends_with('=') => {
                let key = &name["config.".len()..name.len() - 1];
                self.set_config(key, a.first().cloned().unwrap_or_default())?;
                Value::Null
            }
            "config.reset!" => {
                self.app.settings.plugins.values.remove(&self.id);
                self.app
                    .plugins
                    .settings_dirty
                    .get_or_insert(std::time::Instant::now());
                Value::Null
            }
            "config.all" => {
                let keys = self.app.plugins.list[self.index]
                    .settings
                    .iter()
                    .filter(|decl| decl.kind.has_key())
                    .map(|decl| decl.key.clone())
                    .collect::<Vec<_>>();
                Value::map(keys.into_iter().filter_map(|key| {
                    let value = self.config_value(&key)?;
                    Some((key, value))
                }))
            }
            // ---------------------------------------------------- plugin
            "plugin.stop!" => {
                self.stop_requested = true;
                Value::Null
            }
            "plugin.open!" | "plugin.close!" => {
                let title = text(a, 0, "the window title")?;
                let key = (self.id.clone(), title);
                if name == "plugin.open!" {
                    self.app.plugins.open_windows.insert(key);
                } else {
                    self.app.plugins.open_windows.remove(&key);
                }
                Value::Null
            }
            "plugin.tool!" => {
                match a.first() {
                    None | Some(Value::Null) => {
                        if self
                            .app
                            .plugins
                            .active_tool
                            .as_ref()
                            .is_some_and(|(owner, _)| *owner == self.id)
                        {
                            self.app.plugins.active_tool = None;
                        }
                    }
                    _ => {
                        self.app.plugins.active_tool =
                            Some((self.id.clone(), text(a, 0, "the tool name")?));
                    }
                }
                Value::Null
            }
            "plugin.blob" => {
                let wanted = text(a, 0, "the blob name")?;
                self.app.plugins.list[self.index]
                    .doc
                    .as_ref()
                    .and_then(|doc| doc.blob(&wanted))
                    .and_then(|blob| blob.bytes())
                    .map_or(Value::Null, |bytes| {
                        Value::str(String::from_utf8_lossy(&bytes))
                    })
            }
            // ---------------------------------------------------- editor
            "editor.frame" => Value::Num(f64::from(self.app.session.current_frame) + 1.0),
            "editor.goto!" => {
                let frame =
                    (num(a, 0, "goto!")?.round() as i64 - 1).clamp(0, i64::from(u16::MAX)) as u16;
                self.app.go_to_frame(frame);
                Value::Null
            }
            "editor.frames" => {
                let q0rg_id = self.app.session.current_q0rg_id;
                Value::Num(f64::from(self.q0rg(q0rg_id)?.frame_count))
            }
            "editor.play!" => {
                if !self.app.session.playing {
                    self.app.queue(crate::app::Action::TogglePlay);
                }
                Value::Null
            }
            "editor.stop!" => {
                if self.app.session.playing {
                    self.app.queue(crate::app::Action::TogglePlay);
                }
                Value::Null
            }
            "editor.playing" => Value::Bool(self.app.session.playing),
            "editor.tool" => match &self.app.plugins.active_tool {
                Some((_, tool)) => Value::str(tool),
                None => Value::str(self.app.session.current_tool.label().to_lowercase()),
            },
            "editor.tool!" => {
                let wanted = text(a, 0, "tool!")?;
                let tool = tool_by_name(&wanted).ok_or_else(|| {
                    let names = crate::state::Tool::ALL
                        .iter()
                        .map(|tool| tool.label().to_lowercase())
                        .collect::<Vec<_>>();
                    format!(
                        "there is no tool `{wanted}`; there are {}",
                        names.join(", ")
                    )
                })?;
                self.app.plugins.active_tool = None;
                self.app.queue(crate::app::Action::SelectTool(tool));
                Value::Null
            }
            "editor.zoom" => Value::Num(f64::from(self.app.session.viewport.zoom)),
            "editor.zoom!" => {
                self.app.session.viewport.zoom = num(a, 0, "zoom!")?.clamp(0.02, 64.0) as f32;
                Value::Null
            }
            "editor.status!" => {
                self.app.session.status =
                    a.iter().map(Value::to_text).collect::<Vec<_>>().join(" ");
                Value::Null
            }
            "editor.action!" => {
                let wanted = text(a, 0, "action!")?;
                let action = crate::app::Action::from_plugin_name(&wanted).ok_or_else(|| {
                    format!("there is no action `{wanted}`; editor.actions() lists them")
                })?;
                self.app.queue(action);
                Value::Null
            }
            "editor.actions" => Value::list(
                crate::app::PLUGIN_ACTIONS
                    .iter()
                    .map(|name| Value::str(*name))
                    .collect(),
            ),
            "editor.version" => Value::str(env!("CARGO_PKG_VERSION")),
            "editor.color" => color_value(self.app.session.stroke_color),
            "editor.color!" => {
                self.app.session.stroke_color =
                    color_arg(a, 0, "color!", self.app.session.stroke_color)?;
                Value::Null
            }
            "editor.fill" => self.app.session.fill_color.map_or(Value::Null, color_value),
            "editor.fill!" => {
                self.app.session.fill_color = match a.first() {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(color_of(value, "fill!")?),
                };
                Value::Null
            }
            "editor.width" => Value::Num(f64::from(self.app.session.stroke_width)),
            "editor.width!" => {
                self.app.session.stroke_width = num(a, 0, "width!")?.clamp(0.1, 500.0) as f32;
                Value::Null
            }
            "editor.mouse" => match self.app.session.stage_cursor {
                Some(at) => point_value(at.x, at.y),
                None => Value::Null,
            },
            "editor.screen" => {
                let screen = self.ctx.screen_rect();
                point_value(screen.width(), screen.height())
            }
            "editor.dark" => Value::Bool(self.ctx.style().visuals.dark_mode),
            "editor.accent" => {
                let accent = self.app.settings.theme.accent.to_color32();
                Value::str(q0ex::stdlib::color_text([
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    255,
                ]))
            }
            // ------------------------------------------------------ keys
            "keys.down" | "keys.pressed" => {
                let wanted = text(a, 0, name)?;
                let key = egui::Key::from_name(&wanted)
                    .or_else(|| egui::Key::from_name(&wanted.to_uppercase()))
                    .ok_or_else(|| format!("`{wanted}` is not a key name"))?;
                let pressed = name == "keys.pressed";
                Value::Bool(self.ctx.input(|input| {
                    if pressed {
                        input.key_pressed(key)
                    } else {
                        input.key_down(key)
                    }
                }))
            }
            "keys.shift" => Value::Bool(self.ctx.input(|input| input.modifiers.shift)),
            "keys.ctrl" => Value::Bool(self.ctx.input(|input| input.modifiers.command)),
            "keys.alt" => Value::Bool(self.ctx.input(|input| input.modifiers.alt)),
            // --------------------------------------------------- project
            "project.name" => Value::str(&self.app.state.project.meta.name),
            "project.path" => self
                .app
                .state
                .file_path
                .as_ref()
                .map_or(Value::Null, |path| Value::str(path.display().to_string())),
            "project.width" => Value::Num(f64::from(self.app.state.project.meta.stage_width)),
            "project.height" => Value::Num(f64::from(self.app.state.project.meta.stage_height)),
            "project.fps" => Value::Num(f64::from(self.app.state.project.meta.fps)),
            "project.fps!" => {
                let fps = num(a, 0, "fps!")?.round().clamp(1.0, 240.0) as u16;
                self.touch();
                self.app.state.project.meta.fps = fps;
                Value::Null
            }
            "project.size!" => {
                let width = num(a, 0, "size!")?.round().clamp(1.0, 16384.0) as u16;
                let height = num(a, 1, "size!")?.round().clamp(1.0, 16384.0) as u16;
                self.touch_artwork();
                self.app.state.project.meta.stage_width = width;
                self.app.state.project.meta.stage_height = height;
                Value::Null
            }
            "project.background" => color_value(self.app.state.project.meta.background_color),
            "project.background!" => {
                let color = color_arg(
                    a,
                    0,
                    "background!",
                    self.app.state.project.meta.background_color,
                )?;
                self.touch();
                self.app.state.project.meta.background_color = color;
                Value::Null
            }
            "project.dirty" => Value::Bool(self.app.state.dirty),
            "project.save!" => {
                self.app.queue(crate::app::Action::SaveProject);
                Value::Null
            }
            "project.timeline" => timeline_handle(self.app.session.current_q0rg_id),
            "project.main" => timeline_handle(self.app.state.project.meta.entry_q0rg_id),
            "project.timelines" => Value::list(
                self.app
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .map(|q0rg| timeline_handle(q0rg.q0rg_id))
                    .collect(),
            ),
            "project.layers" => {
                let q0rg_id = self.app.session.current_q0rg_id;
                Value::list(
                    self.q0rg(q0rg_id)?
                        .layers
                        .iter()
                        .map(|layer| layer_handle(q0rg_id, layer.layer_id))
                        .collect(),
                )
            }
            "project.layer" => {
                let q0rg_id = self.app.session.current_q0rg_id;
                match a.first() {
                    None | Some(Value::Null) => {
                        layer_handle(q0rg_id, self.app.session.current_layer_id)
                    }
                    Some(Value::Str(wanted)) => self
                        .q0rg(q0rg_id)?
                        .layers
                        .iter()
                        .find(|layer| layer.name == **wanted)
                        .map_or(Value::Null, |layer| layer_handle(q0rg_id, layer.layer_id)),
                    Some(Value::Handle(handle)) if handle.kind == "layer" => {
                        let (q0rg_id, layer_id) = self.layer_of(handle)?;
                        if q0rg_id != self.app.session.current_q0rg_id {
                            return Err("that layer is on another timeline".into());
                        }
                        self.app.session.current_layer_id = layer_id;
                        layer_handle(q0rg_id, layer_id)
                    }
                    Some(other) => {
                        return Err(format!(
                            "layer takes a name or a layer, not {}",
                            other.type_name()
                        ))
                    }
                }
            }
            "project.add_layer!" => {
                let wanted = match a.first() {
                    None | Some(Value::Null) => None,
                    _ => Some(text(a, 0, "add_layer!")?),
                };
                self.app
                    .handle(&self.ctx.clone(), crate::app::Action::AddLayer);
                let q0rg_id = self.app.session.current_q0rg_id;
                let layer_id = self.app.session.current_layer_id;
                if let Some(wanted) = wanted {
                    self.touch();
                    self.layer_mut(q0rg_id, layer_id)?.name = wanted;
                }
                layer_handle(q0rg_id, layer_id)
            }
            "project.objects" => {
                let q0rg_id = self.app.session.current_q0rg_id;
                let frame = self.frame_arg(a, 0)?;
                let layers = self
                    .q0rg(q0rg_id)?
                    .layers
                    .iter()
                    .map(|layer| layer.layer_id)
                    .collect::<Vec<_>>();
                let mut all = Vec::new();
                for layer_id in layers {
                    all.extend(self.objects_on(q0rg_id, layer_id, frame)?);
                }
                Value::list(all)
            }
            "project.selection" => Value::list(
                self.selection_refs()
                    .into_iter()
                    .map(|r| object_handle(r.q0rg_id, r.layer_id, r.placement_idx))
                    .collect(),
            ),
            "project.select!" => {
                let mut refs = Vec::new();
                let mut push = |value: &Value| -> Result<(), String> {
                    match value {
                        Value::Handle(handle) if handle.kind == "object" => {
                            refs.push(PlacementRef {
                                q0rg_id: id16(handle, 0)?,
                                layer_id: id16(handle, 1)?,
                                placement_idx: usize::try_from(
                                    handle.ids.get(2).copied().unwrap_or(-1),
                                )
                                .map_err(|_| "that object is gone".to_string())?,
                            });
                            Ok(())
                        }
                        Value::Null => Ok(()),
                        other => Err(format!("select! takes objects, not {}", other.type_name())),
                    }
                };
                for value in a {
                    match value {
                        Value::List(items) => {
                            for item in items.borrow().iter() {
                                push(item)?;
                            }
                        }
                        other => push(other)?,
                    }
                }
                for r in &refs {
                    self.placement(&Handle {
                        kind: "object".into(),
                        ids: vec![
                            i64::from(r.q0rg_id),
                            i64::from(r.layer_id),
                            r.placement_idx as i64,
                        ],
                    })?;
                }
                self.app.session.selection = match refs.len() {
                    0 => Selection::None,
                    1 => Selection::Placement {
                        q0rg_id: refs[0].q0rg_id,
                        layer_id: refs[0].layer_id,
                        placement_idx: refs[0].placement_idx,
                    },
                    _ => Selection::Multi(refs),
                };
                Value::Null
            }
            "project.deselect!" => {
                self.app.session.selection = Selection::None;
                Value::Null
            }
            "project.assets" => Value::list(
                self.app
                    .state
                    .project
                    .assets
                    .iter()
                    .map(|asset| asset_handle(asset.id()))
                    .collect(),
            ),
            "project.asset" => match a.first() {
                Some(Value::Num(id)) => {
                    let id = *id as u16;
                    self.asset(id).map_or(Value::Null, |_| asset_handle(id))
                }
                Some(Value::Str(wanted)) => self
                    .app
                    .state
                    .project
                    .asset_names
                    .iter()
                    .find(|(_, name)| **name == **wanted)
                    .map_or(Value::Null, |(id, _)| asset_handle(*id)),
                _ => return Err("asset takes an id or a name".into()),
            },
            "project.line!" => {
                let (x1, y1, x2, y2) = (
                    num(a, 0, "line!")? as f32,
                    num(a, 1, "line!")? as f32,
                    num(a, 2, "line!")? as f32,
                    num(a, 3, "line!")? as f32,
                );
                let stroke = self.stroke_style(a, 4, 5)?;
                let anchor = |x: f32, y: f32| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                };
                self.add_drawing(
                    VPath {
                        anchors: vec![anchor(x1, y1), anchor(x2, y2)],
                        closed: false,
                    },
                    None,
                    Some(stroke),
                )?
            }
            "project.path!" => {
                let points = points_arg(a, 0, "path!")?;
                if points.len() < 2 {
                    return Err("path! needs at least two points".into());
                }
                let closed = a.get(1).is_some_and(Value::truthy);
                let fill = match a.get(2) {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(color_of(value, "path! fill")?),
                };
                let stroke = self.stroke_style(a, 3, 4)?;
                self.add_drawing(
                    VPath {
                        anchors: points
                            .into_iter()
                            .map(|point| Anchor {
                                point,
                                in_handle: None,
                                out_handle: None,
                            })
                            .collect(),
                        closed,
                    },
                    fill.filter(|_| closed),
                    Some(stroke),
                )?
            }
            "project.rect!" | "project.circle!" => {
                let circle = name == "project.circle!";
                let (path, color_at) = if circle {
                    let (cx, cy, r) = (
                        num(a, 0, name)? as f32,
                        num(a, 1, name)? as f32,
                        num(a, 2, name)?.abs() as f32,
                    );
                    let k = 0.552_284_8 * r;
                    let anchor = |px: f32, py: f32, ix: f32, iy: f32, ox: f32, oy: f32| Anchor {
                        point: Vec2::new(px, py),
                        in_handle: Some(Vec2::new(ix, iy)),
                        out_handle: Some(Vec2::new(ox, oy)),
                    };
                    (
                        VPath {
                            anchors: vec![
                                anchor(cx, cy - r, cx - k, cy - r, cx + k, cy - r),
                                anchor(cx + r, cy, cx + r, cy - k, cx + r, cy + k),
                                anchor(cx, cy + r, cx + k, cy + r, cx - k, cy + r),
                                anchor(cx - r, cy, cx - r, cy + k, cx - r, cy - k),
                            ],
                            closed: true,
                        },
                        3,
                    )
                } else {
                    let (x, y, w, h) = (
                        num(a, 0, name)? as f32,
                        num(a, 1, name)? as f32,
                        num(a, 2, name)? as f32,
                        num(a, 3, name)? as f32,
                    );
                    let (x0, x1) = (x.min(x + w), x.max(x + w));
                    let (y0, y1) = (y.min(y + h), y.max(y + h));
                    let anchor = |px: f32, py: f32| Anchor {
                        point: Vec2::new(px, py),
                        in_handle: None,
                        out_handle: None,
                    };
                    (
                        VPath {
                            anchors: vec![
                                anchor(x0, y0),
                                anchor(x1, y0),
                                anchor(x1, y1),
                                anchor(x0, y1),
                            ],
                            closed: true,
                        },
                        4,
                    )
                };
                let fill = match a.get(color_at) {
                    None => self.app.session.fill_color,
                    Some(Value::Null) => None,
                    Some(value) => Some(color_of(value, "the fill")?),
                };
                let stroke = match a.get(color_at + 1) {
                    Some(Value::Null) => None,
                    _ => Some(self.stroke_style(a, color_at + 1, color_at + 2)?),
                };
                self.add_drawing(path, fill, stroke)?
            }
            "project.raw" => to_json(&self.app.state.project)?,
            "project.raw!" => {
                let value = a.first().cloned().unwrap_or_default();
                let replacement = from_json(&value, "project.raw!")?;
                self.touch_artwork();
                self.app.state.project = replacement;
                self.app.session.selection = Selection::None;
                Value::Null
            }
            "project.runtime" => to_json(&self.app.state.project.runtime)?,
            "project.runtime!" => {
                let value = a.first().cloned().unwrap_or_default();
                let replacement = from_json(&value, "project.runtime!")?;
                self.touch_artwork();
                self.app.state.project.runtime = replacement;
                Value::Null
            }
            // ------------------------------------------------------- ui
            "ui.text!" | "ui.heading!" | "ui.small!" | "ui.code!" => {
                let line = a.iter().map(Value::to_text).collect::<Vec<_>>().join(" ");
                let ui = self.ui()?;
                let text = egui::RichText::new(line);
                ui.label(match name {
                    "ui.heading!" => text.strong().size(16.0),
                    "ui.small!" => text.small().color(ui.visuals().weak_text_color()),
                    "ui.code!" => text.monospace(),
                    _ => text,
                });
                Value::Null
            }
            "ui.colored!" => {
                let color = color32(color_arg(
                    a,
                    0,
                    "colored!",
                    Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    },
                )?);
                let line = a
                    .iter()
                    .skip(1)
                    .map(Value::to_text)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.ui()?.colored_label(color, line);
                Value::Null
            }
            "ui.page!" => {
                let markup = text(a, 0, "page!")?;
                let ui = self.ui()?;
                let doc = self.app.plugins.list[self.index].doc.clone();
                super::page::show(ui, &markup, doc.as_ref(), &self.id);
                Value::Null
            }
            "ui.button" | "ui.link" => {
                let label = text(a, 0, name)?;
                let ui = self.ui()?;
                let response = if name == "ui.link" {
                    ui.link(label)
                } else {
                    ui.button(label)
                };
                Value::Bool(response.clicked())
            }
            "ui.slider" => {
                let label = text(a, 0, "slider")?;
                let mut value = num(a, 1, "slider")?;
                let (low, high) = (num_or(a, 2, "slider", 0.0)?, num_or(a, 3, "slider", 100.0)?);
                let ui = self.ui()?;
                ui.add(egui::Slider::new(&mut value, low.min(high)..=high.max(low)).text(label));
                Value::Num(value)
            }
            "ui.number" => {
                let label = text(a, 0, "number")?;
                let mut value = num(a, 1, "number")?;
                let ui = self.ui()?;
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(&mut value).speed(0.1));
                });
                Value::Num(value)
            }
            "ui.toggle" => {
                let label = text(a, 0, "toggle")?;
                let mut value = a.get(1).is_some_and(Value::truthy);
                self.ui()?.checkbox(&mut value, label);
                Value::Bool(value)
            }
            "ui.input" => {
                let label = text(a, 0, "input")?;
                let mut value = match a.get(1) {
                    None | Some(Value::Null) => String::new(),
                    Some(value) => value.to_text(),
                };
                let ui = self.ui()?;
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.text_edit_singleline(&mut value);
                });
                Value::str(value)
            }
            "ui.color" => {
                let label = text(a, 0, "color")?;
                let current = color_arg(
                    a,
                    1,
                    "color",
                    Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    },
                )?;
                let mut color = color32(current);
                let ui = self.ui()?;
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.color_edit_button_srgba(&mut color);
                });
                let [r, g, b, alpha] = color.to_srgba_unmultiplied();
                Value::str(q0ex::stdlib::color_text([r, g, b, alpha]))
            }
            "ui.choice" => {
                let label = text(a, 0, "choice")?;
                let current = a.get(1).cloned().unwrap_or_default();
                let Some(Value::List(options)) = a.get(2) else {
                    return Err("choice needs its options as a list".into());
                };
                let options = options.borrow().clone();
                let mut picked = current.clone();
                let ui = self.ui()?;
                egui::ComboBox::from_label(label)
                    .selected_text(current.to_text())
                    .show_ui(ui, |ui| {
                        for option in &options {
                            if ui
                                .selectable_label(option.equals(&current), option.to_text())
                                .clicked()
                            {
                                picked = option.clone();
                            }
                        }
                    });
                picked
            }
            "ui.sep!" => {
                self.ui()?.separator();
                Value::Null
            }
            "ui.space!" => {
                let amount = num_or(a, 0, "space!", 8.0)? as f32;
                self.ui()?.add_space(amount.clamp(0.0, 400.0));
                Value::Null
            }
            "ui.progress!" => {
                let value = num(a, 0, "progress!")?.clamp(0.0, 1.0) as f32;
                self.ui()?
                    .add(egui::ProgressBar::new(value).show_percentage());
                Value::Null
            }
            "ui.row!" | "ui.column!" => {
                let func = a.first().cloned().unwrap_or_default();
                let ui = self.ui()?;
                let mut outcome = Ok(());
                if name == "ui.row!" {
                    ui.horizontal_wrapped(|child| outcome = self.nested(it, child, &func));
                } else {
                    ui.vertical(|child| outcome = self.nested(it, child, &func));
                }
                outcome?;
                Value::Null
            }
            "ui.group!" => {
                let title = text(a, 0, "group!")?;
                let func = a.get(1).cloned().unwrap_or_default();
                let ui = self.ui()?;
                let mut outcome = Ok(());
                egui::CollapsingHeader::new(title)
                    .id_source(("plugin-group", &self.id, ui.next_auto_id()))
                    .default_open(true)
                    .show(ui, |child| outcome = self.nested(it, child, &func));
                outcome?;
                Value::Null
            }
            "ui.image!" => {
                let wanted = text(a, 0, "image!")?;
                let width = num_or(a, 1, "image!", 0.0)? as f32;
                let doc = self.app.plugins.list[self.index].doc.clone();
                let texture = doc
                    .as_ref()
                    .and_then(|doc| doc.blob(&wanted))
                    .and_then(|blob| blob.bytes())
                    .and_then(|bytes| {
                        super::cached_texture(
                            &self.ctx,
                            &format!("plugin-{}-{wanted}", self.id),
                            &bytes,
                        )
                    })
                    .ok_or_else(|| format!("this plugin has no picture `{wanted}`"))?;
                let ui = self.ui()?;
                let size = texture.size_vec2();
                let width = if width > 0.0 {
                    width
                } else {
                    size.x.min(ui.available_width())
                };
                ui.image((
                    texture.id(),
                    egui::vec2(width, width * size.y / size.x.max(1.0)),
                ));
                Value::Null
            }
            // ----------------------------------------------------- draw
            "draw.line!" | "draw.circle!" | "draw.ring!" | "draw.rect!" | "draw.box!"
            | "draw.text!" | "draw.path!" | "draw.poly!" => {
                let (painter, view) = self.painter()?;
                let white = Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                };
                let at = |x: f64, y: f64| view.stage_to_screen(Vec2::new(x as f32, y as f32));
                let scale = view.effective_scale();
                match name {
                    "draw.line!" => {
                        let color = color32(color_arg(a, 4, "line!", white)?);
                        let width = num_or(a, 5, "line!", 2.0)? as f32;
                        painter.line_segment(
                            [
                                at(num(a, 0, name)?, num(a, 1, name)?),
                                at(num(a, 2, name)?, num(a, 3, name)?),
                            ],
                            egui::Stroke::new(width, color),
                        );
                    }
                    "draw.circle!" | "draw.ring!" => {
                        let center = at(num(a, 0, name)?, num(a, 1, name)?);
                        let radius = num(a, 2, name)? as f32 * scale;
                        let color = color32(color_arg(a, 3, name, white)?);
                        if name == "draw.circle!" {
                            painter.circle_filled(center, radius, color);
                        } else {
                            let width = num_or(a, 4, name, 2.0)? as f32;
                            painter.circle_stroke(center, radius, egui::Stroke::new(width, color));
                        }
                    }
                    "draw.rect!" | "draw.box!" => {
                        let (x, y) = (num(a, 0, name)?, num(a, 1, name)?);
                        let (w, h) = (num(a, 2, name)?, num(a, 3, name)?);
                        let rect = egui::Rect::from_two_pos(at(x, y), at(x + w, y + h));
                        let color = color32(color_arg(a, 4, name, white)?);
                        if name == "draw.rect!" {
                            painter.rect_filled(rect, 0.0, color);
                        } else {
                            let width = num_or(a, 5, name, 2.0)? as f32;
                            painter.rect_stroke(rect, 0.0, egui::Stroke::new(width, color));
                        }
                    }
                    "draw.text!" => {
                        let position = at(num(a, 0, name)?, num(a, 1, name)?);
                        let words = a.get(2).map(Value::to_text).unwrap_or_default();
                        let color = color32(color_arg(a, 3, name, white)?);
                        let size = num_or(a, 4, name, 14.0)? as f32;
                        painter.text(
                            position,
                            egui::Align2::LEFT_TOP,
                            words,
                            egui::FontId::proportional(size.clamp(4.0, 200.0)),
                            color,
                        );
                    }
                    _ => {
                        let points: Vec<Pos2> = points_arg(a, 0, name)?
                            .into_iter()
                            .map(|point| view.stage_to_screen(point))
                            .collect();
                        let color = color32(color_arg(a, 1, name, white)?);
                        if name == "draw.poly!" && points.len() >= 3 {
                            painter.add(egui::Shape::convex_polygon(
                                points,
                                color,
                                egui::Stroke::NONE,
                            ));
                        } else if points.len() >= 2 {
                            let width = num_or(a, 2, name, 2.0)? as f32;
                            painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
                        }
                    }
                }
                Value::Null
            }
            // ----------------------------------------------------- files
            "file.read" => {
                let path = text(a, 0, "read")?;
                std::fs::read_to_string(&path)
                    .map(Value::str)
                    .map_err(|error| format!("could not read {path}: {error}"))?
            }
            "file.write!" | "file.append!" => {
                let path = text(a, 0, name)?;
                let contents = a.get(1).map(Value::to_text).unwrap_or_default();
                let outcome = if name == "file.write!" {
                    std::fs::write(&path, contents)
                } else {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut file| file.write_all(contents.as_bytes()))
                };
                outcome.map_err(|error| format!("could not write {path}: {error}"))?;
                Value::Null
            }
            "file.exists" => Value::Bool(std::path::Path::new(&text(a, 0, "exists")?).exists()),
            "file.list" => {
                let dir = text(a, 0, "list")?;
                let mut names = std::fs::read_dir(&dir)
                    .map_err(|error| format!("could not list {dir}: {error}"))?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path().display().to_string())
                    .collect::<Vec<_>>();
                names.sort();
                Value::list(names.into_iter().map(Value::str).collect())
            }
            "file.delete!" => {
                let path = text(a, 0, "delete!")?;
                std::fs::remove_file(&path)
                    .map_err(|error| format!("could not delete {path}: {error}"))?;
                Value::Null
            }
            "file.folder!" => {
                let path = text(a, 0, "folder!")?;
                std::fs::create_dir_all(&path)
                    .map_err(|error| format!("could not make {path}: {error}"))?;
                Value::Null
            }
            "file.dir" => {
                let dir = super::plugin_dir()
                    .ok_or("there is no settings folder")?
                    .join("data")
                    .join(super::file_name_for(&self.id).trim_end_matches(".qx"));
                let _ = std::fs::create_dir_all(&dir);
                Value::str(dir.display().to_string())
            }
            "file.home" => {
                dirs::home_dir().map_or(Value::Null, |dir| Value::str(dir.display().to_string()))
            }
            // -------------------------------------------------- settings
            "settings.get" | "settings.set!" | "settings.all" => {
                let mut json =
                    serde_json::to_value(&self.app.settings).map_err(|error| error.to_string())?;
                if name == "settings.all" {
                    return Ok(Value::from_json(&json));
                }
                let path = text(a, 0, name)?;
                let slot = json_path(&mut json, &path)
                    .ok_or_else(|| format!("there is no setting `{path}`"))?;
                if name == "settings.get" {
                    return Ok(Value::from_json(slot));
                }
                *slot = a.get(1).cloned().unwrap_or_default().to_json();
                let settings: crate::settings::Settings =
                    serde_json::from_value(json).map_err(|error| {
                        format!("settings.set!: that value does not fit `{path}`: {error}")
                    })?;
                let theme_changed = settings.theme != self.app.settings.theme;
                self.app.settings = settings;
                if theme_changed {
                    self.app.settings.theme.apply(&self.ctx);
                }
                self.app
                    .plugins
                    .settings_dirty
                    .get_or_insert(std::time::Instant::now());
                Value::Null
            }
            // ----------------------------------------------------- clip
            "clip.copy!" => {
                let words = a.iter().map(Value::to_text).collect::<Vec<_>>().join(" ");
                self.ctx.output_mut(|output| output.copied_text = words);
                Value::Null
            }
            _ => return Err(format!("there is no function `{name}`")),
        })
    }

    fn get(&mut self, _: &mut Interp, handle: &Handle, field: &str) -> Result<Value, String> {
        match handle.kind.as_str() {
            "timeline" => {
                let q0rg = self.q0rg(id16(handle, 0)?)?;
                Ok(match field {
                    "id" => Value::Num(f64::from(q0rg.q0rg_id)),
                    "name" => Value::str(&q0rg.name),
                    "frames" => Value::Num(f64::from(q0rg.frame_count)),
                    "script" => Value::str(&q0rg.script),
                    "main" => Value::Bool(q0rg.q0rg_id == self.app.state.project.meta.entry_q0rg_id),
                    "raw" => to_json(q0rg)?,
                    _ => return Err(format!("a timeline has no `{field}`; it has id, name, frames, script, main and raw")),
                })
            }
            "layer" => {
                let (q0rg_id, layer_id) = self.layer_of(handle)?;
                let project = &self.app.state.project;
                let layer = self
                    .q0rg(q0rg_id)?
                    .layers
                    .iter()
                    .find(|layer| layer.layer_id == layer_id)
                    .ok_or("that layer is gone")?;
                let metadata = project.layer_metadata(q0rg_id, layer_id);
                Ok(match field {
                    "id" => Value::Num(f64::from(layer_id)),
                    "name" => Value::str(&layer.name),
                    "kind" => Value::str(match metadata.kind {
                        LayerKind::Normal => "normal",
                        LayerKind::Folder => "folder",
                        LayerKind::Mask => "mask",
                    }),
                    "visible" => Value::Bool(!metadata.hidden),
                    "locked" => Value::Bool(metadata.locked),
                    "keyframes" => Value::list(
                        layer
                            .keyframe_frames()
                            .into_iter()
                            .map(|frame| Value::Num(f64::from(frame) + 1.0))
                            .collect(),
                    ),
                    "timeline" => timeline_handle(q0rg_id),
                    "current" => Value::Bool(
                        q0rg_id == self.app.session.current_q0rg_id && layer_id == self.app.session.current_layer_id,
                    ),
                    "raw" => to_json(layer)?,
                    "metadata" => to_json(&metadata)?,
                    _ => {
                        return Err(format!(
                            "a layer has no `{field}`; it has id, name, kind, visible, locked, keyframes, timeline, current, raw and metadata"
                        ))
                    }
                })
            }
            "object" => {
                let placement = self.placement(handle)?;
                let transform = placement.transform;
                let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
                Ok(match field {
                    "x" => Value::Num(f64::from(transform.tx)),
                    "y" => Value::Num(f64::from(transform.ty)),
                    "rotation" => Value::Num(f64::from(transform.rotation.to_degrees())),
                    "scale_x" => Value::Num(f64::from(transform.sx)),
                    "scale_y" => Value::Num(f64::from(transform.sy)),
                    "skew_x" => Value::Num(f64::from(transform.skew_x.to_degrees())),
                    "skew_y" => Value::Num(f64::from(transform.skew_y.to_degrees())),
                    "alpha" => Value::Num(f64::from(placement.fx.opacity)),
                    "frame" => Value::Num(f64::from(placement.frame) + 1.0),
                    "kind" => Value::str(match placement.target {
                        Target::Q0rg(_) => "symbol",
                        Target::Asset(asset_id) => match self.asset(asset_id) {
                            Ok(Asset::Vector(_)) => "drawing",
                            Ok(Asset::Bitmap(_)) => "bitmap",
                            Ok(Asset::Q0v(_)) => "video",
                            Ok(Asset::Rig(_)) => "rig",
                            Ok(Asset::Model(_)) => "model",
                            Err(_) => "missing",
                        },
                    }),
                    "asset" => match placement.target {
                        Target::Asset(asset_id) => asset_handle(asset_id),
                        Target::Q0rg(_) => Value::Null,
                    },
                    "symbol" => match placement.target {
                        Target::Q0rg(id) => timeline_handle(id),
                        Target::Asset(_) => Value::Null,
                    },
                    "layer" => layer_handle(q0rg_id, layer_id),
                    "timeline" => timeline_handle(q0rg_id),
                    "id" => Value::Num(f64::from(placement.instance_id)),
                    "name" => {
                        let key = q0s_format::v2::InstanceKey {
                            q0rg_id,
                            instance_id: placement.instance_id,
                        };
                        self.app
                            .state
                            .project
                            .runtime
                            .instance_names
                            .get(&key)
                            .map_or(Value::str(""), Value::str)
                    }
                    "selected" => Value::Bool(self.selection_refs().contains(&PlacementRef {
                        q0rg_id,
                        layer_id,
                        placement_idx: handle.ids[2] as usize,
                    })),
                    "raw" => to_json(placement)?,
                    _ => {
                        return Err(format!(
                            "an object has no `{field}`; it has x, y, rotation, scale_x, scale_y, skew_x, skew_y, alpha, frame, kind, asset, symbol, layer, timeline, id, name, selected and raw"
                        ))
                    }
                })
            }
            "asset" => {
                let asset_id = id16(handle, 0)?;
                let asset = self.asset(asset_id)?;
                Ok(match field {
                    "id" => Value::Num(f64::from(asset_id)),
                    "name" => Value::str(
                        self.app
                            .state
                            .project
                            .asset_names
                            .get(&asset_id)
                            .cloned()
                            .unwrap_or_else(|| format!("Item {asset_id}")),
                    ),
                    "kind" => Value::str(match asset {
                        Asset::Vector(_) => "drawing",
                        Asset::Bitmap(_) => "bitmap",
                        Asset::Q0v(_) => "video",
                        Asset::Rig(_) => "rig",
                        Asset::Model(_) => "model",
                    }),
                    "width" => match asset {
                        Asset::Bitmap(bitmap) => Value::Num(f64::from(bitmap.width)),
                        _ => Value::Null,
                    },
                    "height" => match asset {
                        Asset::Bitmap(bitmap) => Value::Num(f64::from(bitmap.height)),
                        _ => Value::Null,
                    },
                    "fill" => match asset {
                        Asset::Vector(vector) => vector.fill.map_or(Value::Null, color_value),
                        _ => Value::Null,
                    },
                    "stroke" => match asset {
                        Asset::Vector(vector) => vector.stroke.map_or(Value::Null, |stroke| color_value(stroke.color)),
                        _ => Value::Null,
                    },
                    "width_of_line" => match asset {
                        Asset::Vector(vector) => vector.stroke.map_or(Value::Null, |stroke| Value::Num(f64::from(stroke.width))),
                        _ => Value::Null,
                    },
                    "raw" => to_json(asset)?,
                    _ => return Err(format!("a library item has no `{field}`; it has id, name, kind, width, height, fill, stroke, width_of_line and raw")),
                })
            }
            other => Err(format!("unknown kind of object `{other}`")),
        }
    }

    fn set(
        &mut self,
        _: &mut Interp,
        handle: &Handle,
        field: &str,
        value: Value,
    ) -> Result<(), String> {
        let number = |what: &str| -> Result<f32, String> {
            value
                .as_num()
                .filter(|n| n.is_finite())
                .map(|n| n as f32)
                .ok_or_else(|| format!("{what} takes a number, not {}", value.type_name()))
        };
        match handle.kind.as_str() {
            "timeline" => {
                let q0rg_id = id16(handle, 0)?;
                match field {
                    "name" => {
                        let name = value.to_text();
                        self.touch();
                        self.q0rg_mut(q0rg_id)?.name = name;
                    }
                    "frames" => {
                        let frames =
                            number("frames")?.round().clamp(1.0, f32::from(u16::MAX)) as u16;
                        self.touch_artwork();
                        self.q0rg_mut(q0rg_id)?.frame_count = frames;
                    }
                    "script" => {
                        let script = value.to_text();
                        self.touch();
                        self.q0rg_mut(q0rg_id)?.script = script;
                    }
                    "raw" => {
                        let mut replacement: Q0rg = from_json(&value, "timeline.raw")?;
                        replacement.q0rg_id = q0rg_id;
                        self.touch_artwork();
                        *self.q0rg_mut(q0rg_id)? = replacement;
                    }
                    _ => {
                        return Err(format!(
                        "a timeline's `{field}` cannot be set; name, frames, script and raw can"
                    ))
                    }
                }
            }
            "layer" => {
                let (q0rg_id, layer_id) = self.layer_of(handle)?;
                let key = LayerKey { q0rg_id, layer_id };
                match field {
                    "name" => {
                        let name = value.to_text();
                        self.touch();
                        self.layer_mut(q0rg_id, layer_id)?.name = name;
                    }
                    "visible" | "locked" => {
                        let on = value.truthy();
                        self.touch_artwork();
                        let metadata = self.app.state.project.layer_metadata.entry(key).or_default();
                        if field == "visible" {
                            metadata.hidden = !on;
                        } else {
                            metadata.locked = on;
                        }
                    }
                    "raw" => {
                        let mut replacement: q0s_format::v2::Layer = from_json(&value, "layer.raw")?;
                        replacement.layer_id = layer_id;
                        self.touch_artwork();
                        *self.layer_mut(q0rg_id, layer_id)? = replacement;
                    }
                    "metadata" => {
                        let replacement = from_json(&value, "layer.metadata")?;
                        self.touch_artwork();
                        self.app.state.project.layer_metadata.insert(key, replacement);
                    }
                    _ => return Err(format!("a layer's `{field}` cannot be set; name, visible, locked, raw and metadata can")),
                }
            }
            "object" => {
                if field == "name" {
                    let (q0rg_id, _) = (id16(handle, 0)?, id16(handle, 1)?);
                    let mut instance_id = self.placement(handle)?.instance_id;
                    self.touch();
                    if instance_id == 0 {
                        instance_id = next_instance_id(&self.app.state.project)
                            .ok_or("there are no free object ids left")?;
                        self.placement_mut(handle)?.instance_id = instance_id;
                    }
                    let key = q0s_format::v2::InstanceKey {
                        q0rg_id,
                        instance_id,
                    };
                    let name = value.to_text();
                    let names = &mut self.app.state.project.runtime.instance_names;
                    if name.is_empty() {
                        names.remove(&key);
                    } else {
                        names.insert(key, name);
                    }
                    return Ok(());
                }
                if field == "raw" {
                    let replacement: Placement = from_json(&value, "object.raw")?;
                    self.placement(handle)?;
                    self.touch_artwork();
                    *self.placement_mut(handle)? = replacement;
                    return Ok(());
                }
                let amount = number(field)?;
                self.placement(handle)?;
                self.touch_artwork();
                let placement = self.placement_mut(handle)?;
                let transform = &mut placement.transform;
                match field {
                    "x" => transform.tx = amount,
                    "y" => transform.ty = amount,
                    "rotation" => transform.rotation = amount.to_radians(),
                    "scale_x" => transform.sx = amount,
                    "scale_y" => transform.sy = amount,
                    "skew_x" => transform.skew_x = amount.to_radians(),
                    "skew_y" => transform.skew_y = amount.to_radians(),
                    "alpha" => placement.fx.opacity = amount.clamp(0.0, 1.0),
                    _ => {
                        return Err(format!(
                            "an object's `{field}` cannot be set; x, y, rotation, scale_x, scale_y, skew_x, skew_y, alpha, name and raw can"
                        ))
                    }
                }
            }
            "asset" => {
                let asset_id = id16(handle, 0)?;
                self.asset(asset_id)?;
                match field {
                    "name" => {
                        let name = value.to_text();
                        self.touch();
                        self.app.state.project.asset_names.insert(asset_id, name);
                    }
                    "fill" | "stroke" | "width_of_line" => {
                        let fill = match &value {
                            Value::Null => None,
                            other if field != "width_of_line" => Some(color_of(other, field)?),
                            _ => None,
                        };
                        let width = value.as_num();
                        let default_stroke = VStroke {
                            color: self.app.session.stroke_color,
                            width: self.app.session.stroke_width,
                            cap: CapShape::Round,
                            join: JoinShape::Round,
                        };
                        self.touch_artwork();
                        let Some(Asset::Vector(vector)) =
                            self.app.state.project.assets.iter_mut().find(|asset| asset.id() == asset_id)
                        else {
                            return Err("only drawings have fills and lines".into());
                        };
                        match field {
                            "fill" => vector.fill = fill,
                            "stroke" => match fill {
                                Some(color) => vector.stroke.get_or_insert(default_stroke).color = color,
                                None => vector.stroke = None,
                            },
                            _ => {
                                let width = width.ok_or("width_of_line takes a number")? as f32;
                                vector.stroke.get_or_insert(default_stroke).width = width.max(0.1);
                            }
                        }
                    }
                    "raw" => {
                        let replacement: Asset = from_json(&value, "asset.raw")?;
                        if replacement.id() != asset_id {
                            return Err("asset.raw: the asset id must stay the same".into());
                        }
                        self.touch_artwork();
                        if let Some(asset) = self.app.state.project.assets.iter_mut().find(|asset| asset.id() == asset_id) {
                            *asset = replacement;
                        }
                    }
                    _ => return Err(format!("a library item's `{field}` cannot be set; name, fill, stroke, width_of_line and raw can")),
                }
            }
            other => return Err(format!("unknown kind of object `{other}`")),
        }
        Ok(())
    }

    fn method(
        &mut self,
        _: &mut Interp,
        handle: &Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        match (handle.kind.as_str(), name) {
            ("timeline", "layers") => {
                let q0rg_id = id16(handle, 0)?;
                Ok(Value::list(
                    self.q0rg(q0rg_id)?
                        .layers
                        .iter()
                        .map(|layer| layer_handle(q0rg_id, layer.layer_id))
                        .collect(),
                ))
            }
            ("timeline", "enter!") => {
                let q0rg_id = id16(handle, 0)?;
                self.q0rg(q0rg_id)?;
                self.app.queue(crate::app::Action::EnterQ0rg(q0rg_id));
                Ok(Value::Null)
            }
            ("layer", "objects") => {
                let (q0rg_id, layer_id) = self.layer_of(handle)?;
                let frame = self.frame_arg(&args, 0)?;
                Ok(Value::list(self.objects_on(q0rg_id, layer_id, frame)?))
            }
            ("layer", "select!") => {
                let (q0rg_id, layer_id) = self.layer_of(handle)?;
                if q0rg_id != self.app.session.current_q0rg_id {
                    return Err("that layer is on another timeline".into());
                }
                self.app.session.current_layer_id = layer_id;
                Ok(Value::Null)
            }
            ("layer", "remove!") => {
                let (q0rg_id, layer_id) = self.layer_of(handle)?;
                self.app
                    .queue(crate::app::Action::DeleteLayer(q0rg_id, layer_id));
                Ok(Value::Null)
            }
            ("object", "remove!") => {
                self.placement(handle)?;
                let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
                let index = handle.ids[2] as usize;
                self.touch_artwork();
                self.layer_mut(q0rg_id, layer_id)?.placements.remove(index);
                self.app.session.selection = Selection::None;
                Ok(Value::Null)
            }
            ("object", "copy!") => {
                let mut copy = self.placement(handle)?.clone();
                copy.instance_id = 0;
                let (q0rg_id, layer_id) = (id16(handle, 0)?, id16(handle, 1)?);
                if let (Some(dx), Some(dy)) = (
                    args.first().and_then(Value::as_num),
                    args.get(1).and_then(Value::as_num),
                ) {
                    copy.transform.tx += dx as f32;
                    copy.transform.ty += dy as f32;
                }
                self.touch_artwork();
                let layer = self.layer_mut(q0rg_id, layer_id)?;
                layer.push_placement(copy);
                Ok(object_handle(q0rg_id, layer_id, layer.placements.len() - 1))
            }
            ("object", "move!") => {
                let dx = num(&args, 0, "move!")? as f32;
                let dy = num(&args, 1, "move!")? as f32;
                self.placement(handle)?;
                self.touch_artwork();
                let placement = self.placement_mut(handle)?;
                placement.transform.tx += dx;
                placement.transform.ty += dy;
                Ok(Value::Null)
            }
            ("object", "select!") => {
                self.placement(handle)?;
                self.app.session.selection = Selection::Placement {
                    q0rg_id: id16(handle, 0)?,
                    layer_id: id16(handle, 1)?,
                    placement_idx: handle.ids[2] as usize,
                };
                Ok(Value::Null)
            }
            (kind, _) => Err(format!("a {kind} cannot `{name}`")),
        }
    }
}
