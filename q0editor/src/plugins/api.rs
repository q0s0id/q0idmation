//! Every word a plugin can use, with a line about it: the studio's
//! reference list, its suggestions and the block palette all come from
//! here. `{name}` in a label is where a block takes a value.

use egui::Color32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Cat {
    Events,
    Control,
    Variables,
    Editor,
    Project,
    Objects,
    Ui,
    Draw,
    Math,
    Text,
    Lists,
    Files,
    Plugin,
}

impl Cat {
    pub const ALL: [Cat; 13] = [
        Cat::Events,
        Cat::Control,
        Cat::Variables,
        Cat::Editor,
        Cat::Project,
        Cat::Objects,
        Cat::Ui,
        Cat::Draw,
        Cat::Math,
        Cat::Text,
        Cat::Lists,
        Cat::Files,
        Cat::Plugin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Cat::Events => "Events",
            Cat::Control => "Control",
            Cat::Variables => "Variables",
            Cat::Editor => "Editor",
            Cat::Project => "Project",
            Cat::Objects => "Objects",
            Cat::Ui => "Windows",
            Cat::Draw => "Draw",
            Cat::Math => "Math",
            Cat::Text => "Text",
            Cat::Lists => "Lists",
            Cat::Files => "Files",
            Cat::Plugin => "Plugin",
        }
    }

    pub fn color(self) -> Color32 {
        let rgb = |r, g, b| Color32::from_rgb(r, g, b);
        match self {
            Cat::Events => rgb(255, 191, 0),
            Cat::Control => rgb(255, 171, 25),
            Cat::Variables => rgb(255, 140, 26),
            Cat::Editor => rgb(76, 151, 255),
            Cat::Project => rgb(153, 102, 255),
            Cat::Objects => rgb(207, 99, 207),
            Cat::Ui => rgb(92, 177, 214),
            Cat::Draw => rgb(15, 189, 140),
            Cat::Math => rgb(89, 192, 89),
            Cat::Text => rgb(64, 175, 120),
            Cat::Lists => rgb(255, 102, 26),
            Cat::Files => rgb(91, 128, 165),
            Cat::Plugin => rgb(255, 102, 128),
        }
    }
}

pub(crate) struct Entry {
    /// What code writes: `editor.goto!`, `len`.
    pub name: &'static str,
    pub args: &'static [&'static str],
    /// The block's words, `{arg}` where a value goes.
    pub label: &'static str,
    pub doc: &'static str,
    pub cat: Cat,
    /// Gives back a value: a round block that fits in a slot.
    pub value: bool,
}

pub(crate) struct Event {
    pub event: &'static str,
    /// The quoted word after the event, when it takes one.
    pub arg: Option<&'static str>,
    pub params: &'static [&'static str],
    pub label: &'static str,
    pub doc: &'static str,
}

const fn command(
    name: &'static str,
    args: &'static [&'static str],
    label: &'static str,
    cat: Cat,
    doc: &'static str,
) -> Entry {
    Entry {
        name,
        args,
        label,
        doc,
        cat,
        value: false,
    }
}

const fn value(
    name: &'static str,
    args: &'static [&'static str],
    label: &'static str,
    cat: Cat,
    doc: &'static str,
) -> Entry {
    Entry {
        name,
        args,
        label,
        doc,
        cat,
        value: true,
    }
}

pub(crate) const EVENTS: &[Event] = &[
    Event { event: "enabled", arg: None, params: &[], label: "when this plugin turns on", doc: "Runs when the plugin is switched on, and when the editor starts with it on." },
    Event { event: "disabled", arg: None, params: &[], label: "when this plugin turns off", doc: "Runs just before the plugin is switched off." },
    Event { event: "menu", arg: Some("Do the thing"), params: &[], label: "when menu item {arg} is clicked", doc: "Adds an item to Plugins > (this plugin)." },
    Event { event: "window", arg: Some("My window"), params: &[], label: "draw window {arg}", doc: "A window under Plugins > (this plugin). Runs every frame while it is open; use ui to fill it." },
    Event { event: "tool", arg: Some("My tool"), params: &[], label: "when tool {arg} is picked", doc: "Adds a stage tool. While it is in use, press, drag, release and hover get the pointer." },
    Event { event: "press", arg: None, params: &["x", "y"], label: "when the stage is pressed at x y", doc: "The plugin's tool is in use and the stage was pressed, at stage x, y." },
    Event { event: "drag", arg: None, params: &["x", "y"], label: "when dragging on the stage at x y", doc: "Every frame of a drag with the plugin's tool." },
    Event { event: "release", arg: None, params: &["x", "y"], label: "when the stage is let go at x y", doc: "The press of the plugin's tool ended." },
    Event { event: "hover", arg: None, params: &["x", "y"], label: "when the pointer moves to x y", doc: "The pointer moved over the stage with the plugin's tool." },
    Event { event: "draw", arg: None, params: &[], label: "when the stage is drawn", doc: "Draw over the stage every frame with draw." },
    Event { event: "tick", arg: None, params: &["dt"], label: "every frame, dt seconds after the last", doc: "Runs every frame. Keep it quick." },
    Event { event: "frame", arg: None, params: &["n"], label: "when the playhead moves to frame n", doc: "The current frame changed; n counts from 1." },
    Event { event: "selection", arg: None, params: &[], label: "when the selection changes", doc: "Something else got selected." },
    Event { event: "opened", arg: None, params: &[], label: "when a project opens", doc: "Another project or tab became the current one." },
    Event { event: "saved", arg: None, params: &["path"], label: "when the project is saved to path", doc: "The project was saved." },
    Event { event: "key", arg: Some("ctrl+k"), params: &[], label: "when key {arg} is pressed", doc: "A key or chord like ctrl+shift+k, while no text field is being typed in. \"@name\" takes the keys from the plugin's `key name` setting, so people can pick their own." },
    Event { event: "action", arg: Some("insert-keyframe"), params: &["name"], label: "after command {arg} runs", doc: "An editor command ran: a button, a menu item, a shortcut or editor.action!. Without a quoted name it hears every command, and name says which." },
    Event { event: "message", arg: Some("hello"), params: &["data", "from"], label: "when message {arg} arrives", doc: "Another plugin sent this one a message with plugin.send!; from is its id." },
    Event { event: "stage_menu", arg: Some("Do this here"), params: &["x", "y"], label: "stage right-click item {arg}", doc: "Adds an item to the stage's right-click menu; x, y is where it was opened." },
    Event { event: "layer_menu", arg: Some("Do this to the layer"), params: &["layer"], label: "layer right-click item {arg}", doc: "Adds an item to a layer's right-click menu in the timeline." },
    Event { event: "frame_menu", arg: Some("Do this on the frame"), params: &["layer", "frame"], label: "frame right-click item {arg}", doc: "Adds an item to the timeline's right-click menu on frames." },
    Event { event: "library_menu", arg: Some("Do this to the item"), params: &["item"], label: "library right-click item {arg}", doc: "Adds an item to the library's right-click menu; item is the library item, or a symbol's timeline." },
    Event { event: "setting", arg: Some("name"), params: &["value"], label: "when setting {arg} changes to value", doc: "A setting of this plugin changed, or its button was pressed." },
];

pub(crate) const ENTRIES: &[Entry] = &[
    // core
    command("say!", &["text"], "say {text}", Cat::Plugin, "Shows a bubble from the plugin and writes to its console."),
    command("print!", &["text"], "print {text}", Cat::Plugin, "Writes to the plugin's console."),
    command("oops!", &["message"], "stop with error {message}", Cat::Control, "Stops with an error; `try` can catch it."),
    // plugin
    value("plugin.name", &[], "plugin name", Cat::Plugin, "This plugin's name."),
    value("plugin.id", &[], "plugin id", Cat::Plugin, "This plugin's id."),
    command("plugin.stop!", &[], "turn this plugin off", Cat::Plugin, "Switches the plugin off after this handler."),
    command("plugin.open!", &["title"], "open window {title}", Cat::Plugin, "Opens one of the plugin's windows."),
    command("plugin.close!", &["title"], "close window {title}", Cat::Plugin, "Closes one of the plugin's windows."),
    command("plugin.tool!", &["name"], "use tool {name}", Cat::Plugin, "Makes the plugin's tool the one in use; null puts it away."),
    command("plugin.send!", &["plugin", "name", "data"], "send {name} with {data} to plugin {plugin}", Cat::Plugin, "Sends a message another plugin hears with `when message`; \"*\" sends it to every plugin that is on."),
    value("plugin.running", &[], "plugins that are on", Cat::Plugin, "The ids of the plugins that are on, to send messages to."),
    value("plugin.blob", &["name"], "text of blob {name}", Cat::Plugin, "A blob of the plugin as text."),
    command("config.reset!", &[], "reset settings", Cat::Plugin, "Puts every setting back to its default."),
    value("config.all", &[], "all settings", Cat::Plugin, "Every setting as a map."),
    // editor
    value("editor.frame", &[], "current frame", Cat::Editor, "The frame the playhead is on, from 1."),
    command("editor.goto!", &["frame"], "go to frame {frame}", Cat::Editor, "Moves the playhead."),
    value("editor.frames", &[], "frames in the timeline", Cat::Editor, "How long the current timeline is."),
    command("editor.play!", &[], "play", Cat::Editor, "Starts playing."),
    command("editor.stop!", &[], "stop playing", Cat::Editor, "Stops playing."),
    value("editor.playing", &[], "playing?", Cat::Editor, "Whether it is playing."),
    value("editor.tool", &[], "tool in use", Cat::Editor, "The tool in use, like brush or select."),
    command("editor.tool!", &["name"], "use the {name} tool", Cat::Editor, "Picks a built-in tool: select, brush, pen, line, rectangle, oval..."),
    value("editor.zoom", &[], "zoom", Cat::Editor, "The stage zoom; 1 fits the window."),
    command("editor.zoom!", &["zoom"], "set zoom to {zoom}", Cat::Editor, "Zooms the stage."),
    command("editor.status!", &["text"], "show {text} in the status line", Cat::Editor, "Writes the status line at the bottom."),
    command("editor.action!", &["name"], "do {name}", Cat::Editor, "Runs any editor command by name, like insert-keyframe or undo."),
    value("editor.actions", &[], "all command names", Cat::Editor, "The names editor.action! takes."),
    value("editor.color", &[], "line colour", Cat::Editor, "The colour new lines get."),
    command("editor.color!", &["color"], "set line colour to {color}", Cat::Editor, "Sets the colour new lines get."),
    value("editor.fill", &[], "fill colour", Cat::Editor, "The colour new fills get, or null."),
    command("editor.fill!", &["color"], "set fill colour to {color}", Cat::Editor, "Sets the fill colour; null for no fill."),
    value("editor.width", &[], "line width", Cat::Editor, "The width new lines get."),
    command("editor.width!", &["width"], "set line width to {width}", Cat::Editor, "Sets the width new lines get."),
    value("editor.mouse", &[], "pointer on the stage", Cat::Editor, "Where the pointer is on the stage, as {x, y}, or null."),
    value("editor.layer", &[], "current layer", Cat::Editor, "The layer being drawn on."),
    value("editor.cells", &[], "picked timeline cells", Cat::Editor, "The frames picked in the timeline, as {layer, frame} each."),
    value("editor.version", &[], "editor version", Cat::Editor, "q0editor's version."),
    value("editor.dark", &[], "dark theme?", Cat::Editor, "Whether the theme is dark."),
    value("editor.accent", &[], "accent colour", Cat::Editor, "The theme's accent colour."),
    value("keys.down", &["key"], "key {key} held?", Cat::Editor, "Whether a key is held, like Space or A."),
    value("keys.pressed", &["key"], "key {key} pressed?", Cat::Editor, "Whether a key went down this frame."),
    value("keys.shift", &[], "shift held?", Cat::Editor, "Whether shift is held."),
    value("keys.ctrl", &[], "ctrl held?", Cat::Editor, "Whether ctrl (cmd on a Mac) is held."),
    value("mouse.x", &[], "pointer x", Cat::Editor, "The pointer's stage x, or null."),
    value("mouse.y", &[], "pointer y", Cat::Editor, "The pointer's stage y, or null."),
    // project
    value("project.name", &[], "project name", Cat::Project, "The project's name."),
    value("project.path", &[], "project file", Cat::Project, "Where the project is saved, or null."),
    value("project.width", &[], "stage width", Cat::Project, "The stage's width."),
    value("project.height", &[], "stage height", Cat::Project, "The stage's height."),
    command("project.size!", &["width", "height"], "set stage size to {width} by {height}", Cat::Project, "Resizes the stage."),
    value("project.fps", &[], "frame rate", Cat::Project, "Frames a second."),
    command("project.fps!", &["fps"], "set frame rate to {fps}", Cat::Project, "Sets the frame rate."),
    value("project.background", &[], "background colour", Cat::Project, "The stage colour."),
    command("project.background!", &["color"], "set background to {color}", Cat::Project, "Sets the stage colour."),
    value("project.dirty", &[], "unsaved changes?", Cat::Project, "Whether there are unsaved changes."),
    command("project.save!", &[], "save the project", Cat::Project, "Saves, asking where when it was never saved."),
    value("project.timeline", &[], "this timeline", Cat::Project, "The timeline being edited."),
    value("project.main", &[], "main timeline", Cat::Project, "The project's main timeline."),
    value("project.timelines", &[], "all timelines", Cat::Project, "The main timeline and every symbol's."),
    value("project.layers", &[], "layers", Cat::Project, "The layers of the timeline being edited, top first."),
    value("project.layer", &["name"], "layer {name}", Cat::Project, "The current layer, the one with that name, or makes a given layer current."),
    command("project.add_layer!", &["name"], "add a layer called {name}", Cat::Project, "Adds a layer above the current one and gives it back."),
    value("project.labels", &[], "frame labels", Cat::Project, "This timeline's frame labels, as {frame, name} each."),
    command("project.label!", &["frame", "name"], "label frame {frame} {name}", Cat::Project, "Names a frame of this timeline; null takes the label off."),
    value("project.import!", &["path"], "import file {path}", Cat::Project, "Brings a picture, sound or video into the library. Gives back a picture at once; a sound or video turns up a moment later."),
    value("project.place!", &["item", "x", "y"], "place {item} at {x} {y}", Cat::Project, "Puts a library item or symbol on the stage, on the current layer and frame, and gives back the object."),
    value("project.objects", &["frame"], "objects on frame {frame}", Cat::Project, "Every object on a frame (the current one when left out)."),
    value("project.selection", &[], "selected objects", Cat::Project, "The selected objects."),
    command("project.select!", &["objects"], "select {objects}", Cat::Project, "Selects objects."),
    command("project.deselect!", &[], "select nothing", Cat::Project, "Clears the selection."),
    value("project.assets", &[], "library items", Cat::Project, "Everything in the library."),
    value("project.asset", &["id"], "library item {id}", Cat::Project, "A library item by id or name."),
    command("project.line!", &["x1", "y1", "x2", "y2", "color", "width"], "draw a line from {x1} {y1} to {x2} {y2}", Cat::Project, "Draws a line on the current layer; colour and width are optional."),
    command("project.rect!", &["x", "y", "w", "h", "fill", "line", "width"], "draw a rectangle at {x} {y} size {w} {h}", Cat::Project, "Draws a rectangle; fill, line colour and width are optional."),
    command("project.circle!", &["x", "y", "r", "fill", "line", "width"], "draw a circle at {x} {y} radius {r}", Cat::Project, "Draws a circle; fill, line colour and width are optional."),
    command("project.path!", &["points", "closed", "fill", "line", "width"], "draw a path through {points}", Cat::Project, "Draws a path through [x, y] points."),
    value("project.raw", &[], "whole project as data", Cat::Project, "Everything in the project as maps and lists. Slow on big projects."),
    command("project.raw!", &["data"], "replace the project with {data}", Cat::Project, "Replaces the whole project with edited data."),
    value("project.runtime", &[], "project extras as data", Cat::Project, "Physics, particles, lights, camera, labels and more, as data."),
    command("project.runtime!", &["data"], "replace project extras with {data}", Cat::Project, "Replaces them with edited data."),
    // objects (fields and methods on handles, shown for the reference)
    value("obj.x", &[], "x of object", Cat::Objects, "An object's x; set it to move it. Also y, rotation, scale_x, scale_y, skew_x, skew_y, alpha."),
    value("obj.kind", &[], "kind of object", Cat::Objects, "drawing, bitmap, video, rig or symbol."),
    value("obj.name", &[], "name of object", Cat::Objects, "The instance name q0lang uses; set it to rename."),
    value("obj.raw", &[], "object as data", Cat::Objects, "Every detail of the object; set it to change any of them."),
    command("obj.move!", &["dx", "dy"], "move object by {dx} {dy}", Cat::Objects, "Moves an object."),
    command("obj.copy!", &["dx", "dy"], "copy object, shifted {dx} {dy}", Cat::Objects, "Copies an object and gives the copy back."),
    command("obj.remove!", &[], "remove object", Cat::Objects, "Deletes an object."),
    command("obj.select!", &[], "select object", Cat::Objects, "Selects just this object."),
    value("obj.paths", &[], "paths of a drawing", Cat::Objects, "A drawing's lines and shapes as {points, handles, closed}, points as {x, y} in stage pixels; set it to reshape the drawing. A bare list of points is an open line."),
    value("layer.objects", &["frame"], "objects of layer", Cat::Objects, "A layer's objects on a frame. Layers also have name, kind, visible, locked, keyframes and raw."),
    command("layer.select!", &[], "make layer current", Cat::Objects, "Makes a layer the current one."),
    command("layer.remove!", &[], "delete layer", Cat::Objects, "Deletes a layer."),
    command("layer.keyframe!", &["frame"], "keyframe at frame {frame}", Cat::Objects, "Makes a keyframe there, as F6 does; locked layers take it too."),
    command("layer.blank!", &["frame"], "blank keyframe at frame {frame}", Cat::Objects, "Makes an empty keyframe there, as F7 does."),
    command("layer.clear!", &["frame"], "clear keyframe at frame {frame}", Cat::Objects, "Takes the keyframe there away, as Shift+F6 does."),
    command("layer.add_frames!", &["frame", "count"], "add {count} frames at {frame}", Cat::Objects, "Holds frames there, as F5 does."),
    command("layer.remove_frames!", &["frame", "count"], "remove {count} frames from {frame}", Cat::Objects, "Takes frames out of this layer and closes the gap."),
    command("layer.tween!", &["frame", "kind"], "tween at frame {frame} as {kind}", Cat::Objects, "Tweens the keyframe span there: \"motion\", \"shape\" or \"none\"."),
    value("timeline.layers", &[], "layers of timeline", Cat::Objects, "A timeline's layers. Timelines also have name, frames, script and raw."),
    command("timeline.enter!", &[], "edit timeline", Cat::Objects, "Opens a symbol's timeline for editing."),
    // ui
    command("ui.text!", &["text"], "text {text}", Cat::Ui, "A line of text."),
    command("ui.heading!", &["text"], "heading {text}", Cat::Ui, "A big line of text."),
    command("ui.small!", &["text"], "small text {text}", Cat::Ui, "A quiet line of text."),
    command("ui.colored!", &["color", "text"], "{color} text {text}", Cat::Ui, "A line of coloured text."),
    command("ui.page!", &["markup"], "page text {markup}", Cat::Ui, "Text in page markup: **bold**, {red}colour{/} and the rest."),
    value("ui.button", &["label"], "button {label} clicked?", Cat::Ui, "A button; true on the frame it is clicked."),
    value("ui.link", &["label"], "link {label} clicked?", Cat::Ui, "A link-looking button."),
    value("ui.slider", &["label", "value", "min", "max"], "slider {label} at {value} from {min} to {max}", Cat::Ui, "A slider; gives back the new value."),
    value("ui.number", &["label", "value"], "number box {label} at {value}", Cat::Ui, "A number field; gives back the new value."),
    value("ui.toggle", &["label", "on"], "checkbox {label} at {on}", Cat::Ui, "A checkbox; gives back true or false."),
    value("ui.input", &["label", "text"], "text box {label} with {text}", Cat::Ui, "A text field; gives back the text."),
    value("ui.color", &["label", "color"], "colour box {label} with {color}", Cat::Ui, "A colour picker; gives back the colour."),
    value("ui.choice", &["label", "value", "options"], "pick {label} at {value} of {options}", Cat::Ui, "A drop-down; gives back what is picked."),
    command("ui.sep!", &[], "line across", Cat::Ui, "A separating line."),
    command("ui.space!", &["px"], "space {px}", Cat::Ui, "Empty space."),
    command("ui.progress!", &["amount"], "progress bar at {amount}", Cat::Ui, "A bar filled from 0 to 1."),
    command("ui.row!", &["fn"], "in a row: {fn}", Cat::Ui, "Lays out what a function adds side by side."),
    command("ui.column!", &["fn"], "in a column: {fn}", Cat::Ui, "Lays out what a function adds one under another."),
    command("ui.group!", &["title", "fn"], "foldable group {title}: {fn}", Cat::Ui, "A section that folds open and shut."),
    command("ui.image!", &["blob", "width"], "picture {blob}", Cat::Ui, "One of the plugin's pictures."),
    // draw
    command("draw.line!", &["x1", "y1", "x2", "y2", "color", "width"], "line from {x1} {y1} to {x2} {y2} in {color}", Cat::Draw, "A line over the stage, in stage pixels."),
    command("draw.circle!", &["x", "y", "r", "color"], "dot at {x} {y} radius {r} in {color}", Cat::Draw, "A filled circle."),
    command("draw.ring!", &["x", "y", "r", "color", "width"], "ring at {x} {y} radius {r} in {color}", Cat::Draw, "A circle's outline."),
    command("draw.rect!", &["x", "y", "w", "h", "color"], "box at {x} {y} size {w} {h} in {color}", Cat::Draw, "A filled rectangle."),
    command("draw.box!", &["x", "y", "w", "h", "color", "width"], "frame at {x} {y} size {w} {h} in {color}", Cat::Draw, "A rectangle's outline."),
    command("draw.text!", &["x", "y", "text", "color", "size"], "write {text} at {x} {y}", Cat::Draw, "Text over the stage."),
    command("draw.path!", &["points", "color", "width"], "lines through {points} in {color}", Cat::Draw, "Lines through [x, y] points."),
    command("draw.poly!", &["points", "color"], "shape {points} in {color}", Cat::Draw, "A filled convex shape."),
    command("draw.arrow!", &["x1", "y1", "x2", "y2", "color", "width"], "arrow from {x1} {y1} to {x2} {y2} in {color}", Cat::Draw, "A line with a head at its end."),
    command("draw.image!", &["blob", "x", "y", "width"], "picture {blob} at {x} {y}", Cat::Draw, "One of the plugin's pictures over the stage; width is in stage pixels."),
    // files
    value("file.read", &["path"], "text of file {path}", Cat::Files, "Reads a text file."),
    command("file.write!", &["path", "text"], "write {text} to file {path}", Cat::Files, "Writes a text file, replacing it."),
    command("file.append!", &["path", "text"], "add {text} to file {path}", Cat::Files, "Adds to the end of a text file."),
    value("file.exists", &["path"], "file {path} exists?", Cat::Files, "Whether something is there."),
    value("file.list", &["folder"], "files in {folder}", Cat::Files, "What is in a folder."),
    command("file.delete!", &["path"], "delete file {path}", Cat::Files, "Deletes a file."),
    command("file.folder!", &["path"], "make folder {path}", Cat::Files, "Makes a folder and those above it."),
    value("file.dir", &[], "plugin's own folder", Cat::Files, "A folder for this plugin's files."),
    value("file.home", &[], "home folder", Cat::Files, "Your home folder."),
    value("file.pick", &["title", "kinds"], "file picked to open, titled {title}", Cat::Files, "Asks for a file to open, like [\"png\", \"jpg\"]; null when nothing was picked."),
    value("file.pick_save", &["title", "kinds"], "file picked to save, titled {title}", Cat::Files, "Asks where to save a file; null when nothing was picked."),
    value("file.pick_folder", &["title"], "folder picked, titled {title}", Cat::Files, "Asks for a folder; null when nothing was picked."),
    value("settings.get", &["path"], "editor setting {path}", Cat::Files, "One of q0editor's own settings, like theme.accent or autosave_enabled."),
    command("settings.set!", &["path", "value"], "set editor setting {path} to {value}", Cat::Files, "Changes one of q0editor's settings."),
    value("settings.all", &[], "all editor settings", Cat::Files, "All of q0editor's settings as data."),
    command("clip.copy!", &["text"], "copy {text}", Cat::Files, "Puts text on the clipboard."),
    // math
    value("random", &["low", "high"], "random from {low} to {high}", Cat::Math, "A random whole number between two whole numbers, or any number between others."),
    value("round", &["x"], "round {x}", Cat::Math, "Rounds; a second value keeps that many decimals."),
    value("floor", &["x"], "floor {x}", Cat::Math, "Rounds down."),
    value("ceil", &["x"], "ceil {x}", Cat::Math, "Rounds up."),
    value("abs", &["x"], "abs {x}", Cat::Math, "Without its minus."),
    value("min", &["a", "b"], "smaller of {a} {b}", Cat::Math, "The smallest of numbers or a list."),
    value("max", &["a", "b"], "bigger of {a} {b}", Cat::Math, "The biggest of numbers or a list."),
    value("clamp", &["x", "low", "high"], "{x} kept between {low} and {high}", Cat::Math, "Keeps a number inside a range."),
    value("lerp", &["a", "b", "t"], "{t} of the way from {a} to {b}", Cat::Math, "In between two numbers."),
    value("sqrt", &["x"], "square root of {x}", Cat::Math, "The square root."),
    value("sin", &["deg"], "sin {deg}", Cat::Math, "Sine of an angle in degrees."),
    value("cos", &["deg"], "cos {deg}", Cat::Math, "Cosine of an angle in degrees."),
    value("atan2", &["y", "x"], "angle to {x} {y}", Cat::Math, "The angle of a direction, in degrees."),
    value("hypot", &["x", "y"], "length of {x} {y}", Cat::Math, "The distance to a point."),
    value("num", &["text"], "number from {text}", Cat::Math, "Reads a number from text, or null."),
    value("rgb", &["r", "g", "b"], "colour {r} {g} {b}", Cat::Math, "A colour from red, green and blue, 0 to 255."),
    value("rgba", &["r", "g", "b", "a"], "colour {r} {g} {b} {a}", Cat::Math, "A colour with how see-through it is."),
    value("hsv", &["hue", "sat", "val"], "colour hue {hue} sat {sat} val {val}", Cat::Math, "A colour from hue in degrees, saturation and value from 0 to 1."),
    value("mix", &["a", "b", "t"], "{t} of the way from colour {a} to {b}", Cat::Math, "Blends two colours."),
    value("clock", &[], "seconds since start", Cat::Math, "How long the plugin has been on."),
    value("now", &[], "time now", Cat::Math, "Seconds since 1970, for dates."),
    // text
    value("str", &["value"], "text of {value}", Cat::Text, "Any value as text."),
    value("len", &["value"], "length of {value}", Cat::Text, "How many letters, items or keys."),
    value("upper", &["text"], "{text} in capitals", Cat::Text, "In capital letters."),
    value("lower", &["text"], "{text} in small letters", Cat::Text, "In small letters."),
    value("trim", &["text"], "{text} without spaces round it", Cat::Text, "Without spaces at the ends."),
    value("split", &["text", "by"], "{text} split at {by}", Cat::Text, "Cuts text into a list."),
    value("join", &["list", "with"], "{list} joined with {with}", Cat::Text, "Joins a list into text."),
    value("replace", &["text", "old", "new"], "{text} with {old} as {new}", Cat::Text, "Swaps every piece of text."),
    value("contains", &["where", "what"], "{where} contains {what}?", Cat::Text, "Whether text, a list or a map has something."),
    value("starts", &["text", "start"], "{text} starts with {start}?", Cat::Text, "Whether text begins so."),
    value("slice", &["value", "from", "to"], "{value} from {from} to {to}", Cat::Text, "A piece of text or a list."),
    // lists
    command("push!", &["list", "item"], "add {item} to {list}", Cat::Lists, "Adds to the end of a list."),
    value("pop!", &["list"], "take the last of {list}", Cat::Lists, "Removes the last item and gives it back."),
    command("insert!", &["list", "at", "item"], "put {item} at {at} of {list}", Cat::Lists, "Puts an item in a list."),
    value("remove!", &["list", "at"], "take item {at} out of {list}", Cat::Lists, "Removes an item and gives it back."),
    command("clear!", &["list"], "empty {list}", Cat::Lists, "Empties a list or map."),
    command("sort!", &["list"], "sort {list}", Cat::Lists, "Sorts a list; a function picks what to sort by."),
    command("shuffle!", &["list"], "shuffle {list}", Cat::Lists, "Mixes a list up."),
    value("map", &["list", "fn"], "{list} each changed by {fn}", Cat::Lists, "A new list of each item put through a function."),
    value("filter", &["list", "fn"], "{list} kept where {fn}", Cat::Lists, "A new list of the items a function says yes to."),
    value("sum", &["list"], "sum of {list}", Cat::Lists, "Adds a list of numbers."),
    value("first", &["list"], "first of {list}", Cat::Lists, "The first item."),
    value("last", &["list"], "last of {list}", Cat::Lists, "The last item."),
    value("pick", &["list"], "random item of {list}", Cat::Lists, "Any one item."),
    value("range", &["from", "to"], "numbers from {from} to {to}", Cat::Lists, "A list of numbers, without the last."),
    value("keys", &["map"], "keys of {map}", Cat::Lists, "A map's keys."),
    value("values", &["map"], "values of {map}", Cat::Lists, "A map's values."),
    value("has", &["map", "key"], "{map} has {key}?", Cat::Lists, "Whether a map has a key."),
    value("copy", &["value"], "copy of {value}", Cat::Lists, "A copy that does not share its lists."),
    value("json", &["value"], "{value} as json", Cat::Lists, "Any value as json text."),
    value("parse_json", &["text"], "json {text} as a value", Cat::Lists, "Reads json text."),
];

pub(crate) fn is_namespace(word: &str) -> bool {
    matches!(
        word,
        "editor"
            | "project"
            | "ui"
            | "draw"
            | "file"
            | "settings"
            | "config"
            | "clip"
            | "plugin"
            | "keys"
            | "mouse"
    )
}

pub(crate) fn entry(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

pub(crate) fn event(name: &str) -> Option<&'static Event> {
    EVENTS.iter().find(|event| event.event == name)
}

/// Names that complete `prefix`, for suggestions while typing.
pub(crate) fn complete(prefix: &str) -> Vec<&'static Entry> {
    if prefix.is_empty() {
        return Vec::new();
    }
    ENTRIES
        .iter()
        .filter(|entry| {
            entry.name.starts_with(prefix)
                && entry.name != prefix
                && !entry.name.starts_with("obj.")
        })
        .take(8)
        .collect()
}

/// The reference page, `docs/q0ex/api.md`, written from this catalog.
#[cfg(test)]
pub(crate) fn reference_markdown() -> String {
    let mut out = String::from(
        "# q0ex api reference\n\n[guide](README.md) · [examples](examples/)\n\n\
         Every word a plugin can use besides the language itself. Words ending in `!` do something; \
         the others give back a value. Frames count from 1, angles are degrees and positions are \
         stage pixels. This page is written from the editor's own list, the same one the Plugin \
         Studio's reference and blocks use.\n\n## events\n\n| when | gives | runs |\n| --- | --- | --- |\n",
    );
    for event in EVENTS {
        let mut head = format!("when {}", event.event);
        if let Some(arg) = event.arg {
            head.push_str(&format!(" \"{arg}\""));
        }
        let params = if event.params.is_empty() {
            String::new()
        } else {
            event.params.join(", ")
        };
        out.push_str(&format!("| `{head}` | {params} | {} |\n", event.doc));
    }
    for cat in Cat::ALL {
        let entries = ENTRIES
            .iter()
            .filter(|entry| entry.cat == cat)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n## {}\n\n| word | takes | does |\n| --- | --- | --- |\n",
            cat.label().to_lowercase()
        ));
        for entry in entries {
            let shown = match entry.name.split_once('.') {
                Some(("obj", rest)) => format!("object.{rest}"),
                _ => entry.name.to_string(),
            };
            let value = if entry.value { " → value" } else { "" };
            out.push_str(&format!(
                "| `{shown}` | {} | {}{value} |\n",
                entry.args.join(", "),
                entry.doc
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_page_is_up_to_date() {
        let page = include_str!("../../../docs/q0ex/api.md").replace("\r\n", "\n");
        assert!(
            page == reference_markdown(),
            "docs/q0ex/api.md is stale; rerun `cargo test -p q0editor write_reference_page -- --ignored`"
        );
    }

    #[test]
    fn the_guide_examples_are_real_q0ex() {
        let guide = include_str!("../../../docs/q0ex/README.md").replace("\r\n", "\n");
        let mut checked = 0;
        for block in guide.split("```q0ex\n").skip(1) {
            let code = block.split("```").next().unwrap_or_default();
            // Sketches with `...` in them are not whole programs.
            if code.contains("...") && !code.contains("blob ") {
                continue;
            }
            let code = code.replace("iVBORw0KGgoAAAANSUhEUgAA...", "iVBORw0KGgo=");
            if let Err(error) = q0ex::parse(&code) {
                panic!("{error}\n{code}");
            }
            checked += 1;
        }
        assert!(checked >= 10, "only {checked} examples were checked");
    }

    #[test]
    #[ignore = "writes docs/q0ex/api.md"]
    fn write_reference_page() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/q0ex/api.md");
        std::fs::write(path, reference_markdown()).unwrap();
    }

    #[test]
    fn every_label_names_its_arguments() {
        for entry in ENTRIES {
            let mut rest = entry.label;
            while let Some(start) = rest.find('{') {
                let end = rest[start..].find('}').expect("closed slot") + start;
                let slot = &rest[start + 1..end];
                assert!(
                    entry.args.contains(&slot),
                    "{}: `{slot}` is not an argument",
                    entry.name
                );
                rest = &rest[end + 1..];
            }
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in ENTRIES {
            assert!(seen.insert(entry.name), "{} twice", entry.name);
        }
    }

    #[test]
    fn library_entries_exist() {
        for entry in ENTRIES.iter().filter(|entry| {
            !entry.name.contains('.') && !entry.name.ends_with('!')
                || matches!(
                    entry.name,
                    "push!" | "pop!" | "insert!" | "remove!" | "clear!" | "sort!" | "shuffle!"
                )
        }) {
            if matches!(entry.name, "say!" | "print!") {
                continue;
            }
            assert!(
                q0ex::stdlib::has(entry.name),
                "{} is not in the standard library",
                entry.name
            );
        }
    }
}
