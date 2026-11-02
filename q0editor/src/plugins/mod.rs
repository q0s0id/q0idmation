//! Plugins: `.qx` / `.q0ex` files written in q0ex that reach into every
//! part of the editor.
//!
//! A plugin file is installed by copying it into the plugin folder under
//! its id. Turning it on loads it into its own [`q0ex::Interp`]; the editor
//! then calls its `when` handlers as things happen: menu items, frames,
//! selection, keys, ticks, stage drawing and plugin tools, windows. Any
//! error stops the plugin and says so on its card, so a broken plugin can
//! never take the editor down with it.

pub(crate) mod api;
mod blocks;
mod highlight;
mod host;
mod manager;
mod page;
pub(crate) mod studio;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Painter, Response, Ui};
use q0ex::{Interp, Meta, PluginDoc, Value};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::render::StageView;

/// Steps a handler the person started (a menu item, a key) may take.
const BUDGET_ACTION: u64 = 5_000_000;
/// Steps a handler that runs every frame may take.
const BUDGET_FRAME: u64 = 400_000;
/// Console lines kept.
const CONSOLE_LINES: usize = 500;

/// What the editor remembers about plugins between runs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginPrefs {
    /// Ids of the plugins that are on.
    pub enabled: Vec<String>,
    /// Each plugin's settings, by plugin id and setting name.
    pub values: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    /// The person has read what plugins can do.
    pub warned: bool,
}

/// One installed plugin.
pub(crate) struct Plugin {
    pub path: PathBuf,
    pub source: String,
    pub doc: Option<PluginDoc>,
    pub meta: Meta,
    pub parse_error: Option<q0ex::ParseError>,
    pub runtime: Option<Interp>,
    /// Why it stopped, when it stopped on an error.
    pub error: Option<String>,
    pub thumbnail: Option<egui::TextureHandle>,
    thumbnail_tried: bool,
    /// The settings the code reads as `config.name`.
    pub settings: Vec<q0ex::SettingDecl>,
    /// Keep values as last saved, to skip writing what did not change.
    saved_keep: Option<serde_json::Value>,
    /// Fingerprint of the file, shown before installing.
    pub fingerprint: String,
}

impl Plugin {
    fn from_source(path: PathBuf, source: String) -> Self {
        let parsed = PluginDoc::parse(&source);
        let (doc, parse_error) = match parsed {
            Ok(doc) => (Some(doc), None),
            Err(error) => (None, Some(error)),
        };
        let meta = doc.as_ref().map(PluginDoc::meta).unwrap_or_else(|| Meta {
            name: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("plugin")
                .to_string(),
            id: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("plugin")
                .to_string(),
            ..Meta::default()
        });
        let settings = doc.as_ref().map(PluginDoc::settings).unwrap_or_default();
        let fingerprint = fingerprint(source.as_bytes());
        Self {
            path,
            source,
            doc,
            meta,
            parse_error,
            runtime: None,
            error: None,
            thumbnail: None,
            thumbnail_tried: false,
            settings,
            saved_keep: None,
            fingerprint,
        }
    }

    pub fn running(&self) -> bool {
        self.runtime.is_some()
    }

    pub fn accent(&self) -> egui::Color32 {
        q0ex::stdlib::parse_color(&self.meta.color)
            .map(|[r, g, b, _]| egui::Color32::from_rgb(r, g, b))
            .unwrap_or(egui::Color32::from_rgb(157, 78, 255))
    }

    /// Handlers of `event` with their quoted argument, while running.
    pub fn handlers(&self, event: &str) -> Vec<(usize, Option<String>)> {
        self.runtime
            .as_ref()
            .map(|runtime| {
                runtime
                    .handlers_for(event)
                    .map(|(index, handler)| (index, handler.arg.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn thumbnail(&mut self, ctx: &Context) -> Option<egui::TextureHandle> {
        if !self.thumbnail_tried {
            self.thumbnail_tried = true;
            let bytes = self.doc.as_ref().and_then(PluginDoc::thumbnail);
            self.thumbnail = bytes.and_then(|bytes| {
                texture_from_bytes(ctx, &format!("plugin-thumb-{}", self.meta.id), &bytes)
            });
        }
        self.thumbnail.clone()
    }
}

/// A short, readable fingerprint of a file: the same file always gives the
/// same words, so a plugin can be checked against where it was shared.
pub(crate) fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let text = format!("{hash:016x}");
    format!(
        "{}-{}-{}-{}",
        &text[0..4],
        &text[4..8],
        &text[8..12],
        &text[12..16]
    )
}

pub(crate) fn texture_from_bytes(
    ctx: &Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

/// A texture for `bytes`, decoded once and kept while the bytes stay the
/// same; pages and windows draw their pictures every frame.
pub(crate) fn cached_texture(
    ctx: &Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("plugin-texture", name, fingerprint(bytes)));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let texture = texture_from_bytes(ctx, name, bytes)?;
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    Some(texture)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind {
    Say,
    Print,
    Error,
    Info,
}

pub(crate) struct ConsoleLine {
    pub plugin: String,
    pub kind: LineKind,
    pub text: String,
}

struct Toast {
    plugin: String,
    color: egui::Color32,
    text: String,
    born: Instant,
}

/// A plugin about to be installed, waiting for the person to agree.
pub(crate) struct PendingInstall {
    pub plugin: Plugin,
    /// The installed plugin it would replace.
    pub replaces: Option<String>,
}

#[derive(Default)]
pub(crate) struct Plugins {
    pub list: Vec<Plugin>,
    loaded: bool,
    pub manager_open: bool,
    /// The plugin whose page is open in the manager.
    pub page: Option<String>,
    pub pending_install: Option<PendingInstall>,
    pub studio: studio::Studio,
    pub console: Vec<ConsoleLine>,
    toasts: Vec<Toast>,
    /// The plugin tool in use: plugin id and tool name.
    pub active_tool: Option<(String, String)>,
    /// Plugin windows that are open: plugin id and window title.
    pub open_windows: HashSet<(String, String)>,
    /// Things that happened for plugins to hear about next frame.
    pending_events: Vec<(String, Vec<Value>)>,
    last_tick: Option<Instant>,
    last_frame: Option<(u16, u16)>,
    last_selection: Option<Vec<crate::state::PlacementRef>>,
    last_document: Option<(Option<u64>, Option<PathBuf>)>,
    /// When settings changed by a plugin should be written.
    settings_dirty: Option<Instant>,
    last_keep_save: Option<Instant>,
    /// Where the pointer was for a plugin tool's hover.
    last_hover: Option<(f32, f32)>,
    /// The editor's context, for saving from places that have none.
    pub ctx: Option<Context>,
}

impl Plugins {
    pub fn log(&mut self, plugin: &str, kind: LineKind, text: impl Into<String>) {
        self.console.push(ConsoleLine {
            plugin: plugin.to_string(),
            kind,
            text: text.into(),
        });
        if self.console.len() > CONSOLE_LINES {
            let extra = self.console.len() - CONSOLE_LINES;
            self.console.drain(..extra);
        }
    }

    pub fn toast(&mut self, plugin: &str, color: egui::Color32, text: impl Into<String>) {
        self.toasts.push(Toast {
            plugin: plugin.to_string(),
            color,
            text: text.into(),
            born: Instant::now(),
        });
        if self.toasts.len() > 6 {
            self.toasts.remove(0);
        }
    }

    /// Lets plugins hear about something on the next frame.
    pub fn notify(&mut self, event: &str, args: Vec<Value>) {
        if self.list.iter().any(Plugin::running) {
            self.pending_events.push((event.to_string(), args));
        }
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.list.iter().position(|plugin| plugin.meta.id == id)
    }
}

/// Where installed plugins live.
pub(crate) fn plugin_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("q0editor").join("plugins"))
}

fn keep_path(plugin: &Plugin) -> PathBuf {
    plugin.path.with_extension("keep.json")
}

fn file_name_for(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.qx", safe.trim_matches('.'))
}

// ------------------------------------------------------------- lifecycle

/// Reads the plugin folder once, and turns on what was on last time.
fn load_all(app: &mut EditorApp, ctx: &Context) {
    app.plugins.loaded = true;
    let Some(dir) = plugin_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut entries = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| q0ex::is_plugin_path(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort();
    for path in entries {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let plugin = Plugin::from_source(path, source);
        if app.plugins.index_of(&plugin.meta.id).is_some() {
            continue;
        }
        app.plugins.list.push(plugin);
    }
    sort_list(&mut app.plugins);
    let enabled = app.settings.plugins.enabled.clone();
    for id in enabled {
        if let Some(index) = app.plugins.index_of(&id) {
            start(app, ctx, index);
        }
    }
}

fn sort_list(plugins: &mut Plugins) {
    plugins
        .list
        .sort_by_key(|plugin| plugin.meta.name.to_lowercase());
}

/// Reads a plugin file someone picked, and asks before installing it.
pub(crate) fn offer_install(app: &mut EditorApp, path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(source) => {
            let plugin = Plugin::from_source(path.to_path_buf(), source);
            let replaces = app
                .plugins
                .index_of(&plugin.meta.id)
                .map(|index| app.plugins.list[index].meta.version.clone());
            app.plugins.pending_install = Some(PendingInstall { plugin, replaces });
            app.plugins.manager_open = true;
        }
        Err(error) => {
            app.session.status = format!("could not read the plugin: {error}");
        }
    }
}

/// Copies an agreed plugin into the plugin folder and lists it.
pub(crate) fn install(app: &mut EditorApp, ctx: &Context, pending: PendingInstall) {
    let id = pending.plugin.meta.id.clone();
    let source = pending.plugin.source.clone();
    match save_plugin_file(app, ctx, &id, &source) {
        Ok(()) => {
            app.plugins.page = Some(id.clone());
            let name = pending.plugin.meta.name.clone();
            app.session.status = format!("installed the plugin {name}; turn it on from its page");
            app.plugins.log(&name, LineKind::Info, "installed");
        }
        Err(error) => app.session.status = format!("could not install the plugin: {error}"),
    }
}

/// Writes a plugin's text into the plugin folder, replacing the plugin with
/// the same id, and restarts it if it was on.
pub(crate) fn save_plugin_file(
    app: &mut EditorApp,
    ctx: &Context,
    id: &str,
    source: &str,
) -> std::io::Result<()> {
    let dir = plugin_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no settings folder"))?;
    std::fs::create_dir_all(&dir)?;
    let existing = app.plugins.index_of(id);
    let path = existing
        .map(|index| app.plugins.list[index].path.clone())
        .unwrap_or_else(|| dir.join(file_name_for(id)));
    crate::file_io::write_bytes_atomic(&path, source.as_bytes())?;
    let was_running = existing.is_some_and(|index| app.plugins.list[index].running());
    if let Some(index) = existing {
        stop(app, ctx, index, None);
        app.plugins.list.remove(index);
    }
    let plugin = Plugin::from_source(path, source.to_string());
    let new_id = plugin.meta.id.clone();
    app.plugins.list.push(plugin);
    sort_list(&mut app.plugins);
    if was_running {
        if let Some(index) = app.plugins.index_of(&new_id) {
            start(app, ctx, index);
        }
    }
    Ok(())
}

pub(crate) fn uninstall(app: &mut EditorApp, ctx: &Context, index: usize) {
    stop(app, ctx, index, None);
    let plugin = app.plugins.list.remove(index);
    let _ = std::fs::remove_file(&plugin.path);
    let _ = std::fs::remove_file(keep_path(&plugin));
    app.settings
        .plugins
        .enabled
        .retain(|id| *id != plugin.meta.id);
    app.settings.plugins.values.remove(&plugin.meta.id);
    app.settings.save();
    app.plugins.page = None;
    app.plugins
        .log(&plugin.meta.name, LineKind::Info, "removed");
}

/// Turns a plugin on and remembers that it is on.
pub(crate) fn enable(app: &mut EditorApp, ctx: &Context, index: usize) {
    let id = app.plugins.list[index].meta.id.clone();
    if !app.settings.plugins.enabled.contains(&id) {
        app.settings.plugins.enabled.push(id);
        app.settings.save();
    }
    start(app, ctx, index);
}

pub(crate) fn disable(app: &mut EditorApp, ctx: &Context, index: usize) {
    let id = app.plugins.list[index].meta.id.clone();
    app.settings.plugins.enabled.retain(|other| *other != id);
    app.settings.save();
    stop(app, ctx, index, None);
    app.plugins.list[index].error = None;
}

/// Loads a plugin's program and runs its top level and `when enabled`.
pub(crate) fn start(app: &mut EditorApp, ctx: &Context, index: usize) {
    if app.plugins.list[index].running() {
        return;
    }
    let plugin = &mut app.plugins.list[index];
    plugin.error = None;
    let Some(doc) = plugin.doc.clone() else {
        let message = plugin
            .parse_error
            .as_ref()
            .map_or("the file could not be read".into(), ToString::to_string);
        plugin.error = Some(message.clone());
        let name = plugin.meta.name.clone();
        app.plugins.log(&name, LineKind::Error, message);
        return;
    };
    let kept = std::fs::read_to_string(keep_path(plugin))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    plugin.saved_keep = kept.clone();
    let kept: HashMap<String, Value> = match kept {
        Some(serde_json::Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| (key.clone(), Value::from_json(value)))
            .collect(),
        _ => HashMap::new(),
    };
    let mut interp = Interp::new();
    let outcome = {
        let mut host = host::EditorHost::new(app, ctx, index);
        let outcome = interp.load(&doc.program, &kept, &mut host, BUDGET_ACTION);
        host.finish();
        outcome
    };
    match outcome {
        Ok(()) => {
            app.plugins.list[index].runtime = Some(interp);
            let name = app.plugins.list[index].meta.name.clone();
            app.plugins.log(&name, LineKind::Info, "on");
            dispatch_one(app, ctx, index, "enabled", vec![], BUDGET_ACTION);
        }
        Err(error) => fail(app, index, &error.to_string()),
    }
}

/// Turns a plugin off: `when disabled`, then its `keep` values are saved.
pub(crate) fn stop(app: &mut EditorApp, ctx: &Context, index: usize, reason: Option<String>) {
    if !app.plugins.list[index].running() {
        return;
    }
    if reason.is_none() {
        dispatch_one(app, ctx, index, "disabled", vec![], BUDGET_ACTION);
    }
    save_keep(app, index, true);
    let plugin = &mut app.plugins.list[index];
    plugin.runtime = None;
    let id = plugin.meta.id.clone();
    let name = plugin.meta.name.clone();
    if app
        .plugins
        .active_tool
        .as_ref()
        .is_some_and(|(owner, _)| *owner == id)
    {
        app.plugins.active_tool = None;
    }
    app.plugins.open_windows.retain(|(owner, _)| *owner != id);
    match reason {
        Some(reason) => {
            app.plugins.list[index].error = Some(reason.clone());
            app.plugins
                .log(&name, LineKind::Error, format!("stopped: {reason}"));
            let color = app.plugins.list[index].accent();
            app.plugins
                .toast(&name, color, format!("stopped: {reason}"));
        }
        None => app.plugins.log(&name, LineKind::Info, "off"),
    }
}

/// Stops a plugin after an error in its code.
fn fail(app: &mut EditorApp, index: usize, message: &str) {
    let plugin = &mut app.plugins.list[index];
    plugin.runtime = None;
    plugin.error = Some(message.to_string());
    let name = plugin.meta.name.clone();
    let color = plugin.accent();
    let id = plugin.meta.id.clone();
    if app
        .plugins
        .active_tool
        .as_ref()
        .is_some_and(|(owner, _)| *owner == id)
    {
        app.plugins.active_tool = None;
    }
    app.plugins.open_windows.retain(|(owner, _)| *owner != id);
    app.plugins
        .log(&name, LineKind::Error, format!("stopped: {message}"));
    app.plugins
        .toast(&name, color, format!("stopped: {message}"));
}

fn save_keep(app: &mut EditorApp, index: usize, force: bool) {
    let plugin = &mut app.plugins.list[index];
    let Some(runtime) = &plugin.runtime else {
        return;
    };
    if runtime.keep_names.is_empty() {
        return;
    }
    let values: serde_json::Map<String, serde_json::Value> = runtime
        .kept()
        .into_iter()
        .map(|(key, value)| (key, value.to_json()))
        .collect();
    let json = serde_json::Value::Object(values);
    if !force && plugin.saved_keep.as_ref() == Some(&json) {
        return;
    }
    if plugin.saved_keep.as_ref() != Some(&json) {
        let path = keep_path(plugin);
        if let Ok(bytes) = serde_json::to_vec_pretty(&json) {
            let _ = crate::file_io::write_bytes_atomic(&path, &bytes);
        }
        plugin.saved_keep = Some(json);
    }
}

/// Saves every running plugin's `keep` values; for leaving the editor.
pub(crate) fn shutdown(app: &mut EditorApp) {
    for index in 0..app.plugins.list.len() {
        save_keep(app, index, true);
    }
    if app.plugins.settings_dirty.take().is_some() {
        app.settings.save();
    }
}

// --------------------------------------------------------------- running

/// What a handler runs inside: a window's `ui`, or the stage's painter.
#[derive(Default)]
pub(crate) struct Scene<'a> {
    pub ui: Option<&'a mut Ui>,
    pub draw: Option<(&'a Painter, &'a StageView)>,
}

/// Runs one handler of plugin `index`; stops the plugin when it fails.
fn run_handler(
    app: &mut EditorApp,
    ctx: &Context,
    index: usize,
    handler: usize,
    args: Vec<Value>,
    budget: u64,
    scene: Scene<'_>,
) -> Option<Value> {
    let mut interp = app.plugins.list.get_mut(index)?.runtime.take()?;
    let id = app.plugins.list[index].meta.id.clone();
    let outcome = {
        let mut host = host::EditorHost::new(app, ctx, index);
        host.scene(scene);
        let outcome = interp.run_handler(handler, args, &mut host, budget);
        let stop_requested = host.stop_requested;
        host.finish();
        (outcome, stop_requested)
    };
    // The list may have changed if the plugin removed itself.
    let index = app.plugins.index_of(&id)?;
    match outcome {
        (Ok(value), false) => {
            app.plugins.list[index].runtime = Some(interp);
            Some(value)
        }
        (Ok(_), true) => {
            app.plugins.list[index].runtime = Some(interp);
            disable(app, ctx, index);
            None
        }
        (Err(error), _) => {
            fail(app, index, &error.to_string());
            None
        }
    }
}

/// Runs every handler of `event` in plugin `index`.
fn dispatch_one(
    app: &mut EditorApp,
    ctx: &Context,
    index: usize,
    event: &str,
    args: Vec<Value>,
    budget: u64,
) {
    for (handler, _) in app.plugins.list[index].handlers(event) {
        run_handler(
            app,
            ctx,
            index,
            handler,
            args.clone(),
            budget,
            Scene::default(),
        );
    }
}

/// Runs `event` in every running plugin.
fn dispatch(app: &mut EditorApp, ctx: &Context, event: &str, args: Vec<Value>, budget: u64) {
    for index in 0..app.plugins.list.len() {
        if index < app.plugins.list.len() && app.plugins.list[index].running() {
            dispatch_one(app, ctx, index, event, args.clone(), budget);
        }
    }
}

/// Runs the handler of `event` whose quoted argument is `arg`.
pub(crate) fn dispatch_arg(
    app: &mut EditorApp,
    ctx: &Context,
    id: &str,
    event: &str,
    arg: &str,
    args: Vec<Value>,
) {
    let Some(index) = app.plugins.index_of(id) else {
        return;
    };
    for (handler, handler_arg) in app.plugins.list[index].handlers(event) {
        if handler_arg.as_deref() == Some(arg) {
            run_handler(
                app,
                ctx,
                index,
                handler,
                args.clone(),
                BUDGET_ACTION,
                Scene::default(),
            );
        }
    }
}

fn any_running_with(app: &EditorApp, event: &str) -> bool {
    app.plugins.list.iter().any(|plugin| {
        plugin
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.has_handler(event))
    })
}

/// Parses a key chord like `ctrl+shift+k`.
fn parse_chord(text: &str) -> Option<(egui::Modifiers, egui::Key)> {
    let mut modifiers = egui::Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" => modifiers.command = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            other => {
                key = egui::Key::from_name(other)
                    .or_else(|| egui::Key::from_name(&other.to_uppercase()));
                key?;
            }
        }
    }
    Some((modifiers, key?))
}

/// Everything plugins do between frames: loading, timers, ticks and the
/// events that happened since the last frame.
pub(crate) fn update(app: &mut EditorApp, ctx: &Context) {
    if app.plugins.ctx.is_none() {
        app.plugins.ctx = Some(ctx.clone());
    }
    if !app.plugins.loaded {
        load_all(app, ctx);
    }
    let now = Instant::now();
    let dt = app
        .plugins
        .last_tick
        .map_or(0.0, |last| now.duration_since(last).as_secs_f64().min(0.25));
    app.plugins.last_tick = Some(now);

    // Timers, then ticks.
    for index in 0..app.plugins.list.len() {
        let Some(mut interp) = app
            .plugins
            .list
            .get_mut(index)
            .and_then(|plugin| plugin.runtime.take())
        else {
            continue;
        };
        let id = app.plugins.list[index].meta.id.clone();
        let errors = {
            let mut host = host::EditorHost::new(app, ctx, index);
            let errors = interp.advance(dt, &mut host, BUDGET_ACTION);
            host.finish();
            errors
        };
        let Some(index) = app.plugins.index_of(&id) else {
            continue;
        };
        app.plugins.list[index].runtime = Some(interp);
        if let Some(error) = errors.first() {
            fail(app, index, &error.to_string());
        }
    }
    if any_running_with(app, "tick") {
        dispatch(app, ctx, "tick", vec![Value::Num(dt)], BUDGET_FRAME);
    }

    // Changes since the last frame.
    let frame = (app.session.current_q0rg_id, app.session.current_frame);
    if app.plugins.last_frame != Some(frame) {
        let first = app.plugins.last_frame.is_none();
        app.plugins.last_frame = Some(frame);
        if !first && any_running_with(app, "frame") {
            dispatch(
                app,
                ctx,
                "frame",
                vec![Value::Num(f64::from(frame.1) + 1.0)],
                BUDGET_ACTION,
            );
        }
    }
    if any_running_with(app, "selection") {
        let selection =
            crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection);
        if app.plugins.last_selection.as_ref() != Some(&selection) {
            let first = app.plugins.last_selection.is_none();
            app.plugins.last_selection = Some(selection);
            if !first {
                dispatch(app, ctx, "selection", vec![], BUDGET_ACTION);
            }
        }
    }
    let document = (app.active_project_tab_id(), app.state.file_path.clone());
    if app.plugins.last_document.as_ref() != Some(&document) {
        let first = app.plugins.last_document.is_none();
        app.plugins.last_document = Some(document);
        if !first {
            app.plugins.notify("opened", vec![]);
        }
    }
    let events = std::mem::take(&mut app.plugins.pending_events);
    for (event, args) in events {
        dispatch(app, ctx, &event, args, BUDGET_ACTION);
    }

    // Keys, while no text field has them.
    if !ctx.wants_keyboard_input() && any_running_with(app, "key") {
        let mut hits = Vec::new();
        for (index, plugin) in app.plugins.list.iter().enumerate() {
            for (handler, arg) in plugin.handlers("key") {
                let Some((modifiers, key)) = arg.as_deref().and_then(parse_chord) else {
                    continue;
                };
                if ctx.input(|input| {
                    input.key_pressed(key) && input.modifiers.matches_logically(modifiers)
                }) {
                    hits.push((index, handler));
                }
            }
        }
        for (index, handler) in hits {
            run_handler(
                app,
                ctx,
                index,
                handler,
                vec![],
                BUDGET_ACTION,
                Scene::default(),
            );
        }
    }

    // Saving what plugins changed, now and then.
    if app
        .plugins
        .settings_dirty
        .is_some_and(|since| now.duration_since(since) > Duration::from_millis(800))
    {
        app.plugins.settings_dirty = None;
        app.settings.save();
    }
    if app
        .plugins
        .last_keep_save
        .is_none_or(|last| now.duration_since(last) > Duration::from_secs(3))
    {
        app.plugins.last_keep_save = Some(now);
        for index in 0..app.plugins.list.len() {
            save_keep(app, index, false);
        }
    }

    let busy = app.plugins.list.iter().any(|plugin| {
        plugin
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.has_timers() || runtime.has_handler("tick"))
    });
    if busy {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    if !app.plugins.toasts.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// Draws what running plugins draw on the stage (`when draw`).
pub(crate) fn paint_stage(app: &mut EditorApp, painter: &Painter, view: &StageView) {
    if !any_running_with(app, "draw") {
        return;
    }
    let ctx = painter.ctx().clone();
    for index in 0..app.plugins.list.len() {
        for (handler, _) in app
            .plugins
            .list
            .get(index)
            .map(|plugin| plugin.handlers("draw"))
            .unwrap_or_default()
        {
            run_handler(
                app,
                &ctx,
                index,
                handler,
                vec![],
                BUDGET_FRAME,
                Scene {
                    ui: None,
                    draw: Some((painter, view)),
                },
            );
        }
    }
}

/// Gives stage input to the plugin tool in use. True when it took it.
pub(crate) fn stage_input(app: &mut EditorApp, response: &Response, view: &StageView) -> bool {
    let Some((id, _)) = app.plugins.active_tool.clone() else {
        return false;
    };
    let Some(index) = app
        .plugins
        .index_of(&id)
        .filter(|index| app.plugins.list[*index].running())
    else {
        app.plugins.active_tool = None;
        return false;
    };
    let ctx = response.ctx.clone();
    if response.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    let Some(pointer) = pointer else {
        return true;
    };
    let stage = view.screen_to_stage(pointer);
    let args = vec![
        Value::Num(f64::from(stage.x)),
        Value::Num(f64::from(stage.y)),
    ];
    let primary = egui::PointerButton::Primary;
    let mut events: Vec<&str> = Vec::new();
    if response.drag_started_by(primary) {
        events.push("press");
    } else if response.dragged_by(primary) {
        events.push("drag");
    }
    if response.drag_stopped_by(primary) {
        events.push("release");
    }
    if response.clicked_by(primary) {
        events.push("press");
        events.push("release");
    }
    let position = (stage.x, stage.y);
    if response.hovered() && app.plugins.last_hover != Some(position) && events.is_empty() {
        app.plugins.last_hover = Some(position);
        events.push("hover");
    }
    for event in events {
        let budget = if matches!(event, "drag" | "hover") {
            BUDGET_FRAME
        } else {
            BUDGET_ACTION
        };
        let Some(index) = app.plugins.index_of(&id) else {
            break;
        };
        for (handler, _) in app.plugins.list[index].handlers(event) {
            run_handler(
                app,
                &ctx,
                index,
                handler,
                args.clone(),
                budget,
                Scene::default(),
            );
        }
    }
    let _ = index;
    true
}

/// Plugin windows, toasts, the plugin manager and the studio.
pub(crate) fn render(app: &mut EditorApp, ctx: &Context) {
    render_plugin_windows(app, ctx);
    manager::render(app, ctx);
    studio::render(app, ctx);
    render_toasts(app, ctx);
}

fn render_plugin_windows(app: &mut EditorApp, ctx: &Context) {
    let open = app.plugins.open_windows.iter().cloned().collect::<Vec<_>>();
    for (id, title) in open {
        let Some(index) = app
            .plugins
            .index_of(&id)
            .filter(|index| app.plugins.list[*index].running())
        else {
            app.plugins.open_windows.remove(&(id, title));
            continue;
        };
        let handlers = app.plugins.list[index]
            .handlers("window")
            .into_iter()
            .filter(|(_, arg)| arg.as_deref() == Some(title.as_str()))
            .map(|(handler, _)| handler)
            .collect::<Vec<_>>();
        if handlers.is_empty() {
            app.plugins.open_windows.remove(&(id, title));
            continue;
        }
        let name = app.plugins.list[index].meta.name.clone();
        let mut open = true;
        egui::Window::new(&title)
            .id(egui::Id::new(("plugin-window", &id, &title)))
            .open(&mut open)
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(&name)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                for handler in &handlers {
                    run_handler(
                        app,
                        ctx,
                        index,
                        *handler,
                        vec![],
                        BUDGET_FRAME,
                        Scene {
                            ui: Some(ui),
                            draw: None,
                        },
                    );
                }
            });
        if !open {
            app.plugins.open_windows.remove(&(id, title));
        }
    }
}

fn render_toasts(app: &mut EditorApp, ctx: &Context) {
    let now = Instant::now();
    app.plugins
        .toasts
        .retain(|toast| now.duration_since(toast.born) < Duration::from_millis(4200));
    if app.plugins.toasts.is_empty() {
        return;
    }
    let screen = ctx.screen_rect();
    let mut bottom = screen.bottom() - 54.0;
    let text_color = app.settings.theme.text.to_color32();
    let panel = app.settings.theme.panel.to_color32();
    for (index, toast) in app.plugins.toasts.iter().enumerate().rev() {
        let age = now.duration_since(toast.born).as_secs_f32();
        let enter = (age / 0.22).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - enter).powi(3);
        let leave = ((4.2 - age) / 0.4).clamp(0.0, 1.0);
        let alpha = (ease * leave * 255.0) as u8;
        let layer =
            egui::LayerId::new(egui::Order::Tooltip, egui::Id::new(("plugin-toast", index)));
        let painter = ctx.layer_painter(layer);
        let font = egui::FontId::proportional(13.0);
        let galley = painter.layout(toast.text.clone(), font, text_color, 300.0);
        let size = galley.size() + egui::vec2(28.0, 30.0);
        let slide = (1.0 - ease) * 40.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(screen.right() - size.x - 18.0 + slide, bottom - size.y),
            size,
        );
        let fade = |color: egui::Color32| {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
        };
        painter.rect_filled(
            rect.translate(egui::vec2(0.0, 3.0)),
            10.0,
            egui::Color32::from_black_alpha(alpha / 3),
        );
        painter.rect_filled(rect, 10.0, fade(panel));
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(4.0, rect.height())),
            egui::Rounding {
                nw: 10.0,
                sw: 10.0,
                ne: 0.0,
                se: 0.0,
            },
            fade(toast.color),
        );
        painter.text(
            rect.left_top() + egui::vec2(14.0, 6.0),
            egui::Align2::LEFT_TOP,
            &toast.plugin,
            egui::FontId::proportional(10.5),
            fade(toast.color),
        );
        painter.galley_with_override_text_color(
            rect.left_top() + egui::vec2(14.0, 21.0),
            galley,
            fade(text_color),
        );
        bottom = rect.top() - 8.0;
    }
}

/// The Plugins menu: managing plugins, and what running plugins add.
pub(crate) fn menu(app: &mut EditorApp, ui: &mut Ui) {
    if ui.button("Manage plugins...").clicked() {
        app.plugins.manager_open = true;
        ui.close_menu();
    }
    if ui.button("Plugin Studio...").clicked() {
        app.plugins.studio.open = true;
        ui.close_menu();
    }
    if ui.button("Add plugin from file...").clicked() {
        app.queue(crate::app::Action::InstallPlugin);
        ui.close_menu();
    }
    let ctx = ui.ctx().clone();
    let running = app
        .plugins
        .list
        .iter()
        .enumerate()
        .filter(|(_, plugin)| plugin.running())
        .map(|(index, plugin)| (index, plugin.meta.id.clone(), plugin.meta.name.clone()))
        .collect::<Vec<_>>();
    if running.is_empty() {
        return;
    }
    ui.separator();
    for (index, id, name) in running {
        let plugin = &app.plugins.list[index];
        let menus = plugin.handlers("menu");
        let windows = plugin.handlers("window");
        let tools = plugin.handlers("tool");
        if menus.is_empty() && windows.is_empty() && tools.is_empty() {
            continue;
        }
        ui.menu_button(&name, |ui| {
            for (_, label) in menus.iter() {
                let Some(label) = label else { continue };
                if ui.button(label).clicked() {
                    ui.close_menu();
                    dispatch_arg(app, &ctx, &id, "menu", label, vec![]);
                }
            }
            if !windows.is_empty() && !menus.is_empty() {
                ui.separator();
            }
            for (_, title) in windows.iter() {
                let Some(title) = title else { continue };
                let key = (id.clone(), title.clone());
                let mut open = app.plugins.open_windows.contains(&key);
                if ui.checkbox(&mut open, title).changed() {
                    if open {
                        app.plugins.open_windows.insert(key);
                    } else {
                        app.plugins.open_windows.remove(&key);
                    }
                }
            }
            if !tools.is_empty() {
                ui.separator();
            }
            for (_, tool) in tools.iter() {
                let Some(tool) = tool else { continue };
                let key = (id.clone(), tool.clone());
                let active = app.plugins.active_tool.as_ref() == Some(&key);
                if ui.radio(active, format!("{tool} tool")).clicked() {
                    ui.close_menu();
                    if active {
                        app.plugins.active_tool = None;
                    } else {
                        app.plugins.active_tool = Some(key);
                        dispatch_arg(app, &ctx, &id, "tool", tool, vec![]);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse() {
        let (modifiers, key) = parse_chord("ctrl+shift+k").unwrap();
        assert!(modifiers.command && modifiers.shift && !modifiers.alt);
        assert_eq!(key, egui::Key::K);
        assert_eq!(parse_chord("F5").map(|(_, key)| key), Some(egui::Key::F5));
        assert!(parse_chord("ctrl+banana").is_none());
    }

    #[test]
    fn fingerprints_are_stable_and_short() {
        assert_eq!(fingerprint(b"hello"), fingerprint(b"hello"));
        assert_ne!(fingerprint(b"hello"), fingerprint(b"hellp"));
        assert_eq!(fingerprint(b"x").len(), 19);
    }

    #[test]
    fn file_names_stay_safe() {
        assert_eq!(file_name_for("q0s.spin-it"), "q0s.spin-it.qx");
        assert_eq!(file_name_for("../evil/../x"), "_evil_.._x.qx");
    }

    use crate::app::{Action, NewProjectSpec};

    /// An editor with a new project and `source` running as a plugin, kept
    /// out of the real plugin folder.
    fn editor_with(source: &str) -> (EditorApp, Context, usize) {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        app.handle(
            &ctx,
            Action::CreateProject(NewProjectSpec {
                name: "Plugin test".into(),
                ..NewProjectSpec::default()
            }),
        );
        let dir = std::env::temp_dir().join(format!(
            "q0ex-test-{}-{}",
            std::process::id(),
            fingerprint(source.as_bytes())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        app.plugins.loaded = true;
        app.plugins
            .list
            .push(Plugin::from_source(dir.join("test.qx"), source.to_string()));
        let index = app.plugins.list.len() - 1;
        start(&mut app, &ctx, index);
        assert!(
            app.plugins.list[index].running(),
            "{:?}",
            app.plugins.list[index].error
        );
        (app, ctx, index)
    }

    fn said(app: &EditorApp) -> Vec<String> {
        app.plugins
            .console
            .iter()
            .filter(|line| line.kind == LineKind::Say)
            .map(|line| line.text.clone())
            .collect()
    }

    #[test]
    fn a_plugin_draws_moves_reads_settings_and_undoes_in_one_step() {
        let (mut app, ctx, index) = editor_with(
            r##"plugin "Test" {
  id = "t.test"
}
settings {
  slider size "Size" = 7 from 1 to 10
}
keep runs = 0
when enabled {
  runs += 1
}
when menu "Draw" {
  project.rect! 10, 20, 30, 40, "#ff0000"
  made = project.objects()
  say! len(made), config.size, runs
  for o in made {
    o.x += 5
    o.rotation = 90
  }
}
"##,
        );
        let before = app.state.project.assets.len();
        dispatch_arg(&mut app, &ctx, "t.test", "menu", "Draw", vec![]);
        assert!(
            app.plugins.list[index].running(),
            "{:?}",
            app.plugins.list[index].error
        );
        assert_eq!(app.state.project.assets.len(), before + 1);
        assert_eq!(said(&app), ["1 7 1"]);
        let q0rg_id = app.session.current_q0rg_id;
        let placement = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .flat_map(|layer| &layer.placements)
                    .last()
            })
            .cloned()
            .unwrap();
        assert_eq!(placement.transform.tx, 5.0);
        assert!((placement.transform.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        app.handle(&ctx, Action::Undo);
        assert_eq!(
            app.state.project.assets.len(),
            before,
            "one undo takes the whole run back"
        );
    }

    #[test]
    fn a_failing_plugin_stops_and_says_why() {
        let (mut app, ctx, index) = editor_with(
            "plugin \"Bad\" {\n  id = \"t.bad\"\n}\nwhen menu \"Typo\" {\n  x = nope + 1\n}\nwhen menu \"Loop\" {\n  while true {\n  }\n}\n",
        );
        dispatch_arg(&mut app, &ctx, "t.bad", "menu", "Typo", vec![]);
        assert!(!app.plugins.list[index].running());
        assert!(app.plugins.list[index]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("nope"));
        start(&mut app, &ctx, index);
        dispatch_arg(&mut app, &ctx, "t.bad", "menu", "Loop", vec![]);
        assert!(!app.plugins.list[index].running());
        assert!(app.plugins.list[index]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("too long"));
    }

    #[test]
    fn raw_data_reaches_what_the_api_has_no_word_for() {
        let (mut app, ctx, index) = editor_with(
            "plugin \"Raw\" {\n  id = \"t.raw\"\n}\nwhen menu \"Rename\" {\n  t = project.timeline()\n  data = t.raw\n  data.name = \"Renamed by raw\"\n  t.raw = data\n  layer = project.layer()\n  layer.name = \"Plugin layer\"\n  say! t.name, layer.name, len(editor.actions()) > 10\n}\n",
        );
        dispatch_arg(&mut app, &ctx, "t.raw", "menu", "Rename", vec![]);
        assert!(
            app.plugins.list[index].running(),
            "{:?}",
            app.plugins.list[index].error
        );
        assert_eq!(said(&app), ["Renamed by raw Plugin layer true"]);
    }

    #[test]
    fn the_example_plugins_run() {
        for (name, source) in studio::EXAMPLES {
            let (mut app, ctx, index) = editor_with(source);
            let id = app.plugins.list[index].meta.id.clone();
            for (_, label) in app.plugins.list[index].handlers("menu") {
                dispatch_arg(
                    &mut app,
                    &ctx,
                    &id,
                    "menu",
                    &label.unwrap_or_default(),
                    vec![],
                );
            }
            for _ in 0..3 {
                update(&mut app, &ctx);
            }
            assert!(
                app.plugins.list[index].running(),
                "{name}: {:?}",
                app.plugins.list[index].error
            );
        }
    }

    #[test]
    fn the_studio_is_a_document_tab_of_its_own() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        app.handle(&ctx, Action::CreateProject(NewProjectSpec::default()));
        let project_tab = app.active_project_tab_id().unwrap();
        app.plugins.studio.open = true;
        studio::render(&mut app, &ctx);
        assert!(app.active_document_is_plugin_studio());
        let tabs = app.document_tab_summaries();
        assert_eq!(tabs.len(), 2);
        let studio_tab = tabs
            .iter()
            .find(|tab| tab.kind == crate::app::DocumentKindSummary::PluginStudio)
            .expect("a studio tab");
        assert!(studio_tab.active && studio_tab.title == "Plugin Studio");
        let studio_id = studio_tab.id;
        // Back to the project, then the studio again: still one studio tab.
        app.handle(&ctx, Action::SwitchDocumentTab(project_tab));
        assert!(!app.active_document_is_plugin_studio());
        app.plugins.studio.open = true;
        studio::render(&mut app, &ctx);
        assert!(app.active_document_is_plugin_studio());
        assert_eq!(app.document_tab_summaries().len(), 2);
        app.handle(&ctx, Action::CloseDocumentTab(studio_id));
        assert_eq!(app.document_tab_summaries().len(), 1);
    }

    #[test]
    fn the_whole_project_survives_raw_data() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        app.handle(&ctx, Action::CreateProject(NewProjectSpec::default()));
        let json = serde_json::to_value(&app.state.project).unwrap();
        let value = Value::from_json(&json);
        let back: q0s_format::v2::ProjectV2 = serde_json::from_value(value.to_json()).unwrap();
        assert_eq!(back, app.state.project);
    }
}
