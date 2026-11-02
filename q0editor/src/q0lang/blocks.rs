//! Simple mode for q0lang: the script as blocks, dark and blunt.
//!
//! The blocks are the program. Every block is a statement or a value of the
//! parsed script, so moving a block moves code, and Pro mode's text is the
//! printer's writing of the blocks. Unlike the plugin studio's rounded,
//! Scratch-like blocks, these keep the shape of the code: one column, top to
//! bottom in the order the script runs, square slabs with a coloured spine,
//! and the line numbers of the text down the side.

use std::time::Instant;

use egui::{
    pos2, vec2, Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui,
    Vec2,
};
use q0s_format::q0lang::parser::{
    self, BinaryOp, Block, Statement, TimelineSignal, UnaryOp, Value, Visibility, BUILTIN_LIBRARIES,
};
use q0s_format::q0lang::printer;

// ------------------------------------------------------------------ source

/// A `#` comment, kept in the program as a statement of its own.
const NOTE: &str = "__note!";

/// Splits a line into its code and its `#` comment, quotes respected.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '#' {
            return (&line[..index], Some(&line[index + 1..]));
        }
    }
    (line, None)
}

fn note(text: &str) -> Statement {
    Statement::Unknown {
        text: format!("{NOTE} {}", printer::quote(text)),
    }
}

/// The words of a note statement.
fn note_text(statement: &Statement) -> Option<String> {
    let Statement::Unknown { text } = statement else {
        return None;
    };
    let rest = text.trim().strip_prefix(NOTE)?;
    Some(match parser::parse_expression(rest) {
        Ok(Value::String(words)) => words,
        _ => rest.trim().to_string(),
    })
}

/// The statements of `source`, comments kept as notes; or why it does not
/// read cleanly enough to be blocks.
pub fn read(source: &str) -> Result<Vec<Statement>, String> {
    let mut lines = Vec::new();
    for line in source.lines() {
        let (code, comment) = split_comment(line);
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        if let Some(comment) = comment {
            let words = comment.strip_prefix(' ').unwrap_or(comment).trim_end();
            if let Statement::Unknown { text } = note(words) {
                lines.push(format!("{indent}{text}"));
            }
        }
        if !code.trim().is_empty() {
            lines.push(code.trim_end().to_string());
        } else if comment.is_none() {
            lines.push(String::new());
        }
    }
    let program = parser::parse(&lines.join("\n"));
    match program.diagnostics.into_iter().next() {
        Some(diagnostic) => Err(diagnostic.message),
        None => Ok(program.statements),
    }
}

/// The text of `statements`, and the line each statement starts on, in
/// the order [`read`] and the blocks walk them.
pub fn write(statements: &[Statement]) -> (String, Vec<usize>) {
    let (text, starts) = printer::print_program(statements);
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(NOTE) {
            let indent = &line[..line.len() - trimmed.len()];
            let words = note_text(&Statement::Unknown {
                text: trimmed.to_string(),
            })
            .unwrap_or_default();
            out.push_str(indent);
            out.push('#');
            if !words.is_empty() {
                out.push(' ');
                out.push_str(&words);
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, starts)
}

// ----------------------------------------------------------------- catalog

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cat {
    Flow,
    Data,
    Math,
    Func,
    Stage,
    Layer,
    Window,
    Fx,
    Misc,
}

impl Cat {
    const ALL: [Self; 9] = [
        Self::Flow,
        Self::Data,
        Self::Math,
        Self::Func,
        Self::Stage,
        Self::Layer,
        Self::Window,
        Self::Fx,
        Self::Misc,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Flow => "FLOW",
            Self::Data => "DATA",
            Self::Math => "MATH",
            Self::Func => "FUNC",
            Self::Stage => "STAGE",
            Self::Layer => "LAYER",
            Self::Window => "WINDOW",
            Self::Fx => "FX",
            Self::Misc => "MISC",
        }
    }

    /// Harsh colours on black: each kind of block has its own spine.
    fn color(self) -> Color32 {
        match self {
            Self::Flow => Color32::from_rgb(255, 59, 48),
            Self::Data => Color32::from_rgb(255, 159, 10),
            Self::Math => Color32::from_rgb(48, 209, 88),
            Self::Func => Color32::from_rgb(191, 90, 242),
            Self::Stage => Color32::from_rgb(100, 210, 255),
            Self::Layer => Color32::from_rgb(198, 255, 0),
            Self::Window => Color32::from_rgb(235, 235, 240),
            Self::Fx => Color32::from_rgb(130, 120, 255),
            Self::Misc => Color32::from_rgb(142, 142, 147),
        }
    }
}

/// What a value slot expects; it picks the suggestions shown while typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arg {
    Num,
    Text,
    Layer,
    Thing,
    Emitter,
    Light,
    Body,
    Link,
    Scene,
    Control,
    Pose,
    Switch,
    Any,
}

struct Entry {
    /// The command with its `!`, or the function.
    name: &'static str,
    /// Words and `{arg}` slots, in the order the block shows them.
    label: &'static str,
    args: &'static [(&'static str, Arg)],
    cat: Cat,
}

macro_rules! entry {
    ($name:expr, $label:expr, $cat:ident $(, $arg:literal : $kind:ident)*) => {
        Entry {
            name: $name,
            label: $label,
            args: &[$(($arg, Arg::$kind)),*],
            cat: Cat::$cat,
        }
    };
}

const ENTRIES: &[Entry] = &[
    // stage
    entry!("q0scene.switch!", "SWITCH SCENE {alias}", Stage, "alias": Scene),
    // layers
    entry!("q0layer.hide!", "HIDE LAYER {layer}", Layer, "layer": Layer),
    entry!("q0layer.show!", "SHOW LAYER {layer}", Layer, "layer": Layer),
    entry!("q0layer.move!", "SHIFT LAYER {layer} BY {x} {y}", Layer, "layer": Layer, "x": Num, "y": Num),
    entry!("q0layer.alpha!", "FADE LAYER {layer} TO {alpha}", Layer, "layer": Layer, "alpha": Num),
    entry!("q0layer.reset!", "RESET LAYER {layer}", Layer, "layer": Layer),
    entry!("q0layer.solid!", "LAYER {layer} WALLS IN {thing}", Layer, "layer": Layer, "thing": Thing),
    entry!("q0layer.unsolid!", "LAYER {layer} LETS {thing} THROUGH", Layer, "layer": Layer, "thing": Thing),
    entry!("q0layer.touches", "{thing} TOUCHES {layer}", Layer, "layer": Layer, "thing": Thing),
    entry!("q0layer.which", "WHAT ON {layer} {thing} TOUCHES", Layer, "layer": Layer, "thing": Thing),
    entry!("q0layer.hit", "POINT {x} {y} ON {layer}", Layer, "layer": Layer, "x": Num, "y": Num),
    entry!("q0layer.circle", "CIRCLE {x} {y} R {radius} ON {layer}", Layer, "layer": Layer, "x": Num, "y": Num, "radius": Num),
    entry!("q0layer.box", "BOX {left} {top} {width} {height} ON {layer}", Layer, "layer": Layer, "left": Num, "top": Num, "width": Num, "height": Num),
    entry!("q0layer.visible", "{layer} SHOWS", Layer, "layer": Layer),
    entry!("q0layer.count", "OBJECTS ON {layer}", Layer, "layer": Layer),
    // window
    entry!("q0window.fullscreen!", "FULLSCREEN {on}", Window, "on": Switch),
    entry!("q0window.ui!", "PLAYER UI {on}", Window, "on": Switch),
    entry!("q0window.hotkeys!", "F9 F4 KEYS {on}", Window, "on": Switch),
    entry!("q0window.top!", "ALWAYS ON TOP {on}", Window, "on": Switch),
    entry!("q0window.move!", "MOVE WINDOW TO {x} {y}", Window, "x": Num, "y": Num),
    entry!("q0window.size!", "WINDOW SIZE {width} {height}", Window, "width": Num, "height": Num),
    entry!("q0window.center!", "CENTER WINDOW", Window),
    entry!("q0window.title!", "WINDOW TITLE {text}", Window, "text": Text),
    entry!("q0window.minimize!", "MINIMIZE WINDOW", Window),
    entry!("q0window.perf!", "COPY PERFORMANCE LOG", Window),
    entry!("q0window.quit!", "QUIT", Window),
    // effects
    entry!("q0particles.start!", "START PARTICLES {emitter}", Fx, "emitter": Emitter),
    entry!("q0particles.stop!", "STOP PARTICLES {emitter}", Fx, "emitter": Emitter),
    entry!("q0particles.burst!", "BURST {count} FROM {emitter}", Fx, "emitter": Emitter, "count": Num),
    entry!("q0particles.move!", "MOVE PARTICLES {emitter} TO {x} {y}", Fx, "emitter": Emitter, "x": Num, "y": Num),
    entry!("q0particles.clear!", "CLEAR PARTICLES {emitter}", Fx, "emitter": Emitter),
    entry!("q0light.on!", "LIGHT ON {light}", Fx, "light": Light),
    entry!("q0light.off!", "LIGHT OFF {light}", Fx, "light": Light),
    entry!("q0light.move!", "MOVE LIGHT {light} TO {x} {y}", Fx, "light": Light, "x": Num, "y": Num),
    entry!("q0light.color!", "LIGHT {light} COLOUR {red} {green} {blue}", Fx, "light": Light, "red": Num, "green": Num, "blue": Num),
    entry!("q0light.brightness!", "LIGHT {light} BRIGHTNESS {value}", Fx, "light": Light, "value": Num),
    entry!("q0light.size!", "LIGHT {light} REACH {radius}", Fx, "light": Light, "radius": Num),
    entry!("q0light.darkness!", "DARKNESS {value}", Fx, "value": Num),
    entry!("q0physics.start!", "PHYSICS START", Fx),
    entry!("q0physics.pause!", "PHYSICS PAUSE", Fx),
    entry!("q0physics.reset!", "PHYSICS RESET", Fx),
    entry!("q0physics.push!", "PUSH {body} BY {x} {y}", Fx, "body": Body, "x": Num, "y": Num),
    entry!("q0physics.velocity!", "SPEED OF {body} {x} {y}", Fx, "body": Body, "x": Num, "y": Num),
    entry!("q0physics.spin!", "SPIN {body} AT {degrees}", Fx, "body": Body, "degrees": Num),
    entry!("q0physics.teleport!", "TELEPORT {body} TO {x} {y}", Fx, "body": Body, "x": Num, "y": Num),
    entry!("q0physics.gravity!", "GRAVITY {x} {y}", Fx, "x": Num, "y": Num),
    entry!("q0physics.cut!", "CUT {link}", Fx, "link": Link),
    entry!("q0rig.position!", "RIG {control} TO {x} {y}", Fx, "control": Control, "x": Num, "y": Num),
    entry!("q0rig.value!", "RIG {control} = {value}", Fx, "control": Control, "value": Num),
    entry!("q0rig.reset!", "RIG RESET {control}", Fx, "control": Control),
    entry!("q0rig.pose!", "POSE {pose} AT {weight}", Fx, "pose": Pose, "weight": Num),
    entry!("q0rig.pose_reset!", "DROP POSE {pose}", Fx, "pose": Pose),
    // misc
    entry!("q0shell.print!", "PRINT {value}", Misc, "value": Any),
    // math
    entry!("sin", "SIN {x}", Math, "x": Num),
    entry!("cos", "COS {x}", Math, "x": Num),
    entry!("tan", "TAN {x}", Math, "x": Num),
    entry!("sqrt", "SQRT {x}", Math, "x": Num),
    entry!("abs", "ABS {x}", Math, "x": Num),
    entry!("floor", "FLOOR {x}", Math, "x": Num),
    entry!("ceil", "CEIL {x}", Math, "x": Num),
    entry!("round", "ROUND {x}", Math, "x": Num),
    entry!("min", "MIN {a} {b}", Math, "a": Num, "b": Num),
    entry!("max", "MAX {a} {b}", Math, "a": Num, "b": Num),
    entry!("clamp", "CLAMP {x} {low} {high}", Math, "x": Num, "low": Num, "high": Num),
    entry!("lerp", "LERP {a} {b} {t}", Math, "a": Num, "b": Num, "t": Num),
];

fn entry(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

/// Which library a name belongs to, for blocks that bring their import.
fn library_of(name: &str) -> Option<&'static str> {
    let prefix = name.split('.').next().unwrap_or_default();
    Some(match prefix {
        "input" => "q0.input",
        "mouse" => "q0.mouse",
        "self" => "q0.object",
        "q0layer" => "q0.layer",
        "q0window" | "window" | "screen" => "q0.window",
        "q0particles" => "q0.particles",
        "q0light" => "q0.light",
        "q0physics" => "q0.physics",
        "q0rig" => "q0.rig",
        "q0scene" | "stage" => "q0.scene",
        "time" => "q0.time",
        _ => return None,
    })
}

fn command_name(statement: &Statement) -> Option<(String, &[Value])> {
    let (library, command, args) = match statement {
        Statement::ShellCommand { command, args } => ("q0shell", command, args),
        Statement::RigCommand { command, args } => ("q0rig", command, args),
        Statement::SceneCommand { command, args } => ("q0scene", command, args),
        Statement::PhysicsCommand { command, args } => ("q0physics", command, args),
        Statement::ParticlesCommand { command, args } => ("q0particles", command, args),
        Statement::LightCommand { command, args } => ("q0light", command, args),
        Statement::LayerCommand { command, args } => ("q0layer", command, args),
        Statement::WindowCommand { command, args } => ("q0window", command, args),
        _ => return None,
    };
    Some((format!("{library}.{command}!"), args.as_slice()))
}

/// The colour a name gives a block: host names look like their library.
fn name_cat(name: &str, names: &Names) -> Cat {
    let prefix = name.split('.').next().unwrap_or_default();
    match prefix {
        "window" | "screen" => Cat::Window,
        "input" | "mouse" | "self" | "stage" | "time" | "frame" => Cat::Stage,
        _ if name.contains('.') && names.instances.iter().any(|instance| instance == prefix) => {
            Cat::Stage
        }
        _ => Cat::Data,
    }
}

/// Names from the project that slots suggest while typing.
#[derive(Debug, Clone, Default)]
pub struct Names {
    pub instances: Vec<String>,
    pub layers: Vec<String>,
    pub emitters: Vec<String>,
    pub lights: Vec<String>,
    pub links: Vec<String>,
    pub scenes: Vec<String>,
    pub controls: Vec<String>,
    pub poses: Vec<String>,
}

impl Names {
    /// What the project offers a timeline's scripts: q0rg `q0rg_id`'s own
    /// named objects and layers, and the movie's effects.
    pub fn of_project(project: &q0s_format::v2::ProjectV2, q0rg_id: u16) -> Self {
        let mut names = Self::default();
        let mut instances: Vec<String> = project
            .runtime
            .instance_names
            .iter()
            .filter(|(key, _)| key.q0rg_id == q0rg_id)
            .map(|(_, name)| name.clone())
            .collect();
        instances.sort();
        names.instances = instances;
        if let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) {
            names.layers = q0rg
                .layers
                .iter()
                .rev()
                .map(|layer| layer.name.clone())
                .collect();
        }
        names.emitters = project
            .runtime
            .particles
            .emitters
            .iter()
            .map(|emitter| emitter.name.clone())
            .collect();
        names.lights = project
            .runtime
            .lighting
            .setups
            .iter()
            .flat_map(|setup| setup.lights.iter().map(|light| light.name.clone()))
            .collect();
        names.links = project
            .runtime
            .physics
            .links
            .iter()
            .map(|link| link.name.clone())
            .collect();
        names.scenes = project
            .runtime
            .project_graph
            .nodes
            .iter()
            .filter(|node| node.kind != q0s_format::v2::ProjectDependencyKind::Q0lang)
            .map(|node| node.alias.clone())
            .collect();
        if let Some(rig) = q0s_format::rig::rig_for_q0rg(project, q0rg_id) {
            names.controls = rig
                .controls
                .iter()
                .filter(|control| control.public_in_simple)
                .map(|control| control.name.clone())
                .collect();
            names.poses = rig.poses.iter().map(|pose| pose.name.clone()).collect();
        }
        for list in [
            &mut names.emitters,
            &mut names.lights,
            &mut names.links,
            &mut names.scenes,
        ] {
            list.sort();
            list.dedup();
        }
        names
    }

    fn for_arg(&self, arg: Arg) -> Vec<String> {
        match arg {
            Arg::Layer => self.layers.clone(),
            Arg::Thing | Arg::Body => self.instances.clone(),
            Arg::Emitter => self.emitters.clone(),
            Arg::Light => self.lights.clone(),
            Arg::Link => self.links.clone(),
            Arg::Scene => self.scenes.clone(),
            Arg::Control => self.controls.clone(),
            Arg::Pose => self.poses.clone(),
            Arg::Switch => vec!["true".into(), "false".into()],
            Arg::Num | Arg::Text | Arg::Any => Vec::new(),
        }
    }

    fn first(&self, arg: Arg) -> String {
        let fallback = match arg {
            Arg::Layer => "Layer 1",
            Arg::Thing | Arg::Body => "player",
            Arg::Emitter => "sparks",
            Arg::Light => "lamp",
            Arg::Link => "rope1",
            Arg::Scene => "room_2",
            Arg::Control => "look",
            Arg::Pose => "wave",
            _ => "",
        };
        self.for_arg(arg)
            .into_iter()
            .next()
            .unwrap_or_else(|| fallback.to_string())
    }
}

// ------------------------------------------------------------------- paths

/// The way from the program to one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Stmt(usize),
    /// One of the statement lists inside a statement: a body, the else.
    Body(usize),
    /// One of the values inside a statement or a value.
    Slot(usize),
}

type NodePath = Vec<Step>;

enum NodeMut<'a> {
    List(&'a mut Vec<Statement>),
    Stmt(&'a mut Statement),
    Value(&'a mut Value),
}

fn bodies_mut(statement: &mut Statement) -> Vec<&mut Vec<Statement>> {
    match statement {
        Statement::Function {
            body: Some(body), ..
        }
        | Statement::DoBlock { body }
        | Statement::While { body, .. }
        | Statement::ForRange { body, .. } => vec![&mut body.statements],
        Statement::If {
            then_body,
            else_body,
            ..
        } => {
            let mut lists = vec![&mut then_body.statements];
            if let Some(else_body) = else_body {
                lists.push(&mut else_body.statements);
            }
            lists
        }
        _ => Vec::new(),
    }
}

fn values_mut(statement: &mut Statement) -> Vec<&mut Value> {
    match statement {
        Statement::ShellCommand { args, .. }
        | Statement::RigCommand { args, .. }
        | Statement::SceneCommand { args, .. }
        | Statement::PhysicsCommand { args, .. }
        | Statement::ParticlesCommand { args, .. }
        | Statement::LightCommand { args, .. }
        | Statement::LayerCommand { args, .. }
        | Statement::WindowCommand { args, .. } => args.iter_mut().collect(),
        Statement::Assignment { value, .. } | Statement::Expression { value } => vec![value],
        Statement::TimelineSignal { target, .. } => vec![target],
        Statement::If { condition, .. } | Statement::While { condition, .. } => vec![condition],
        Statement::ForRange {
            start, stop, step, ..
        } => vec![start, stop, step],
        Statement::Return { value: Some(value) } => vec![value],
        _ => Vec::new(),
    }
}

fn bodies(statement: &Statement) -> Vec<&[Statement]> {
    match statement {
        Statement::Function {
            body: Some(body), ..
        }
        | Statement::DoBlock { body }
        | Statement::While { body, .. }
        | Statement::ForRange { body, .. } => vec![body.statements.as_slice()],
        Statement::If {
            then_body,
            else_body,
            ..
        } => {
            let mut lists = vec![then_body.statements.as_slice()];
            if let Some(else_body) = else_body {
                lists.push(else_body.statements.as_slice());
            }
            lists
        }
        _ => Vec::new(),
    }
}

fn values(statement: &Statement) -> Vec<&Value> {
    if let Some((_, args)) = command_name(statement) {
        return args.iter().collect();
    }
    match statement {
        Statement::Assignment { value, .. } | Statement::Expression { value } => vec![value],
        Statement::TimelineSignal { target, .. } => vec![target],
        Statement::If { condition, .. } | Statement::While { condition, .. } => vec![condition],
        Statement::ForRange {
            start, stop, step, ..
        } => vec![start, stop, step],
        Statement::Return { value: Some(value) } => vec![value],
        _ => Vec::new(),
    }
}

fn children(value: &Value) -> Vec<&Value> {
    match value {
        Value::Unary { value, .. } => vec![value.as_ref()],
        Value::Binary { left, right, .. } => vec![left.as_ref(), right.as_ref()],
        Value::Call { args, .. } => args.iter().collect(),
        _ => Vec::new(),
    }
}

fn children_mut(value: &mut Value) -> Vec<&mut Value> {
    match value {
        Value::Unary { value, .. } => vec![value.as_mut()],
        Value::Binary { left, right, .. } => vec![left.as_mut(), right.as_mut()],
        Value::Call { args, .. } => args.iter_mut().collect(),
        _ => Vec::new(),
    }
}

fn resolve<'a>(root: &'a mut Vec<Statement>, path: &[Step]) -> Option<NodeMut<'a>> {
    let mut node = NodeMut::List(root);
    for step in path {
        node = match (node, step) {
            (NodeMut::List(list), Step::Stmt(index)) => NodeMut::Stmt(list.get_mut(*index)?),
            (NodeMut::Stmt(statement), Step::Body(index)) => {
                NodeMut::List(bodies_mut(statement).into_iter().nth(*index)?)
            }
            (NodeMut::Stmt(statement), Step::Slot(index)) => {
                NodeMut::Value(values_mut(statement).into_iter().nth(*index)?)
            }
            (NodeMut::Value(value), Step::Slot(index)) => {
                NodeMut::Value(children_mut(value).into_iter().nth(*index)?)
            }
            _ => return None,
        };
    }
    Some(node)
}

fn resolve_value<'a>(root: &'a mut Vec<Statement>, path: &[Step]) -> Option<&'a mut Value> {
    match resolve(root, path)? {
        NodeMut::Value(value) => Some(value),
        _ => None,
    }
}

fn resolve_statement<'a>(root: &'a mut Vec<Statement>, path: &[Step]) -> Option<&'a mut Statement> {
    match resolve(root, path)? {
        NodeMut::Stmt(statement) => Some(statement),
        _ => None,
    }
}

fn resolve_list<'a>(root: &'a mut Vec<Statement>, path: &[Step]) -> Option<&'a mut Vec<Statement>> {
    match resolve(root, path)? {
        NodeMut::List(list) => Some(list),
        _ => None,
    }
}

// ------------------------------------------------------------- what's shown

#[derive(Debug, Clone, PartialEq)]
enum FieldKind {
    /// A number, text, true/false or null in a slot.
    Literal(Arg),
    /// The name a SET or a FOR gives a value to.
    Name,
    FnName,
    Params,
    Handler,
    Callee,
    /// A line the blocks do not know, kept as it was typed.
    Raw,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Operator,
    Variable,
    SetMode,
    Signal,
    Event,
    Library,
    Visibility,
    Listen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    AddElse,
    DropElse,
    AddValue,
    DropValue,
}

enum Part<'e> {
    /// A keyword, white.
    Word(String),
    /// A joining word, dim.
    Dim(String),
    /// A value, found by its path from the block.
    Slot(Vec<Step>, &'e Value, Arg),
    Field(FieldKind, String),
    Pick(PickKind, String),
    Button(ButtonKind),
}

enum Seg<'e> {
    Row(Vec<Part<'e>>),
    Mouth(usize, &'e [Statement]),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    /// A function: its header is the colour of its kind, words in black.
    Head,
    Note,
    Raw,
}

struct Look<'e> {
    cat: Cat,
    style: Style,
    segs: Vec<Seg<'e>>,
}

fn is_literal(value: &Value) -> bool {
    matches!(
        value,
        Value::Number(_) | Value::String(_) | Value::Bool(_) | Value::Null
    )
}

fn literal_text(value: &Value) -> String {
    match value {
        Value::Number(raw) => raw.clone(),
        Value::String(text) => text.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".into(),
        other => printer::print_value(other),
    }
}

fn is_boolean(value: &Value) -> bool {
    match value {
        Value::Binary { op, .. } => matches!(
            op,
            BinaryOp::Equal
                | BinaryOp::NotEqual
                | BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual
                | BinaryOp::And
                | BinaryOp::Or
        ),
        Value::Unary {
            op: UnaryOp::Not, ..
        } => true,
        Value::Call { name, .. } => matches!(
            name.as_str(),
            "q0layer.touches"
                | "q0layer.hit"
                | "q0layer.circle"
                | "q0layer.box"
                | "q0layer.visible"
        ),
        _ => false,
    }
}

/// Splits a catalog label into words and slots for `args`.
fn label_parts<'e>(entry: &Entry, args: &'e [Value], base: &[Step], parts: &mut Vec<Part<'e>>) {
    let mut used = vec![false; args.len()];
    let mut rest = entry.label;
    while let Some(start) = rest.find('{') {
        let words = rest[..start].trim();
        if !words.is_empty() {
            parts.push(Part::Word(words.to_string()));
        }
        let end = rest[start..]
            .find('}')
            .map_or(rest.len(), |end| start + end);
        let name = &rest[start + 1..end];
        if let Some(index) = entry.args.iter().position(|(arg, _)| *arg == name) {
            if let Some(value) = args.get(index) {
                let mut path = base.to_vec();
                path.push(Step::Slot(index));
                parts.push(Part::Slot(path, value, entry.args[index].1));
                used[index] = true;
            }
        }
        rest = rest.get(end + 1..).unwrap_or("");
    }
    if !rest.trim().is_empty() {
        parts.push(Part::Word(rest.trim().to_string()));
    }
    // Values the label has no place for still show.
    for (index, value) in args.iter().enumerate() {
        if !used[index] {
            let mut path = base.to_vec();
            path.push(Step::Slot(index));
            parts.push(Part::Slot(path, value, Arg::Any));
        }
    }
}

fn row(parts: Vec<Part<'_>>) -> Seg<'_> {
    Seg::Row(parts)
}

fn statement_look<'e>(statement: &'e Statement, names: &Names) -> Look<'e> {
    let plain = |cat, parts| Look {
        cat,
        style: Style::Plain,
        segs: vec![row(parts)],
    };
    if let Some(words) = note_text(statement) {
        return Look {
            cat: Cat::Misc,
            style: Style::Note,
            segs: vec![row(vec![Part::Field(FieldKind::Note, words)])],
        };
    }
    if let Some((name, args)) = command_name(statement) {
        let mut parts = Vec::new();
        let cat = match entry(&name) {
            Some(entry) => {
                label_parts(entry, args, &[], &mut parts);
                entry.cat
            }
            None => {
                parts.push(Part::Word(name.clone()));
                for (index, value) in args.iter().enumerate() {
                    parts.push(Part::Slot(vec![Step::Slot(index)], value, Arg::Any));
                }
                Cat::Fx
            }
        };
        return plain(cat, parts);
    }
    match statement {
        Statement::Assignment { name, value } => {
            let cat = name_cat(name, names);
            match value {
                Value::Binary {
                    op: BinaryOp::Add,
                    left,
                    right,
                } if matches!(left.as_ref(), Value::Ident(same) if same == name) => plain(
                    cat,
                    vec![
                        Part::Pick(PickKind::SetMode, "CHANGE".into()),
                        Part::Field(FieldKind::Name, name.clone()),
                        Part::Dim("by".into()),
                        Part::Slot(vec![Step::Slot(0), Step::Slot(1)], right, Arg::Num),
                    ],
                ),
                _ => plain(
                    cat,
                    vec![
                        Part::Pick(PickKind::SetMode, "SET".into()),
                        Part::Field(FieldKind::Name, name.clone()),
                        Part::Dim("to".into()),
                        Part::Slot(vec![Step::Slot(0)], value, Arg::Any),
                    ],
                ),
            }
        }
        Statement::Expression { value } => match value {
            Value::Call { name, args } if entry(name).is_none() => {
                let mut parts = vec![
                    Part::Word("CALL".into()),
                    Part::Field(FieldKind::Callee, name.clone()),
                ];
                for (index, arg) in args.iter().enumerate() {
                    parts.push(Part::Slot(
                        vec![Step::Slot(0), Step::Slot(index)],
                        arg,
                        Arg::Any,
                    ));
                }
                plain(Cat::Func, parts)
            }
            _ => plain(
                Cat::Func,
                vec![
                    Part::Word("EVAL".into()),
                    Part::Slot(vec![Step::Slot(0)], value, Arg::Any),
                ],
            ),
        },
        Statement::TimelineSignal { kind, target } => plain(
            Cat::Stage,
            vec![
                Part::Word("GO TO FRAME".into()),
                Part::Slot(vec![Step::Slot(0)], target, Arg::Num),
                Part::Pick(
                    PickKind::Signal,
                    match kind {
                        TimelineSignal::GoRun => "AND PLAY",
                        TimelineSignal::GoStop => "AND STOP",
                    }
                    .into(),
                ),
            ],
        ),
        Statement::Listen { event, handler } | Statement::Unlisten { event, handler } => {
            let on = matches!(statement, Statement::Listen { .. });
            plain(
                Cat::Func,
                vec![
                    Part::Pick(PickKind::Listen, if on { "ON" } else { "STOP" }.into()),
                    Part::Pick(PickKind::Event, event.clone()),
                    Part::Dim(if on { "call" } else { "calling" }.into()),
                    Part::Field(FieldKind::Handler, handler.clone()),
                ],
            )
        }
        Statement::Function {
            visibility,
            name,
            params,
            body,
        } => {
            let mut segs = vec![row(vec![
                Part::Pick(
                    PickKind::Visibility,
                    match visibility {
                        Visibility::Public => "FUNC",
                        Visibility::Private => "PRIVATE FUNC",
                    }
                    .into(),
                ),
                Part::Field(FieldKind::FnName, name.clone()),
                Part::Field(FieldKind::Params, params.join(" ")),
            ])];
            if let Some(body) = body {
                segs.push(Seg::Mouth(0, &body.statements));
            }
            Look {
                cat: Cat::Func,
                style: Style::Head,
                segs,
            }
        }
        Statement::DoBlock { body } => Look {
            cat: Cat::Flow,
            style: Style::Plain,
            segs: vec![
                row(vec![Part::Word("DO".into())]),
                Seg::Mouth(0, &body.statements),
            ],
        },
        Statement::If {
            condition,
            then_body,
            else_body,
        } => {
            let mut head = vec![
                Part::Word("IF".into()),
                Part::Slot(vec![Step::Slot(0)], condition, Arg::Any),
            ];
            if else_body.is_none() {
                head.push(Part::Button(ButtonKind::AddElse));
            }
            let mut segs = vec![row(head), Seg::Mouth(0, &then_body.statements)];
            if let Some(else_body) = else_body {
                let mut parts = vec![Part::Word("ELSE".into())];
                if else_body.statements.is_empty() {
                    parts.push(Part::Button(ButtonKind::DropElse));
                }
                segs.push(row(parts));
                segs.push(Seg::Mouth(1, &else_body.statements));
            }
            Look {
                cat: Cat::Flow,
                style: Style::Plain,
                segs,
            }
        }
        Statement::While { condition, body } => Look {
            cat: Cat::Flow,
            style: Style::Plain,
            segs: vec![
                row(vec![
                    Part::Word("WHILE".into()),
                    Part::Slot(vec![Step::Slot(0)], condition, Arg::Any),
                ]),
                Seg::Mouth(0, &body.statements),
            ],
        },
        Statement::ForRange {
            name,
            start,
            stop,
            step,
            body,
        } => Look {
            cat: Cat::Flow,
            style: Style::Plain,
            segs: vec![
                row(vec![
                    Part::Word("FOR".into()),
                    Part::Field(FieldKind::Name, name.clone()),
                    Part::Dim("from".into()),
                    Part::Slot(vec![Step::Slot(0)], start, Arg::Num),
                    Part::Dim("below".into()),
                    Part::Slot(vec![Step::Slot(1)], stop, Arg::Num),
                    Part::Dim("step".into()),
                    Part::Slot(vec![Step::Slot(2)], step, Arg::Num),
                ]),
                Seg::Mouth(0, &body.statements),
            ],
        },
        Statement::Break => plain(Cat::Flow, vec![Part::Word("BREAK".into())]),
        Statement::Continue => plain(Cat::Flow, vec![Part::Word("NEXT ROUND".into())]),
        Statement::Return { value } => {
            let mut parts = vec![Part::Word("RETURN".into())];
            match value {
                Some(value) => {
                    parts.push(Part::Slot(vec![Step::Slot(0)], value, Arg::Any));
                    parts.push(Part::Button(ButtonKind::DropValue));
                }
                None => parts.push(Part::Button(ButtonKind::AddValue)),
            }
            plain(Cat::Func, parts)
        }
        Statement::Import { library } => plain(
            Cat::Misc,
            vec![
                Part::Word("IMPORT".into()),
                Part::Pick(PickKind::Library, library.clone()),
            ],
        ),
        Statement::Unknown { text } => Look {
            cat: Cat::Flow,
            style: Style::Raw,
            segs: vec![row(vec![Part::Field(FieldKind::Raw, text.clone())])],
        },
        // Commands were handled above.
        _ => plain(Cat::Misc, vec![Part::Word("?".into())]),
    }
}

fn value_look<'e>(value: &'e Value, names: &Names) -> Look<'e> {
    let plain = |cat, parts| Look {
        cat,
        style: Style::Plain,
        segs: vec![row(parts)],
    };
    match value {
        Value::Ident(name) => plain(
            name_cat(name, names),
            vec![Part::Pick(PickKind::Variable, name.clone())],
        ),
        Value::Unary { op, value } => plain(
            Cat::Math,
            vec![
                Part::Word(
                    match op {
                        UnaryOp::Not => "NOT",
                        UnaryOp::Neg => "-",
                    }
                    .into(),
                ),
                Part::Slot(vec![Step::Slot(0)], value, Arg::Any),
            ],
        ),
        Value::Binary { op, left, right } => plain(
            Cat::Math,
            vec![
                Part::Slot(vec![Step::Slot(0)], left, Arg::Any),
                Part::Pick(PickKind::Operator, printer::operator_text(*op).into()),
                Part::Slot(vec![Step::Slot(1)], right, Arg::Any),
            ],
        ),
        Value::Call { name, args } => match entry(name) {
            Some(entry) => {
                let mut parts = Vec::new();
                label_parts(entry, args, &[], &mut parts);
                plain(entry.cat, parts)
            }
            None => {
                let mut parts = vec![Part::Field(FieldKind::Callee, name.clone())];
                for (index, arg) in args.iter().enumerate() {
                    parts.push(Part::Slot(vec![Step::Slot(index)], arg, Arg::Any));
                }
                plain(Cat::Func, parts)
            }
        },
        Value::Raw(text) => Look {
            cat: Cat::Flow,
            style: Style::Raw,
            segs: vec![row(vec![Part::Field(FieldKind::Raw, text.clone())])],
        },
        literal => plain(
            Cat::Math,
            vec![Part::Field(
                FieldKind::Literal(Arg::Any),
                literal_text(literal),
            )],
        ),
    }
}

// ------------------------------------------------------------------- sizes

/// Sizes at zoom 1.
const ROW: f32 = 28.0;
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;
const SPINE: f32 = 5.0;
const ARM: f32 = 18.0;
const MOUTH_EMPTY: f32 = 16.0;
const FOOT: f32 = 9.0;
const FIELD_H: f32 = 20.0;
const SHADOW: f32 = 3.0;
const GUTTER: f32 = 40.0;
const SNAP: f32 = 34.0;

const CANVAS: Color32 = Color32::from_rgb(10, 10, 12);
const GRID: Color32 = Color32::from_rgb(20, 20, 24);
const SLAB: Color32 = Color32::from_rgb(26, 26, 31);
const EDGE: Color32 = Color32::from_rgb(58, 58, 66);
const INSET: Color32 = Color32::from_rgb(12, 12, 15);
const REPORTER: Color32 = Color32::from_rgb(36, 36, 43);
const INK: Color32 = Color32::from_rgb(236, 236, 236);
const DIM: Color32 = Color32::from_rgb(130, 130, 140);
const RAW: Color32 = Color32::from_rgb(52, 16, 18);
const NOTE_INK: Color32 = Color32::from_rgb(196, 176, 92);
const DROP: Color32 = Color32::from_rgb(198, 255, 0);
const ISSUE: Color32 = Color32::from_rgb(255, 59, 48);

fn literal_color(value: &Value) -> Color32 {
    match value {
        Value::Number(_) => Color32::from_rgb(90, 200, 250),
        Value::String(_) => Color32::from_rgb(255, 214, 10),
        Value::Bool(_) => Color32::from_rgb(255, 107, 214),
        _ => DIM,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape3 {
    Reporter,
    Boolean,
}

/// Something under the pointer.
#[derive(Clone, Debug)]
struct Hit {
    rect: Rect,
    path: NodePath,
    what: HitWhat,
}

#[derive(Clone, Debug, PartialEq)]
enum HitWhat {
    Stmt,
    Value,
    /// A slot a dragged value can replace.
    Slot(Arg),
    Field(FieldKind, String, Vec<String>),
    Pick(PickKind, String),
    Button(ButtonKind),
}

/// Where a dragged statement may go: list `list`, before index `index`.
#[derive(Clone, Debug)]
struct Socket {
    at: Pos2,
    list: NodePath,
    index: usize,
}

struct Layout<'n> {
    painter: Painter,
    zoom: f32,
    font: FontId,
    names: &'n Names,
    hits: Vec<Hit>,
    sockets: Vec<Socket>,
    /// Where each statement starts in the text, in walking order.
    starts: Vec<usize>,
    next_start: usize,
    /// Statement tops and their line numbers, for the gutter.
    numbers: Vec<(f32, usize, Rect)>,
    issues: &'n [usize],
    measuring: bool,
}

impl Layout<'_> {
    fn text_width(&self, text: &str) -> f32 {
        self.painter
            .layout_no_wrap(text.to_string(), self.font.clone(), INK)
            .size()
            .x
    }

    fn part_size(&mut self, part: &Part<'_>) -> Vec2 {
        let z = self.zoom;
        match part {
            Part::Word(text) | Part::Dim(text) => vec2(self.text_width(text), FIELD_H * z),
            Part::Slot(_, value, _) => self.value_size(value),
            Part::Field(kind, text) => {
                let shown = field_display(kind, text);
                let width = self.text_width(&shown) + 12.0 * z;
                match kind {
                    FieldKind::Note => vec2(width.max(60.0 * z), FIELD_H * z),
                    _ => vec2(width.max(24.0 * z), FIELD_H * z),
                }
            }
            Part::Pick(PickKind::Operator, text) => {
                vec2(self.text_width(text) + 16.0 * z, FIELD_H * z)
            }
            Part::Pick(_, text) => vec2(self.text_width(text) + 22.0 * z, FIELD_H * z),
            Part::Button(_) => vec2(16.0 * z, 16.0 * z),
        }
    }

    fn row_size(&mut self, parts: &[Part<'_>]) -> Vec2 {
        let z = self.zoom;
        let mut width = PAD * z;
        let mut height: f32 = ROW * z;
        for part in parts {
            let size = self.part_size(part);
            width += size.x + GAP * z;
            height = height.max(size.y + 8.0 * z);
        }
        vec2(width - GAP * z + PAD * z, height)
    }

    fn value_size(&mut self, value: &Value) -> Vec2 {
        let z = self.zoom;
        if let Value::Ident(name) = value {
            return vec2(self.text_width(name) + 22.0 * z, FIELD_H * z);
        }
        if is_literal(value) {
            let shown = literal_display(value);
            return vec2(
                (self.text_width(&shown) + 12.0 * z).max(26.0 * z),
                FIELD_H * z,
            );
        }
        let look = value_look(value, self.names);
        let Some(Seg::Row(parts)) = look.segs.first() else {
            return vec2(26.0 * z, FIELD_H * z);
        };
        let row = self.row_size(parts);
        let content = row.x - 2.0 * PAD * z;
        let height = row.y - 6.0 * z;
        vec2(content + 2.0 * reporter_side(value, height, z), height)
    }

    /// The size of a block, and the heights of its segments.
    fn look_size(&mut self, look: &Look<'_>) -> (Vec2, Vec<f32>) {
        let z = self.zoom;
        let mut width: f32 = 36.0 * z;
        let mut heights = Vec::new();
        for seg in &look.segs {
            match seg {
                Seg::Row(parts) => {
                    let size = self.row_size(parts);
                    width = width.max(size.x + SPINE * z);
                    heights.push(size.y);
                }
                Seg::Mouth(_, statements) => {
                    let (list_width, list_height) = self.list_size(statements);
                    width = width.max(ARM * z + list_width.min(60.0 * z));
                    heights.push(list_height.max(MOUTH_EMPTY * z));
                }
            }
        }
        let mut height: f32 = heights.iter().sum();
        if matches!(look.segs.last(), Some(Seg::Mouth(..))) {
            height += FOOT * z;
        }
        (vec2(width, height), heights)
    }

    fn list_size(&mut self, statements: &[Statement]) -> (f32, f32) {
        let mut width: f32 = 0.0;
        let mut height = 0.0;
        for statement in statements {
            let look = statement_look(statement, self.names);
            let (size, _) = self.look_size(&look);
            width = width.max(size.x);
            height += size.y;
        }
        (width, height)
    }

    // ------------------------------------------------------------ draw

    fn draw_statement(&mut self, statement: &Statement, at: Pos2, path: &NodePath) -> Vec2 {
        let z = self.zoom;
        let look = statement_look(statement, self.names);
        let (size, heights) = self.look_size(&look);
        let rect = Rect::from_min_size(at, size);
        let line = self.starts.get(self.next_start).copied();
        self.next_start += 1;
        let flagged = line.is_some_and(|line| self.issues.contains(&line));
        if let Some(line) = line {
            self.numbers.push((at.y, line, rect));
        }
        if !self.measuring {
            paint_slab(&self.painter, &look, &heights, rect, z, flagged);
        }
        self.hits.push(Hit {
            rect,
            path: path.clone(),
            what: HitWhat::Stmt,
        });
        let mut y = at.y;
        for (seg, height) in look.segs.iter().zip(&heights) {
            match seg {
                Seg::Row(parts) => {
                    let ink = if look.style == Style::Head {
                        Color32::BLACK
                    } else {
                        INK
                    };
                    self.draw_row(parts, pos2(at.x + SPINE * z, y), *height, path, ink);
                }
                Seg::Mouth(index, statements) => {
                    let mut list = path.clone();
                    list.push(Step::Body(*index));
                    self.draw_list(statements, pos2(at.x + ARM * z, y), &list);
                }
            }
            y += height;
        }
        size
    }

    fn draw_row(
        &mut self,
        parts: &[Part<'_>],
        at: Pos2,
        height: f32,
        path: &NodePath,
        ink: Color32,
    ) {
        let z = self.zoom;
        let mut x = at.x + PAD * z;
        for part in parts {
            let size = self.part_size(part);
            let rect = Rect::from_min_size(pos2(x, at.y + (height - size.y) / 2.0), size);
            match part {
                Part::Word(text) | Part::Dim(text) => {
                    if !self.measuring {
                        let color = match part {
                            Part::Dim(_) if ink == INK => DIM,
                            _ => ink,
                        };
                        self.painter.text(
                            pos2(x, at.y + height / 2.0),
                            Align2::LEFT_CENTER,
                            text,
                            self.font.clone(),
                            color,
                        );
                    }
                }
                Part::Slot(steps, value, arg) => {
                    let mut slot = path.clone();
                    slot.extend(steps.iter().copied());
                    self.draw_slot(value, rect, &slot, *arg);
                }
                Part::Field(kind, text) => {
                    if !self.measuring {
                        self.paint_field(rect, kind, text);
                    }
                    let suggestions = match kind {
                        FieldKind::Name => self
                            .names
                            .instances
                            .iter()
                            .flat_map(|name| [format!("{name}.x"), format!("{name}.y")])
                            .collect(),
                        _ => Vec::new(),
                    };
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Field(kind.clone(), text.clone(), suggestions),
                    });
                }
                Part::Pick(kind, text) => {
                    if !self.measuring {
                        // Operators read as plain symbols; the others as switches.
                        let text_left = if *kind == PickKind::Operator {
                            rect.left() + 2.0 * z
                        } else {
                            self.painter
                                .rect_filled(rect, 0.0, Color32::from_rgb(44, 44, 52));
                            self.painter
                                .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, EDGE));
                            rect.left() + 6.0 * z
                        };
                        // Switches have their own dark box, so their words stay light.
                        let color = if *kind == PickKind::Operator {
                            ink
                        } else {
                            INK
                        };
                        self.painter.text(
                            pos2(text_left, rect.center().y),
                            Align2::LEFT_CENTER,
                            text,
                            self.font.clone(),
                            color,
                        );
                        caret(&self.painter, rect.right_center(), z, DIM);
                    }
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Pick(*kind, text.clone()),
                    });
                }
                Part::Button(kind) => {
                    if !self.measuring {
                        self.painter
                            .rect_filled(rect, 0.0, Color32::from_rgb(44, 44, 52));
                        self.painter
                            .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, EDGE));
                        self.painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            match kind {
                                ButtonKind::DropElse | ButtonKind::DropValue => "−",
                                _ => "+",
                            },
                            FontId::monospace(12.0 * z),
                            INK,
                        );
                    }
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Button(*kind),
                    });
                }
            }
            x += size.x + GAP * z;
        }
    }

    fn draw_slot(&mut self, value: &Value, rect: Rect, path: &NodePath, arg: Arg) {
        if is_literal(value) {
            if !self.measuring {
                self.painter.rect_filled(rect, 0.0, INSET);
                self.painter
                    .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, EDGE));
                self.painter.text(
                    pos2(rect.left() + 6.0 * self.zoom, rect.center().y),
                    Align2::LEFT_CENTER,
                    literal_display(value),
                    self.font.clone(),
                    literal_color(value),
                );
            }
            self.hits.push(Hit {
                rect,
                path: path.clone(),
                what: HitWhat::Slot(arg),
            });
            self.hits.push(Hit {
                rect,
                path: path.clone(),
                what: HitWhat::Field(
                    FieldKind::Literal(arg),
                    literal_text(value),
                    self.names.for_arg(arg),
                ),
            });
            return;
        }
        // A value block can be swapped for another by dropping on it.
        self.hits.push(Hit {
            rect,
            path: path.clone(),
            what: HitWhat::Slot(arg),
        });
        if let Value::Ident(name) = value {
            let z = self.zoom;
            if !self.measuring {
                let color = name_cat(name, self.names).color();
                self.painter.rect_filled(rect, 0.0, REPORTER);
                self.painter.rect_filled(
                    Rect::from_min_size(rect.min, vec2(3.0 * z, rect.height())),
                    0.0,
                    color,
                );
                self.painter
                    .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, darker(color, 0.6)));
                self.painter.text(
                    pos2(rect.left() + 7.0 * z, rect.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    self.font.clone(),
                    INK,
                );
                caret(&self.painter, rect.right_center(), z, DIM);
            }
            self.hits.push(Hit {
                rect,
                path: path.clone(),
                what: HitWhat::Value,
            });
            self.hits.push(Hit {
                rect,
                path: path.clone(),
                what: HitWhat::Pick(PickKind::Variable, name.clone()),
            });
            return;
        }
        self.draw_reporter(value, rect, path);
    }

    fn draw_reporter(&mut self, value: &Value, rect: Rect, path: &NodePath) {
        let z = self.zoom;
        let look = value_look(value, self.names);
        let shape = if is_boolean(value) {
            Shape3::Boolean
        } else {
            Shape3::Reporter
        };
        if !self.measuring {
            paint_reporter(&self.painter, look.cat, look.style, shape, rect);
        }
        self.hits.push(Hit {
            rect,
            path: path.clone(),
            what: HitWhat::Value,
        });
        let side = reporter_side(value, rect.height(), z);
        if let Some(Seg::Row(parts)) = look.segs.first() {
            self.draw_row(
                parts,
                pos2(rect.left() + side - PAD * z, rect.top()),
                rect.height(),
                path,
                INK,
            );
        }
    }

    fn paint_field(&self, rect: Rect, kind: &FieldKind, text: &str) {
        let z = self.zoom;
        let (fill, color) = match kind {
            FieldKind::Note => (Color32::TRANSPARENT, NOTE_INK),
            FieldKind::Raw => (INSET, Color32::from_rgb(255, 140, 130)),
            FieldKind::Callee | FieldKind::FnName | FieldKind::Handler => {
                (INSET, Color32::from_rgb(214, 160, 255))
            }
            _ => (INSET, INK),
        };
        if fill != Color32::TRANSPARENT {
            self.painter.rect_filled(rect, 0.0, fill);
            self.painter
                .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, EDGE));
        }
        self.painter.text(
            pos2(rect.left() + 6.0 * z, rect.center().y),
            Align2::LEFT_CENTER,
            field_display(kind, text),
            self.font.clone(),
            color,
        );
    }

    /// Statements one under the other; gives back the height.
    fn draw_list(&mut self, statements: &[Statement], at: Pos2, list: &NodePath) -> f32 {
        let mut y = at.y;
        self.sockets.push(Socket {
            at: pos2(at.x, y),
            list: list.clone(),
            index: 0,
        });
        for (index, statement) in statements.iter().enumerate() {
            let mut path = list.clone();
            path.push(Step::Stmt(index));
            let size = self.draw_statement(statement, pos2(at.x, y), &path);
            y += size.y;
            self.sockets.push(Socket {
                at: pos2(at.x, y),
                list: list.clone(),
                index: index + 1,
            });
        }
        y - at.y
    }
}

/// The room a value block keeps on each side of what it holds: true/false
/// questions need space for their pointed ends.
fn reporter_side(value: &Value, height: f32, z: f32) -> f32 {
    if is_boolean(value) {
        height * 0.5 + 2.0 * z
    } else {
        6.0 * z
    }
}

/// A small downward triangle: this opens a list.
fn caret(painter: &Painter, right: Pos2, z: f32, color: Color32) {
    let half = 3.0 * z;
    let tip = pos2(right.x - 2.0 * z - half, right.y + half * 0.7);
    painter.add(Shape::convex_polygon(
        vec![
            pos2(tip.x - half, tip.y - half * 1.4),
            pos2(tip.x + half, tip.y - half * 1.4),
            tip,
        ],
        color,
        Stroke::NONE,
    ));
}

fn field_display(kind: &FieldKind, text: &str) -> String {
    match kind {
        FieldKind::Note => format!("# {text}"),
        FieldKind::Params if text.is_empty() => "( )".into(),
        FieldKind::Params => format!("( {text} )"),
        _ if text.is_empty() => " ".into(),
        _ => text.to_string(),
    }
}

fn literal_display(value: &Value) -> String {
    match value {
        Value::String(text) => printer::quote(text),
        other => literal_text(other),
    }
}

fn darker(color: Color32, amount: f32) -> Color32 {
    Color32::from_rgb(
        (f32::from(color.r()) * amount) as u8,
        (f32::from(color.g()) * amount) as u8,
        (f32::from(color.b()) * amount) as u8,
    )
}

/// The pieces a block is made of: its rows and foot across its width, and
/// the arm down its left beside each mouth.
fn slab_pieces(look: &Look<'_>, heights: &[f32], rect: Rect, z: f32) -> Vec<(Rect, bool)> {
    let mut pieces = Vec::new();
    let mut y = rect.top();
    for (seg, height) in look.segs.iter().zip(heights) {
        match seg {
            Seg::Row(_) => pieces.push((
                Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), *height)),
                true,
            )),
            Seg::Mouth(..) => pieces.push((
                Rect::from_min_size(pos2(rect.left(), y), vec2(ARM * z, *height)),
                false,
            )),
        }
        y += height;
    }
    if y < rect.bottom() - 0.5 {
        pieces.push((
            Rect::from_min_max(pos2(rect.left(), y), rect.right_bottom()),
            false,
        ));
    }
    pieces
}

/// A statement block: square, a hard black shadow, a coloured spine.
fn paint_slab(
    painter: &Painter,
    look: &Look<'_>,
    heights: &[f32],
    rect: Rect,
    z: f32,
    flagged: bool,
) {
    let color = look.cat.color();
    if look.style == Style::Note {
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(2.0 * z, rect.height())),
            0.0,
            darker(NOTE_INK, 0.7),
        );
        return;
    }
    let pieces = slab_pieces(look, heights, rect, z);
    for (piece, _) in &pieces {
        painter.rect_filled(
            piece.translate(vec2(SHADOW * z, SHADOW * z)),
            0.0,
            Color32::BLACK,
        );
    }
    for (piece, header) in &pieces {
        let fill = match look.style {
            Style::Head if *header => color,
            Style::Raw => RAW,
            _ => SLAB,
        };
        painter.rect_filled(*piece, 0.0, fill);
    }
    // The spine runs the whole height, mouths included.
    painter.rect_filled(
        Rect::from_min_size(rect.min, vec2(SPINE * z, rect.height())),
        0.0,
        if look.style == Style::Raw {
            ISSUE
        } else {
            color
        },
    );
    let edge = if flagged { ISSUE } else { EDGE };
    let width = if flagged { 2.0_f32 } else { 1.0_f32 };
    for (piece, _) in &pieces {
        painter.rect_stroke(*piece, 0.0, Stroke::new(width, edge));
    }
}

fn paint_reporter(painter: &Painter, cat: Cat, style: Style, shape: Shape3, rect: Rect) {
    let color = cat.color();
    let fill = if style == Style::Raw { RAW } else { REPORTER };
    match shape {
        Shape3::Boolean => {
            let inset = rect.height() / 2.0;
            let points = vec![
                pos2(rect.left(), rect.center().y),
                pos2(rect.left() + inset, rect.top()),
                pos2(rect.right() - inset, rect.top()),
                pos2(rect.right(), rect.center().y),
                pos2(rect.right() - inset, rect.bottom()),
                pos2(rect.left() + inset, rect.bottom()),
            ];
            painter.add(Shape::convex_polygon(
                points,
                fill,
                Stroke::new(1.5_f32, color),
            ));
        }
        _ => {
            painter.rect_filled(rect, 0.0, fill);
            painter.rect_filled(
                Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                0.0,
                color,
            );
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, darker(color, 0.75)));
        }
    }
}

// ------------------------------------------------------------- the editor

/// What is being dragged.
#[derive(Clone)]
enum Payload {
    Stmts(Vec<Statement>),
    Value(Value),
}

#[derive(Clone)]
struct Drag {
    payload: Payload,
    /// The pointer's place on the dragged block.
    grab: Vec2,
    /// Where a value came from, to go back to when let go over nothing.
    origin: Option<NodePath>,
    /// Libraries the block uses, imported when it lands.
    imports: Vec<&'static str>,
}

#[derive(Clone)]
struct Editing {
    path: NodePath,
    kind: FieldKind,
    text: String,
    rect: Rect,
    suggestions: Vec<String>,
    fresh: bool,
}

#[derive(Clone)]
struct Popup {
    path: NodePath,
    kind: PickKind,
    at: Pos2,
}

#[derive(Clone)]
struct PendingPress {
    hit: Hit,
    start: Pos2,
}

#[derive(Clone)]
enum DropTarget {
    Socket(Socket),
    Slot(NodePath),
}

#[derive(Clone)]
pub struct BlockEditor {
    pan: Vec2,
    zoom: f32,
    palette: Cat,
    drag: Option<Drag>,
    editing: Option<Editing>,
    popup: Option<Popup>,
    pending: Option<PendingPress>,
    snaps: Vec<(Rect, Instant)>,
    /// The program as last read, and the source it was read from.
    cache: Option<(String, Result<Vec<Statement>, String>)>,
}

impl Default for BlockEditor {
    fn default() -> Self {
        Self {
            pan: vec2(0.0, 12.0),
            zoom: 1.0,
            palette: Cat::Flow,
            drag: None,
            editing: None,
            popup: None,
            pending: None,
            snaps: Vec::new(),
            cache: None,
        }
    }
}

/// A value to start a slot with.
fn default_arg(name: &str, arg: Arg, names: &Names) -> String {
    match arg {
        Arg::Num => match name {
            "count" => "20",
            "radius" => "40",
            "alpha" | "value" | "weight" | "t" => "1",
            "width" => "640",
            "height" => "480",
            "red" | "green" | "blue" => "255",
            "degrees" => "90",
            _ => "0",
        }
        .into(),
        Arg::Switch => "true".into(),
        Arg::Text | Arg::Any => "\"hello\"".into(),
        other => printer::quote(&names.first(other)),
    }
}

fn entry_source(entry: &Entry, names: &Names) -> String {
    let args = entry
        .args
        .iter()
        .map(|(name, arg)| default_arg(name, *arg, names))
        .collect::<Vec<_>>();
    if entry.name.ends_with('!') {
        if args.is_empty() {
            entry.name.to_string()
        } else {
            format!("{} {}", entry.name, args.join(", "))
        }
    } else {
        format!("{}({})", entry.name, args.join(", "))
    }
}

/// Names a program sets, and the functions it defines.
fn program_names(statements: &[Statement]) -> (Vec<String>, Vec<(String, usize)>) {
    fn walk(
        statements: &[Statement],
        variables: &mut std::collections::BTreeSet<String>,
        functions: &mut Vec<(String, usize)>,
    ) {
        for statement in statements {
            match statement {
                Statement::Assignment { name, .. } => {
                    variables.insert(name.clone());
                }
                Statement::ForRange { name, .. } => {
                    variables.insert(name.clone());
                }
                Statement::Function { name, params, .. } => {
                    if !functions.iter().any(|(known, _)| known == name) {
                        functions.push((name.clone(), params.len()));
                    }
                    variables.extend(params.iter().cloned());
                }
                _ => {}
            }
            for body in bodies(statement) {
                walk(body, variables, functions);
            }
        }
    }
    let mut variables = std::collections::BTreeSet::new();
    let mut functions = Vec::new();
    walk(statements, &mut variables, &mut functions);
    (variables.into_iter().collect(), functions)
}

/// The blocks a palette page offers, as code: (source, is a value).
fn palette_sources(
    cat: Cat,
    names: &Names,
    variables: &[String],
    functions: &[(String, usize)],
) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let statements = |list: &[&str], out: &mut Vec<(String, bool)>| {
        out.extend(list.iter().map(|source| (source.to_string(), false)));
    };
    let bare_thing = names.first(Arg::Thing);
    match cat {
        Cat::Flow => statements(
            &[
                "if true {\n}",
                "if true {\n} else {\n}",
                "while false {\n}",
                "for i in range(10) {\n}",
                "do! {\n}",
                "break",
                "continue",
            ],
            &mut out,
        ),
        Cat::Data => {
            let first = variables
                .iter()
                .find(|name| !name.contains('.'))
                .cloned()
                .unwrap_or_else(|| "score".into());
            out.push((format!("{first} = 0"), false));
            out.push((format!("{first} = {first} + 1"), false));
            for name in variables {
                out.push((name.clone(), true));
            }
            out.push(("\"text\"".into(), true));
            out.push(("true".into(), true));
            out.push(("null".into(), true));
        }
        Cat::Math => {
            for source in [
                "1 + 1",
                "1 - 1",
                "2 * 3",
                "6 / 2",
                "7 % 3",
                "2 ^ 3",
                "-x",
                "1 < 2",
                "1 > 2",
                "1 <= 2",
                "1 >= 2",
                "1 == 1",
                "1 != 2",
                "true and true",
                "true or false",
                "not true",
                "pi",
                "tau",
            ] {
                out.push((source.into(), true));
            }
            for entry in ENTRIES.iter().filter(|entry| entry.cat == Cat::Math) {
                out.push((entry_source(entry, names), true));
            }
        }
        Cat::Func => {
            out.push(("pb func my_block value {\n}".into(), false));
            out.push(("return 0".into(), false));
            for (name, count) in functions {
                let args = vec!["0"; *count].join(", ");
                out.push((format!("{name}({args})"), false));
                out.push((format!("{name}({args})"), true));
            }
            let handler = functions
                .first()
                .map_or("on_click".to_string(), |(name, _)| name.clone());
            out.push((format!("listen time.tick {handler}"), false));
            out.push((format!("listen mouse.click {handler}"), false));
            out.push((format!("xlisten mouse.click {handler}"), false));
        }
        Cat::Stage => {
            statements(&["gorun! 0", "gostop! 0"], &mut out);
            out.push((format!("{bare_thing}.x = {bare_thing}.x + 5"), false));
            out.push((format!("{bare_thing}.y = {bare_thing}.y + 5"), false));
            out.push((
                entry_source(entry("q0scene.switch!").expect("entry"), names),
                false,
            ));
            for name in &names.instances {
                for property in [
                    "x", "y", "left", "right", "top", "bottom", "width", "height",
                ] {
                    out.push((format!("{name}.{property}"), true));
                }
            }
            for name in [
                "input.left",
                "input.right",
                "input.jump",
                "input.jump_pressed",
                "mouse.x",
                "mouse.y",
                "mouse.inside",
                "stage.width",
                "stage.height",
                "time.dt",
                "self.x",
                "self.y",
                "self.frame",
                "self.playing",
                "self.hovered",
                "frame",
            ] {
                out.push((name.into(), true));
            }
        }
        Cat::Layer | Cat::Window | Cat::Fx => {
            for entry in ENTRIES.iter().filter(|entry| entry.cat == cat) {
                out.push((entry_source(entry, names), !entry.name.ends_with('!')));
            }
            if cat == Cat::Window {
                for name in [
                    "window.x",
                    "window.y",
                    "window.width",
                    "window.height",
                    "window.fullscreen",
                    "window.ui",
                    "screen.width",
                    "screen.height",
                ] {
                    out.push((name.into(), true));
                }
                out.push(("window.x = window.x + 10".into(), false));
            }
        }
        Cat::Misc => {
            out.push(("__note!".into(), false));
            for library in BUILTIN_LIBRARIES
                .iter()
                .filter(|library| library.name != "q0.core" && library.name != "q0shell")
            {
                out.push((format!("import {}", library.name), false));
            }
            out.push(("q0shell.print! \"hello\"".into(), false));
        }
    }
    out
}

/// A fresh copy of a palette block.
fn palette_payload(source: &str, value: bool) -> Option<Payload> {
    if source == NOTE {
        return Some(Payload::Stmts(vec![note("a note")]));
    }
    if value {
        return parser::parse_expression(source).ok().map(Payload::Value);
    }
    let program = parser::parse(source);
    (program.diagnostics.is_empty() && !program.statements.is_empty())
        .then_some(Payload::Stmts(program.statements))
}

/// Libraries the names in a payload come from.
fn payload_imports(payload: &Payload) -> Vec<&'static str> {
    fn walk_value(value: &Value, out: &mut Vec<&'static str>) {
        match value {
            Value::Ident(name) | Value::Call { name, .. } => out.extend(library_of(name)),
            _ => {}
        }
        for child in children(value) {
            walk_value(child, out);
        }
    }
    fn statements(list: &[Statement], out: &mut Vec<&'static str>) {
        for statement in list {
            if let Some((name, _)) = command_name(statement) {
                out.extend(library_of(&name));
            }
            match statement {
                Statement::Assignment { name, .. } => out.extend(library_of(name)),
                Statement::Listen { event, .. } | Statement::Unlisten { event, .. } => {
                    out.push(if event == "time.tick" {
                        "q0.time"
                    } else {
                        "q0.mouse"
                    })
                }
                Statement::TimelineSignal { .. } => out.push("q0.timeline"),
                _ => {}
            }
            for item in values(statement) {
                walk_value(item, out);
            }
            for body in bodies(statement) {
                statements(body, out);
            }
        }
    }
    let mut out = Vec::new();
    match payload {
        Payload::Stmts(list) => statements(list, &mut out),
        Payload::Value(item) => walk_value(item, &mut out),
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Puts `import library` at the top of the program unless it is there.
fn ensure_imports(program: &mut Vec<Statement>, libraries: &[&'static str]) {
    for library in libraries {
        let present = program
            .iter()
            .any(|statement| matches!(statement, Statement::Import { library: known } if known == library));
        if !present {
            let at = program
                .iter()
                .take_while(|statement| {
                    matches!(statement, Statement::Import { .. }) || note_text(statement).is_some()
                })
                .count();
            program.insert(
                at,
                Statement::Import {
                    library: (*library).to_string(),
                },
            );
        }
    }
}

/// Shows `source` as blocks in a block editor kept under `id`. Edits write
/// `source` anew; true when they did. `issues` are the lines the code's
/// checks complain about.
pub fn show(
    ui: &mut Ui,
    id: egui::Id,
    source: &mut String,
    names: &Names,
    issues: &[usize],
) -> bool {
    let mut editor = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<BlockEditor>(id))
        .unwrap_or_default();
    let changed = editor.show(ui, source, names, issues);
    ui.ctx().data_mut(|data| data.insert_temp(id, editor));
    changed
}

/// The SIMPLE / PRO switch every q0lang editor shows. True when it flipped.
pub fn mode_switch(ui: &mut Ui, simple: &mut bool) -> bool {
    let mut flipped = false;
    for (value, label, hint) in [
        (true, "SIMPLE", "The code as blocks"),
        (false, "PRO", "The code as text"),
    ] {
        let text = egui::RichText::new(label).monospace().strong();
        if ui
            .selectable_label(*simple == value, text)
            .on_hover_text(hint)
            .clicked()
            && *simple != value
        {
            *simple = value;
            flipped = true;
        }
    }
    flipped
}

impl BlockEditor {
    fn program_for(&mut self, source: &str) -> Result<Vec<Statement>, String> {
        if let Some((cached, program)) = &self.cache {
            if cached == source {
                return program.clone();
            }
        }
        let program = read(source);
        self.cache = Some((source.to_string(), program.clone()));
        program
    }

    /// Shows `source` as blocks. Edits write `source` anew; true when they
    /// did. `issues` are the lines the code's checks complain about.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        source: &mut String,
        names: &Names,
        issues: &[usize],
    ) -> bool {
        let available = ui.available_rect_before_wrap();
        ui.allocate_rect(available, Sense::hover());
        let mut program = match self.program_for(source) {
            Ok(program) => program,
            Err(message) => {
                let painter = ui.painter_at(available);
                painter.rect_filled(available, 0.0, CANVAS);
                let card = Rect::from_min_size(
                    available.min + vec2(24.0, 24.0),
                    vec2((available.width() - 48.0).max(120.0), 92.0),
                );
                painter.rect_filled(card.translate(vec2(4.0, 4.0)), 0.0, Color32::BLACK);
                painter.rect_filled(card, 0.0, RAW);
                painter.rect_filled(
                    Rect::from_min_size(card.min, vec2(SPINE, card.height())),
                    0.0,
                    ISSUE,
                );
                painter.text(
                    card.min + vec2(18.0, 16.0),
                    Align2::LEFT_TOP,
                    "THIS CODE DOES NOT READ AS BLOCKS",
                    FontId::monospace(14.0),
                    INK,
                );
                painter.text(
                    card.min + vec2(18.0, 42.0),
                    Align2::LEFT_TOP,
                    &message,
                    FontId::monospace(12.0),
                    Color32::from_rgb(255, 160, 150),
                );
                painter.text(
                    card.min + vec2(18.0, 64.0),
                    Align2::LEFT_TOP,
                    "Fix it in PRO, then come back.",
                    FontId::monospace(12.0),
                    DIM,
                );
                return false;
            }
        };
        let (_, starts) = write(&program);
        let (variables, functions) = program_names(&program);
        let mut changed = false;

        let palette_width = 320.0_f32.min(available.width() * 0.42);
        let palette_rect =
            Rect::from_min_size(available.min, vec2(palette_width, available.height()));
        let canvas_rect =
            Rect::from_min_max(pos2(palette_rect.right(), available.top()), available.max);
        let canvas = ui.interact(
            canvas_rect,
            ui.id().with("q0lang_block_canvas"),
            Sense::click_and_drag(),
        );
        ui.interact(
            palette_rect,
            ui.id().with("q0lang_block_palette"),
            Sense::click_and_drag(),
        );
        let pointer = ui.ctx().pointer_interact_pos();
        let z = self.zoom;
        let font = FontId::new(12.5 * z, FontFamily::Monospace);

        if canvas.hovered() {
            let (scroll, zoom_delta) = ui
                .ctx()
                .input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
            if zoom_delta != 1.0 {
                let old = self.zoom;
                self.zoom = (self.zoom * zoom_delta).clamp(0.5, 2.0);
                if let Some(pointer) = pointer {
                    let anchor = pointer - canvas_rect.min - self.pan;
                    self.pan -= anchor * (self.zoom / old - 1.0);
                }
            } else {
                self.pan += scroll;
            }
        }

        // ---------------------------------------------------- the canvas
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 0.0, CANVAS);
        let spacing = 32.0 * z;
        let start = canvas_rect.min
            + vec2(
                self.pan.x.rem_euclid(spacing),
                self.pan.y.rem_euclid(spacing),
            );
        let mut gx = start.x;
        while gx < canvas_rect.right() {
            painter.line_segment(
                [pos2(gx, canvas_rect.top()), pos2(gx, canvas_rect.bottom())],
                Stroke::new(1.0_f32, GRID),
            );
            gx += spacing;
        }
        let mut gy = start.y;
        while gy < canvas_rect.bottom() {
            painter.line_segment(
                [pos2(canvas_rect.left(), gy), pos2(canvas_rect.right(), gy)],
                Stroke::new(1.0_f32, GRID),
            );
            gy += spacing;
        }
        let gutter = Rect::from_min_size(canvas_rect.min, vec2(GUTTER * z, canvas_rect.height()));
        painter.rect_filled(gutter, 0.0, Color32::from_rgb(6, 6, 8));
        painter.line_segment(
            [gutter.right_top(), gutter.right_bottom()],
            Stroke::new(1.0_f32, EDGE),
        );
        let origin =
            pos2(gutter.right() + 14.0 * z, canvas_rect.top()) + vec2(self.pan.x, self.pan.y);
        let mut layout = Layout {
            painter: painter.clone(),
            zoom: z,
            font: font.clone(),
            names,
            hits: Vec::new(),
            sockets: Vec::new(),
            starts,
            next_start: 0,
            numbers: Vec::new(),
            issues,
            measuring: false,
        };
        let height = layout.draw_list(&program, origin, &Vec::new());
        // Line numbers down the gutter, the way the text has them.
        let gutter_painter = ui.painter_at(gutter);
        for (top, line, rect) in &layout.numbers {
            let flagged = issues.contains(line);
            gutter_painter.text(
                pos2(gutter.right() - 6.0 * z, top + 14.0 * z),
                Align2::RIGHT_CENTER,
                line.to_string(),
                FontId::monospace(10.5 * z),
                if flagged { ISSUE } else { DIM },
            );
            if flagged {
                gutter_painter.rect_filled(
                    Rect::from_min_size(
                        pos2(gutter.left(), *top),
                        vec2(3.0, rect.height().min(ROW * z)),
                    ),
                    0.0,
                    ISSUE,
                );
            }
        }
        if program.is_empty() {
            painter.text(
                origin + vec2(0.0, 6.0 * z),
                Align2::LEFT_TOP,
                "EMPTY. DRAG BLOCKS IN FROM THE LEFT.",
                FontId::monospace(13.0 * z),
                DIM,
            );
        }
        // Keep the column on screen.
        let content_bottom = origin.y + height;
        if content_bottom < canvas_rect.top() + 40.0 {
            self.pan.y += canvas_rect.top() + 40.0 - content_bottom;
        }
        self.pan.y = self.pan.y.min(12.0);
        self.pan.x = self.pan.x.min(0.0);

        // Snap flashes.
        let now = Instant::now();
        self.snaps
            .retain(|(_, born)| now.duration_since(*born).as_secs_f32() < 0.3);
        for (rect, born) in &self.snaps {
            let t = now.duration_since(*born).as_secs_f32() / 0.3;
            painter.rect_stroke(
                rect.expand(2.0 + 5.0 * t),
                0.0,
                Stroke::new(3.0 * (1.0 - t), DROP.gamma_multiply(1.0 - t)),
            );
        }
        if !self.snaps.is_empty() {
            ui.ctx().request_repaint();
        }

        // ---------------------------------------------------- the palette
        let palette_hits = self.palette(ui, palette_rect, names, &variables, &functions);

        // --------------------------------------------------------- input
        let (primary_pressed, primary_released, primary_down) = ui.ctx().input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_released(),
                input.pointer.primary_down(),
            )
        });
        let mut press_hit: Option<Hit> = None;
        if primary_pressed && self.drag.is_none() && self.editing.is_none() && self.popup.is_none()
        {
            if let Some(pointer) = pointer {
                if palette_rect.contains(pointer) {
                    if let Some((rect, source, value)) = palette_hits
                        .iter()
                        .rev()
                        .find(|(rect, ..)| rect.contains(pointer))
                    {
                        if let Some(payload) = palette_payload(source, *value) {
                            let imports = payload_imports(&payload);
                            self.drag = Some(Drag {
                                payload,
                                grab: pointer - rect.min,
                                origin: None,
                                imports,
                            });
                        }
                    }
                } else if canvas_rect.contains(pointer) {
                    press_hit = layout
                        .hits
                        .iter()
                        .rev()
                        .find(|hit| {
                            hit.rect.contains(pointer) && !matches!(hit.what, HitWhat::Slot(_))
                        })
                        .cloned();
                }
            }
        }
        if let (Some(hit), Some(start)) = (press_hit, pointer) {
            self.pending = Some(PendingPress { hit, start });
        }
        if let Some(press) = self.pending.clone() {
            let moved = pointer.is_some_and(|pointer| (pointer - press.start).length() > 4.0);
            if primary_down && moved {
                self.pending = None;
                if let Some(drag) = self.begin_drag(&mut program, &press.hit, press.start) {
                    self.drag = Some(drag);
                    changed = true;
                }
            } else if primary_released {
                self.pending = None;
                changed |= self.click(&mut program, &press.hit);
            } else if !primary_down {
                self.pending = None;
            }
        }

        if let (Some(drag), Some(pointer)) = (&self.drag, pointer) {
            let at = pointer - drag.grab;
            let over_palette = palette_rect.contains(pointer);
            let target = if over_palette {
                None
            } else {
                drop_target(drag, at, pointer, &layout, canvas_rect)
            };
            match &target {
                Some(DropTarget::Socket(socket)) => {
                    let width = 160.0 * z;
                    painter.rect_filled(
                        Rect::from_min_size(socket.at - vec2(0.0, 2.0 * z), vec2(width, 4.0 * z)),
                        0.0,
                        DROP,
                    );
                }
                Some(DropTarget::Slot(path)) => {
                    if let Some(hit) = layout
                        .hits
                        .iter()
                        .find(|hit| hit.path == *path && matches!(hit.what, HitWhat::Slot(_)))
                    {
                        painter.rect_stroke(hit.rect.expand(2.0), 0.0, Stroke::new(2.5_f32, DROP));
                    }
                }
                None => {}
            }
            if over_palette {
                let trash = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    ui.id().with("q0lang_block_trash"),
                ));
                trash.rect_filled(
                    palette_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(255, 59, 48, 60),
                );
                trash.text(
                    palette_rect.center(),
                    Align2::CENTER_CENTER,
                    "DROP TO DELETE",
                    FontId::monospace(14.0),
                    INK,
                );
            }
            let mut preview = Layout {
                painter: ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    ui.id().with("q0lang_block_dragged"),
                )),
                zoom: z,
                font: font.clone(),
                names,
                hits: Vec::new(),
                sockets: Vec::new(),
                starts: Vec::new(),
                next_start: 0,
                numbers: Vec::new(),
                issues: &[],
                measuring: false,
            };
            draw_payload(&mut preview, &drag.payload, at);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.ctx().request_repaint();
            if primary_released {
                let drag = self.drag.take().expect("dragging");
                if !over_palette {
                    if let Some(rect) = place(&mut program, drag, target, at, z) {
                        self.snaps.push((rect, Instant::now()));
                    }
                }
                changed = true;
            }
        } else if self.drag.is_some() && !primary_down {
            self.drag = None;
        }

        // Pan by dragging the empty canvas.
        if canvas.dragged() && self.drag.is_none() && self.pending.is_none() {
            self.pan += canvas.drag_delta();
        }

        // Delete takes away the statement under the pointer.
        if self.editing.is_none() && self.drag.is_none() && canvas.hovered() {
            let delete = ui.ctx().input(|input| input.key_pressed(egui::Key::Delete));
            if delete {
                if let Some(path) = pointer.and_then(|pointer| {
                    layout
                        .hits
                        .iter()
                        .rev()
                        .find(|hit| hit.what == HitWhat::Stmt && hit.rect.contains(pointer))
                        .map(|hit| hit.path.clone())
                }) {
                    if let (Some(Step::Stmt(index)), Some((_, list))) =
                        (path.last().copied(), path.split_last())
                    {
                        if let Some(list) = resolve_list(&mut program, list) {
                            if index < list.len() {
                                list.remove(index);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        changed |= self.edit_field(ui, &mut program);
        changed |= self.show_popup(ui, &mut program, &variables);

        // Zoom buttons.
        let tools = Rect::from_min_size(
            canvas_rect.right_top() + vec2(-112.0, 8.0),
            vec2(104.0, 22.0),
        );
        ui.allocate_ui_at_rect(tools, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("−").on_hover_text("Smaller").clicked() {
                    self.zoom = (self.zoom / 1.15).max(0.5);
                }
                if ui
                    .small_button("1:1")
                    .on_hover_text("Normal size")
                    .clicked()
                {
                    self.zoom = 1.0;
                }
                if ui.small_button("+").on_hover_text("Bigger").clicked() {
                    self.zoom = (self.zoom * 1.15).min(2.0);
                }
            });
        });

        if changed {
            let (text, _) = write(&program);
            *source = text;
            self.cache = Some((source.clone(), Ok(program)));
        }
        changed
    }

    /// Takes what was grabbed out of the program, to carry it.
    fn begin_drag(&mut self, program: &mut Vec<Statement>, hit: &Hit, start: Pos2) -> Option<Drag> {
        let grab = start - hit.rect.min;
        match &hit.what {
            HitWhat::Stmt => {
                let (Some(Step::Stmt(index)), Some((_, list))) =
                    (hit.path.last().copied(), hit.path.split_last())
                else {
                    return None;
                };
                let list = resolve_list(program, list)?;
                if index >= list.len() {
                    return None;
                }
                let statement = list.remove(index);
                Some(Drag {
                    payload: Payload::Stmts(vec![statement]),
                    grab,
                    origin: None,
                    imports: Vec::new(),
                })
            }
            HitWhat::Value => {
                let value = resolve_value(program, &hit.path)?;
                let taken = std::mem::replace(value, Value::Null);
                Some(Drag {
                    payload: Payload::Value(taken),
                    grab,
                    origin: Some(hit.path.clone()),
                    imports: Vec::new(),
                })
            }
            HitWhat::Pick(PickKind::Variable, _) => {
                // A variable inside a value block drags the variable out.
                let value = resolve_value(program, &hit.path)?;
                let taken = std::mem::replace(value, Value::Null);
                Some(Drag {
                    payload: Payload::Value(taken),
                    grab,
                    origin: Some(hit.path.clone()),
                    imports: Vec::new(),
                })
            }
            _ => None,
        }
    }

    /// A click without a drag: edit a field, open a picker, press a button.
    fn click(&mut self, program: &mut Vec<Statement>, hit: &Hit) -> bool {
        match &hit.what {
            HitWhat::Field(kind, text, suggestions) => {
                self.editing = Some(Editing {
                    path: hit.path.clone(),
                    kind: kind.clone(),
                    text: match kind {
                        FieldKind::Literal(_) => text.clone(),
                        _ => text.clone(),
                    },
                    rect: hit.rect,
                    suggestions: suggestions.clone(),
                    fresh: true,
                });
                false
            }
            HitWhat::Pick(kind, _) => {
                self.popup = Some(Popup {
                    path: hit.path.clone(),
                    kind: *kind,
                    at: hit.rect.left_bottom(),
                });
                false
            }
            HitWhat::Button(kind) => {
                let Some(statement) = resolve_statement(program, &hit.path) else {
                    return false;
                };
                match (kind, statement) {
                    (ButtonKind::AddElse, Statement::If { else_body, .. }) => {
                        *else_body = Some(Block::default());
                    }
                    (ButtonKind::DropElse, Statement::If { else_body, .. }) => {
                        if else_body
                            .as_ref()
                            .is_some_and(|body| body.statements.is_empty())
                        {
                            *else_body = None;
                        }
                    }
                    (ButtonKind::AddValue, Statement::Return { value }) => {
                        *value = Some(Value::Number("0".into()));
                    }
                    (ButtonKind::DropValue, Statement::Return { value }) => *value = None,
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    /// The text box over a field being edited, with suggestions under it.
    fn edit_field(&mut self, ui: &mut Ui, program: &mut Vec<Statement>) -> bool {
        let Some(editing) = self.editing.as_mut() else {
            return false;
        };
        let id = ui.id().with("q0lang_block_field");
        let rect = Rect::from_min_size(
            editing.rect.min,
            vec2(
                editing.rect.width().max(140.0),
                editing.rect.height().max(22.0),
            ),
        );
        let response = ui.put(
            rect,
            egui::TextEdit::singleline(&mut editing.text)
                .id(id)
                .font(FontId::monospace(13.0)),
        );
        let mut chosen = None;
        let mut suggestion_rect = None;
        if !editing.suggestions.is_empty() {
            let typed = editing.text.to_lowercase();
            let matching = editing
                .suggestions
                .iter()
                .filter(|name| typed.is_empty() || name.to_lowercase().contains(&typed))
                .take(10)
                .cloned()
                .collect::<Vec<_>>();
            if !matching.is_empty() {
                let area = egui::Area::new(ui.id().with("q0lang_block_suggestions"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(rect.left_bottom() + vec2(0.0, 2.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(18, 18, 22))
                            .stroke(Stroke::new(1.0_f32, EDGE))
                            .inner_margin(4.0)
                            .show(ui, |ui| {
                                for name in matching {
                                    let button = egui::Button::new(
                                        egui::RichText::new(&name).monospace().color(INK),
                                    )
                                    .fill(Color32::TRANSPARENT);
                                    if ui.add(button).clicked() {
                                        chosen = Some(name);
                                    }
                                }
                            });
                    });
                suggestion_rect = Some(area.response.rect);
            }
        }
        if let Some(name) = chosen {
            let editing = self.editing.take().expect("editing");
            return apply_field(program, &editing.path, &editing.kind, &name);
        }
        if editing.fresh {
            response.request_focus();
            editing.fresh = false;
            return false;
        }
        let escape = ui.ctx().input(|input| input.key_pressed(egui::Key::Escape));
        // A click on a suggestion takes focus away first; let it land.
        let pointer_on_suggestion = suggestion_rect.is_some_and(|area| {
            ui.ctx().input(|input| {
                input.pointer.any_down()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pointer| area.contains(pointer))
            })
        });
        let done = escape || (!response.has_focus() && !pointer_on_suggestion);
        if !done {
            return false;
        }
        let editing = self.editing.take().expect("editing");
        if escape {
            return false;
        }
        apply_field(program, &editing.path, &editing.kind, editing.text.trim())
    }

    fn show_popup(
        &mut self,
        ui: &mut Ui,
        program: &mut Vec<Statement>,
        variables: &[String],
    ) -> bool {
        let Some(popup) = &self.popup else {
            return false;
        };
        let options: Vec<(String, String)> = match popup.kind {
            PickKind::Operator => [
                "+", "-", "*", "/", "%", "^", "==", "!=", "<", "<=", ">", ">=", "and", "or",
            ]
            .iter()
            .map(|op| (op.to_string(), op.to_string()))
            .collect(),
            PickKind::Variable => variables
                .iter()
                .map(|name| (name.clone(), name.clone()))
                .collect(),
            PickKind::SetMode => vec![
                ("SET".into(), "SET … TO".into()),
                ("CHANGE".into(), "CHANGE … BY".into()),
            ],
            PickKind::Signal => vec![
                ("AND PLAY".into(), "AND PLAY (gorun!)".into()),
                ("AND STOP".into(), "AND STOP (gostop!)".into()),
            ],
            PickKind::Event => std::iter::once((
                "time.tick".to_string(),
                "time.tick (every frame)".to_string(),
            ))
            .chain(
                BUILTIN_LIBRARIES
                    .iter()
                    .filter(|library| library.name == "q0.mouse")
                    .flat_map(|library| library.symbols.iter())
                    .map(|event| (event.to_string(), event.to_string())),
            )
            .collect(),
            PickKind::Library => BUILTIN_LIBRARIES
                .iter()
                .map(|library| (library.name.to_string(), library.name.to_string()))
                .collect(),
            PickKind::Visibility => vec![
                ("FUNC".into(), "FUNC (pb)".into()),
                ("PRIVATE FUNC".into(), "PRIVATE FUNC (pr)".into()),
            ],
            PickKind::Listen => vec![
                ("ON".into(), "ON (listen)".into()),
                ("STOP".into(), "STOP (xlisten)".into()),
            ],
        };
        let mut chosen: Option<String> = None;
        let mut type_name = false;
        let area = egui::Area::new(ui.id().with("q0lang_block_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.at)
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(Color32::from_rgb(18, 18, 22))
                    .stroke(Stroke::new(1.0_f32, EDGE))
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.set_max_height(320.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (value, label) in options {
                                let button = egui::Button::new(
                                    egui::RichText::new(label).monospace().color(INK),
                                )
                                .fill(Color32::TRANSPARENT);
                                if ui.add(button).clicked() {
                                    chosen = Some(value);
                                }
                            }
                            if popup.kind == PickKind::Variable {
                                let button = egui::Button::new(
                                    egui::RichText::new("type a name…").monospace().color(DIM),
                                )
                                .fill(Color32::TRANSPARENT);
                                if ui.add(button).clicked() {
                                    type_name = true;
                                }
                            }
                        });
                    });
            });
        let clicked_outside = ui.ctx().input(|input| input.pointer.any_pressed())
            && !ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|pointer| area.response.rect.contains(pointer));
        let popup = self.popup.take().expect("popup");
        if type_name {
            let current = resolve_value(program, &popup.path).map(|value| match value {
                Value::Ident(name) => name.clone(),
                _ => String::new(),
            });
            self.editing = Some(Editing {
                path: popup.path.clone(),
                kind: FieldKind::Literal(Arg::Any),
                text: current.unwrap_or_default(),
                rect: Rect::from_min_size(popup.at - vec2(0.0, 20.0), vec2(140.0, 20.0)),
                suggestions: Vec::new(),
                fresh: true,
            });
            return false;
        }
        if let Some(value) = chosen {
            return apply_pick(program, &popup.path, popup.kind, &value);
        }
        if !clicked_outside {
            self.popup = Some(popup);
        }
        false
    }

    /// The categories and their blocks; gives back where each block is.
    fn palette(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        names: &Names,
        variables: &[String],
        functions: &[(String, usize)],
    ) -> Vec<(Rect, String, bool)> {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(4, 4, 5));
        painter.line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(2.0_f32, EDGE),
        );
        // Categories, one under the other down the left.
        let column = 70.0;
        let mut y = rect.top() + 6.0;
        for cat in Cat::ALL {
            let cell = Rect::from_min_size(pos2(rect.left() + 4.0, y), vec2(column - 8.0, 24.0));
            let response = ui.interact(
                cell,
                ui.id().with(("q0lang_palette_cat", cat.label())),
                Sense::click(),
            );
            let selected = self.palette == cat;
            if selected {
                painter.rect_filled(cell, 0.0, cat.color());
            } else {
                painter.rect_filled(
                    Rect::from_min_size(cell.min, vec2(4.0, cell.height())),
                    0.0,
                    cat.color(),
                );
                if response.hovered() {
                    painter.rect_filled(
                        cell.shrink2(vec2(0.0, 0.0)),
                        0.0,
                        Color32::from_rgb(28, 28, 32),
                    );
                    painter.rect_filled(
                        Rect::from_min_size(cell.min, vec2(4.0, cell.height())),
                        0.0,
                        cat.color(),
                    );
                }
            }
            painter.text(
                pos2(cell.left() + 10.0, cell.center().y),
                Align2::LEFT_CENTER,
                cat.label(),
                FontId::monospace(10.5),
                if selected { Color32::BLACK } else { INK },
            );
            if response.clicked() {
                self.palette = cat;
            }
            y += 28.0;
        }
        let list_rect = Rect::from_min_max(pos2(rect.left() + column, rect.top()), rect.max);
        let mut hits = Vec::new();
        let sources = palette_sources(self.palette, names, variables, functions);
        let scroll_id = ui
            .id()
            .with(("q0lang_palette_scroll", self.palette.label()));
        let mut scroll: f32 = ui
            .ctx()
            .data(|data| data.get_temp(scroll_id).unwrap_or(0.0));
        if ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pointer| list_rect.contains(pointer))
        {
            scroll -= ui.ctx().input(|input| input.smooth_scroll_delta.y);
        }
        let clip = ui.painter_at(list_rect);
        let mut layout = Layout {
            painter: clip,
            zoom: 0.74,
            font: FontId::new(12.5 * 0.74, FontFamily::Monospace),
            names,
            hits: Vec::new(),
            sockets: Vec::new(),
            starts: Vec::new(),
            next_start: 0,
            numbers: Vec::new(),
            issues: &[],
            measuring: false,
        };
        let mut y = list_rect.top() + 10.0 - scroll;
        for (source, value) in sources {
            let Some(payload) = palette_payload(&source, value) else {
                continue;
            };
            let at = pos2(list_rect.left() + 6.0, y);
            let before = layout.hits.len();
            let size = draw_payload(&mut layout, &payload, at);
            layout.hits.truncate(before);
            let block = Rect::from_min_size(at, size);
            if block.intersects(list_rect) {
                hits.push((block.intersect(list_rect), source.clone(), value));
            }
            y += size.y + 9.0;
        }
        let content = y + scroll - list_rect.top();
        scroll = scroll.clamp(0.0, (content - list_rect.height() + 20.0).max(0.0));
        ui.ctx()
            .data_mut(|data| data.insert_temp(scroll_id, scroll));
        hits
    }
}

fn drop_target(
    drag: &Drag,
    at: Pos2,
    pointer: Pos2,
    layout: &Layout<'_>,
    canvas: Rect,
) -> Option<DropTarget> {
    let z = layout.zoom;
    let nearest_socket = |limit: f32| {
        layout
            .sockets
            .iter()
            .map(|socket| (socket, (socket.at - at).length()))
            .filter(|(_, distance)| *distance < limit)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(socket, _)| DropTarget::Socket(socket.clone()))
    };
    match &drag.payload {
        Payload::Stmts(_) => {
            if !canvas.contains(pointer) {
                return None;
            }
            // Statements always land somewhere: nothing is lost in a drop.
            nearest_socket(f32::INFINITY)
        }
        Payload::Value(value) => {
            let slot = layout
                .hits
                .iter()
                .filter(|hit| {
                    matches!(hit.what, HitWhat::Slot(_)) && hit.rect.expand(3.0).contains(pointer)
                })
                .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
                .map(|hit| DropTarget::Slot(hit.path.clone()));
            slot.or_else(|| {
                // A call on its own line runs as a statement.
                matches!(value, Value::Call { .. })
                    .then(|| nearest_socket(SNAP * z))
                    .flatten()
            })
        }
    }
}

/// Puts what was carried where it was let go; gives back where it landed.
fn place(
    program: &mut Vec<Statement>,
    drag: Drag,
    target: Option<DropTarget>,
    at: Pos2,
    z: f32,
) -> Option<Rect> {
    let imports = drag.imports.clone();
    let landed = match (target, drag.payload) {
        (Some(DropTarget::Slot(path)), Payload::Value(value)) => {
            let slot = resolve_value(program, &path)?;
            *slot = value;
            Some(Rect::from_min_size(at, vec2(60.0, 22.0) * z))
        }
        (Some(DropTarget::Socket(socket)), payload) => {
            let statements = match payload {
                Payload::Stmts(statements) => statements,
                Payload::Value(value) => vec![Statement::Expression { value }],
            };
            let list = resolve_list(program, &socket.list)?;
            let index = socket.index.min(list.len());
            for (offset, statement) in statements.into_iter().enumerate() {
                list.insert(index + offset, statement);
            }
            Some(Rect::from_min_size(socket.at, vec2(160.0, ROW) * z))
        }
        (_, Payload::Value(value)) => {
            // A loose value would mean nothing; it goes back where it was.
            if let Some(slot) = drag
                .origin
                .as_deref()
                .and_then(|origin| resolve_value(program, origin))
            {
                if matches!(slot, Value::Null) {
                    *slot = value;
                }
            }
            return None;
        }
        (Some(DropTarget::Slot(_)) | None, Payload::Stmts(statements)) => {
            program.extend(statements);
            None
        }
    };
    if landed.is_some() {
        ensure_imports(program, &imports);
    }
    landed
}

/// Draws a carried or palette block; gives back its size.
fn draw_payload(layout: &mut Layout<'_>, payload: &Payload, at: Pos2) -> Vec2 {
    let path: NodePath = vec![Step::Stmt(usize::MAX)];
    match payload {
        Payload::Stmts(statements) => {
            let height = layout.draw_list(statements, at, &path);
            let (width, _) = layout.list_size(statements);
            vec2(width, height)
        }
        Payload::Value(value) => {
            let size = layout.value_size(value);
            let rect = Rect::from_min_size(at, size);
            layout.draw_slot(value, rect, &path, Arg::Any);
            size
        }
    }
}

/// Writes an edited field back into the program.
fn apply_field(program: &mut Vec<Statement>, path: &[Step], kind: &FieldKind, text: &str) -> bool {
    match kind {
        FieldKind::Literal(arg) => {
            let Some(value) = resolve_value(program, path) else {
                return false;
            };
            let new = match (&*value, arg) {
                // Text stays text, whatever it says.
                (Value::String(_), _) => Value::String(text.to_string()),
                _ if text.is_empty() => match arg {
                    Arg::Num => Value::Number("0".into()),
                    _ => Value::String(String::new()),
                },
                _ => match parser::parse_expression(text) {
                    Ok(parsed) => parsed,
                    Err(_) => Value::String(text.to_string()),
                },
            };
            if *value == new {
                return false;
            }
            *value = new;
            true
        }
        FieldKind::Name => {
            let Some(statement) = resolve_statement(program, path) else {
                return false;
            };
            match statement {
                Statement::Assignment { name, value } if parser::is_member_name(text) => {
                    // CHANGE x BY keeps reading x.
                    if let Value::Binary {
                        op: BinaryOp::Add,
                        left,
                        ..
                    } = value
                    {
                        if matches!(left.as_ref(), Value::Ident(same) if same == name) {
                            **left = Value::Ident(text.to_string());
                        }
                    }
                    *name = text.to_string();
                    true
                }
                Statement::ForRange { name, .. } if parser::is_name(text) => {
                    *name = text.to_string();
                    true
                }
                _ => false,
            }
        }
        FieldKind::FnName => match resolve_statement(program, path) {
            Some(Statement::Function { name, .. }) if parser::is_name(text) => {
                *name = text.to_string();
                true
            }
            _ => false,
        },
        FieldKind::Params => {
            let params: Vec<String> = text
                .split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect();
            if !params.iter().all(|param| parser::is_name(param)) {
                return false;
            }
            match resolve_statement(program, path) {
                Some(Statement::Function { params: old, .. }) => {
                    *old = params;
                    true
                }
                _ => false,
            }
        }
        FieldKind::Handler => match resolve_statement(program, path) {
            Some(Statement::Listen { handler, .. } | Statement::Unlisten { handler, .. })
                if parser::is_name(text) =>
            {
                *handler = text.to_string();
                true
            }
            _ => false,
        },
        FieldKind::Callee => {
            if !parser::is_member_name(text) {
                return false;
            }
            let call = match resolve(program, path) {
                Some(NodeMut::Value(value)) => Some(value),
                Some(NodeMut::Stmt(Statement::Expression { value })) => Some(value),
                _ => None,
            };
            match call {
                Some(Value::Call { name, .. }) => {
                    *name = text.to_string();
                    true
                }
                _ => false,
            }
        }
        FieldKind::Raw => {
            // Typed code becomes blocks when it reads.
            let parsed = parser::parse(text);
            let replacement = if parsed.diagnostics.is_empty() && parsed.statements.len() == 1 {
                parsed.statements.into_iter().next().expect("one statement")
            } else {
                Statement::Unknown {
                    text: text.to_string(),
                }
            };
            match resolve(program, path) {
                Some(NodeMut::Stmt(statement)) => {
                    *statement = replacement;
                    true
                }
                Some(NodeMut::Value(value)) => match parser::parse_expression(text) {
                    Ok(parsed) => {
                        *value = parsed;
                        true
                    }
                    Err(_) => false,
                },
                _ => false,
            }
        }
        FieldKind::Note => match resolve_statement(program, path) {
            Some(statement) => {
                *statement = note(text);
                true
            }
            None => false,
        },
    }
}

fn apply_pick(program: &mut Vec<Statement>, path: &[Step], kind: PickKind, choice: &str) -> bool {
    match kind {
        PickKind::Operator => {
            let op = match choice {
                "+" => BinaryOp::Add,
                "-" => BinaryOp::Sub,
                "*" => BinaryOp::Mul,
                "/" => BinaryOp::Div,
                "%" => BinaryOp::Rem,
                "^" => BinaryOp::Pow,
                "==" => BinaryOp::Equal,
                "!=" => BinaryOp::NotEqual,
                "<" => BinaryOp::Less,
                "<=" => BinaryOp::LessEqual,
                ">" => BinaryOp::Greater,
                ">=" => BinaryOp::GreaterEqual,
                "and" => BinaryOp::And,
                "or" => BinaryOp::Or,
                _ => return false,
            };
            match resolve_value(program, path) {
                Some(Value::Binary { op: old, .. }) => {
                    *old = op;
                    true
                }
                _ => false,
            }
        }
        PickKind::Variable => match resolve_value(program, path) {
            Some(value) => {
                *value = Value::Ident(choice.to_string());
                true
            }
            None => false,
        },
        PickKind::SetMode => match resolve_statement(program, path) {
            Some(Statement::Assignment { name, value }) => {
                let changing = matches!(value, Value::Binary { op: BinaryOp::Add, left, .. } if matches!(left.as_ref(), Value::Ident(same) if same == name));
                match (choice, changing) {
                    ("CHANGE", false) => {
                        let amount = std::mem::replace(value, Value::Null);
                        *value = Value::Binary {
                            op: BinaryOp::Add,
                            left: Box::new(Value::Ident(name.clone())),
                            right: Box::new(match amount {
                                Value::Null => Value::Number("1".into()),
                                other => other,
                            }),
                        };
                        true
                    }
                    ("SET", true) => {
                        if let Value::Binary { right, .. } = std::mem::replace(value, Value::Null) {
                            *value = *right;
                        }
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        },
        PickKind::Signal => match resolve_statement(program, path) {
            Some(Statement::TimelineSignal { kind, .. }) => {
                *kind = if choice == "AND STOP" {
                    TimelineSignal::GoStop
                } else {
                    TimelineSignal::GoRun
                };
                true
            }
            _ => false,
        },
        PickKind::Event => match resolve_statement(program, path) {
            Some(Statement::Listen { event, .. } | Statement::Unlisten { event, .. }) => {
                *event = choice.to_string();
                true
            }
            _ => false,
        },
        PickKind::Library => match resolve_statement(program, path) {
            Some(Statement::Import { library }) => {
                *library = choice.to_string();
                true
            }
            _ => false,
        },
        PickKind::Visibility => match resolve_statement(program, path) {
            Some(Statement::Function { visibility, .. }) => {
                *visibility = if choice == "PRIVATE FUNC" {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
                true
            }
            _ => false,
        },
        PickKind::Listen => {
            let Some(statement) = resolve_statement(program, path) else {
                return false;
            };
            let (event, handler) = match statement {
                Statement::Listen { event, handler } | Statement::Unlisten { event, handler } => {
                    (event.clone(), handler.clone())
                }
                _ => return false,
            };
            *statement = if choice == "STOP" {
                Statement::Unlisten { event, handler }
            } else {
                Statement::Listen { event, handler }
            };
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            instances: vec!["soul".into()],
            layers: vec!["bones".into(), "walls".into()],
            ..Names::default()
        }
    }

    #[test]
    fn comments_live_on_as_notes() {
        let source = "# the fight\nimport q0.layer\nhp = 20 # starts full\nif hp > 0 {\n  # still alive\n  hp = hp - 1\n}\n";
        let program = read(source).expect("reads");
        assert!(note_text(&program[0]).is_some());
        let (text, starts) = write(&program);
        assert_eq!(
            text,
            "# the fight\nimport q0.layer\n# starts full\nhp = 20\nif hp > 0 {\n  # still alive\n  hp = hp - 1\n}\n"
        );
        assert_eq!(starts, vec![1, 2, 3, 4, 5, 6, 7]);
        // Notes with = and quotes in them stay notes.
        let tricky = "# x = \"5\" here\nx = 1\n";
        assert_eq!(write(&read(tricky).unwrap()).0, tricky);
    }

    #[test]
    fn broken_code_is_not_shown_as_blocks() {
        assert!(read("if x {\n  y = 1\n").is_err());
        assert!(read("x = (1 +\n").is_err());
    }

    #[test]
    fn every_palette_block_is_code_that_compiles() {
        let names = names();
        let variables = vec!["hp".to_string()];
        let functions = vec![("hurt".to_string(), 1)];
        for cat in Cat::ALL {
            for (source, value) in palette_sources(cat, &names, &variables, &functions) {
                let payload = palette_payload(&source, value)
                    .unwrap_or_else(|| panic!("{} block `{source}` does not read", cat.label()));
                // Inside a loop inside a function, where every block may go.
                let mut program = read(
                    "pb func probe {
  while false {
  }
}
",
                )
                .unwrap();
                let inside = vec![Step::Stmt(0), Step::Body(0), Step::Stmt(0), Step::Body(0)];
                let payload = match payload {
                    Payload::Value(item) => Payload::Stmts(vec![Statement::Assignment {
                        name: "probe_value".into(),
                        value: item,
                    }]),
                    statements => statements,
                };
                let drag = Drag {
                    imports: payload_imports(&payload),
                    payload,
                    grab: Vec2::ZERO,
                    origin: None,
                };
                let socket = Socket {
                    at: Pos2::ZERO,
                    list: inside,
                    index: 0,
                };
                place(
                    &mut program,
                    drag,
                    Some(DropTarget::Socket(socket)),
                    Pos2::ZERO,
                    1.0,
                )
                .expect("lands");
                let (text, _) = write(&program);
                let compiled = q0s_format::q0lang::compiler::compile_source(&text);
                assert!(
                    compiled.diagnostics.is_empty(),
                    "{} block `{source}` wrote:\n{text}{:?}",
                    cat.label(),
                    compiled.diagnostics
                );
            }
        }
    }

    #[test]
    fn layer_blocks_bring_their_import() {
        let mut program = read("hp = 3\n").unwrap();
        let payload = palette_payload("q0layer.solid! \"walls\", \"soul\"", false).unwrap();
        let drag = Drag {
            imports: payload_imports(&payload),
            payload,
            grab: Vec2::ZERO,
            origin: None,
        };
        let socket = Socket {
            at: Pos2::ZERO,
            list: Vec::new(),
            index: 1,
        };
        place(
            &mut program,
            drag,
            Some(DropTarget::Socket(socket)),
            Pos2::ZERO,
            1.0,
        )
        .unwrap();
        assert_eq!(
            write(&program).0,
            "import q0.layer\nhp = 3\nq0layer.solid! \"walls\", \"soul\"\n"
        );
    }

    #[test]
    fn paths_reach_bodies_slots_and_call_values() {
        let mut program =
            read("pb func hurt amount {\n  if q0layer.touches(\"bones\", \"soul\") {\n    hp = hp - amount\n  }\n}\n")
                .unwrap();
        let path = [
            Step::Stmt(0),
            Step::Body(0),
            Step::Stmt(0),
            Step::Slot(0),
            Step::Slot(1),
        ];
        assert_eq!(
            resolve_value(&mut program, &path).cloned(),
            Some(Value::String("soul".into()))
        );
        assert!(apply_field(
            &mut program,
            &path,
            &FieldKind::Literal(Arg::Thing),
            "heart"
        ));
        let inner = [
            Step::Stmt(0),
            Step::Body(0),
            Step::Stmt(0),
            Step::Body(0),
            Step::Stmt(0),
        ];
        assert!(matches!(
            resolve_statement(&mut program, &inner),
            Some(Statement::Assignment { name, .. }) if name == "hp"
        ));
        assert!(write(&program)
            .0
            .contains("q0layer.touches(\"bones\", \"heart\")"));
    }

    #[test]
    fn fields_take_values_and_keep_text_as_text() {
        let mut program = read("x = 1\ny = \"a\"\n").unwrap();
        let x = [Step::Stmt(0), Step::Slot(0)];
        let y = [Step::Stmt(1), Step::Slot(0)];
        assert!(apply_field(
            &mut program,
            &x,
            &FieldKind::Literal(Arg::Any),
            "speed * 2"
        ));
        assert!(apply_field(
            &mut program,
            &y,
            &FieldKind::Literal(Arg::Any),
            "1 + 1"
        ));
        assert_eq!(write(&program).0, "x = speed * 2\ny = \"1 + 1\"\n");
        // A raw line that reads becomes a block.
        let mut raw = vec![Statement::Unknown {
            text: "oops".into(),
        }];
        assert!(apply_field(
            &mut raw,
            &[Step::Stmt(0)],
            &FieldKind::Raw,
            "gostop! 3"
        ));
        assert!(matches!(raw[0], Statement::TimelineSignal { .. }));
    }

    #[test]
    fn set_and_change_turn_into_one_another() {
        let mut program = read("hp = 5\n").unwrap();
        let path = [Step::Stmt(0)];
        assert!(apply_pick(&mut program, &path, PickKind::SetMode, "CHANGE"));
        assert_eq!(write(&program).0, "hp = hp + 5\n");
        assert!(apply_field(&mut program, &path, &FieldKind::Name, "lives"));
        assert_eq!(write(&program).0, "lives = lives + 5\n");
        assert!(apply_pick(&mut program, &path, PickKind::SetMode, "SET"));
        assert_eq!(write(&program).0, "lives = 5\n");
    }

    #[test]
    fn a_statement_dropped_into_a_mouth_goes_inside() {
        let mut program = read("if true {\n}\nx = 1\n").unwrap();
        let moved = program.remove(1);
        let drag = Drag {
            payload: Payload::Stmts(vec![moved]),
            grab: Vec2::ZERO,
            origin: None,
            imports: Vec::new(),
        };
        let socket = Socket {
            at: Pos2::ZERO,
            list: vec![Step::Stmt(0), Step::Body(0)],
            index: 0,
        };
        place(
            &mut program,
            drag,
            Some(DropTarget::Socket(socket)),
            Pos2::ZERO,
            1.0,
        )
        .unwrap();
        assert_eq!(write(&program).0, "if true {\n  x = 1\n}\n");
    }

    #[test]
    fn the_editor_draws_a_real_script_without_trouble() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("docs")
                .join("q0lang")
                .join("examples")
                .join("basic.q0lang"),
        )
        .expect("example");
        let ctx = egui::Context::default();
        let mut editor = BlockEditor::default();
        let mut text = source.clone();
        let mut drew = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor.show(ui, &mut text, &names(), &[]);
                drew = true;
            });
        });
        assert!(drew);
        assert_eq!(text, source, "looking changes nothing");
    }
}
