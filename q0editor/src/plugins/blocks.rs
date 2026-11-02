//! Simple mode's block editor: q0ex code as Scratch-like blocks.
//!
//! The blocks are the program itself. Every block is a statement or an
//! expression of [`q0ex::Program`], so moving one moves code and the Pro
//! text follows. Hats are `when` handlers and functions; loose stacks are
//! code that runs when the plugin turns on.

use std::rc::Rc;
use std::time::Instant;

use egui::epaint::{Mesh, PathShape};
use egui::{pos2, vec2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use q0ex::ast::*;
use q0ex::parser::{parse, parse_expr};

use super::api::{self, Cat};
use crate::settings::Theme;

// ------------------------------------------------------------------ paths

/// The way from the program to one node.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Item(usize),
    /// One of the statement lists inside a node: a body, a mouth.
    Body(usize),
    Stmt(usize),
    /// One of the values inside a node, in the order the block shows them.
    Slot(usize),
}

type NodePath = Vec<Step>;

enum NodeMut<'a> {
    Item(&'a mut Item),
    List(&'a mut Vec<Stmt>),
    Stmt(&'a mut Stmt),
    Expr(&'a mut Expr),
}

fn item_bodies_mut(kind: &mut ItemKind) -> Vec<&mut Vec<Stmt>> {
    match kind {
        ItemKind::When(when) => vec![Rc::make_mut(&mut when.body)],
        ItemKind::Fn(def) => vec![Rc::make_mut(&mut def.body)],
        ItemKind::Stmt(stmt) => stmt_bodies_mut(&mut stmt.kind),
        _ => Vec::new(),
    }
}

fn item_exprs_mut(kind: &mut ItemKind) -> Vec<&mut Expr> {
    match kind {
        ItemKind::Keep { value, .. } => vec![value],
        ItemKind::Stmt(stmt) => stmt_exprs_mut(&mut stmt.kind),
        _ => Vec::new(),
    }
}

fn stmt_bodies_mut(kind: &mut StmtKind) -> Vec<&mut Vec<Stmt>> {
    match kind {
        StmtKind::If {
            branches,
            otherwise,
        } => branches
            .iter_mut()
            .map(|(_, body)| Rc::make_mut(body))
            .chain(otherwise.as_mut().map(Rc::make_mut))
            .collect(),
        StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::For { body, .. }
        | StmtKind::Do(body)
        | StmtKind::Later { body, .. }
        | StmtKind::Every { body, .. } => vec![Rc::make_mut(body)],
        StmtKind::Try { body, handler, .. } => vec![Rc::make_mut(body), Rc::make_mut(handler)],
        StmtKind::Fn(def) => vec![Rc::make_mut(&mut def.body)],
        _ => Vec::new(),
    }
}

fn stmt_bodies(kind: &StmtKind) -> Vec<&[Stmt]> {
    match kind {
        StmtKind::If {
            branches,
            otherwise,
        } => branches
            .iter()
            .map(|(_, body)| body.as_slice())
            .chain(otherwise.as_deref().map(Vec::as_slice))
            .collect(),
        StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::For { body, .. }
        | StmtKind::Do(body)
        | StmtKind::Later { body, .. }
        | StmtKind::Every { body, .. } => vec![body.as_slice()],
        StmtKind::Try { body, handler, .. } => vec![body.as_slice(), handler.as_slice()],
        StmtKind::Fn(def) => vec![def.body.as_slice()],
        _ => Vec::new(),
    }
}

fn stmt_exprs_mut(kind: &mut StmtKind) -> Vec<&mut Expr> {
    match kind {
        StmtKind::Expr(expr) => vec![expr],
        StmtKind::Assign { target, value, .. } => vec![target, value],
        StmtKind::Swap(a, b) => vec![a, b],
        StmtKind::If { branches, .. } => branches.iter_mut().map(|(cond, _)| cond).collect(),
        StmtKind::While { cond: expr, .. }
        | StmtKind::Repeat { count: expr, .. }
        | StmtKind::For { iter: expr, .. }
        | StmtKind::Later { delay: expr, .. }
        | StmtKind::Every { interval: expr, .. } => vec![expr],
        StmtKind::Return(Some(expr)) => vec![expr],
        _ => Vec::new(),
    }
}

fn call_children_mut<'a>(callee: &'a mut Expr, args: &'a mut [Expr]) -> Vec<&'a mut Expr> {
    let mut out: Vec<&mut Expr> = Vec::new();
    if let ExprKind::Field(object, _) = &mut callee.kind {
        out.push(object);
    }
    out.extend(args.iter_mut());
    out
}

fn expr_children_mut(expr: &mut Expr) -> Vec<&mut Expr> {
    match &mut expr.kind {
        ExprKind::List(items) => items.iter_mut().collect(),
        ExprKind::Map(entries) => entries.iter_mut().map(|(_, value)| value).collect(),
        ExprKind::Field(object, _) => vec![object],
        ExprKind::Index(object, index) => vec![object, index],
        ExprKind::Call { callee, args, .. } => call_children_mut(callee, args),
        ExprKind::Unary(_, operand) => vec![operand],
        ExprKind::Binary(_, left, right) => vec![left, right],
        ExprKind::Range { from, to, .. } => vec![from, to],
        ExprKind::Pipe(value, target) => vec![value, target],
        _ => Vec::new(),
    }
}

fn expr_bodies_mut(expr: &mut Expr) -> Vec<&mut Vec<Stmt>> {
    match &mut expr.kind {
        ExprKind::Lambda { body, .. } => vec![Rc::make_mut(body)],
        _ => Vec::new(),
    }
}

fn resolve<'a>(program: &'a mut Program, path: &[Step]) -> Option<NodeMut<'a>> {
    let (first, rest) = path.split_first()?;
    let Step::Item(index) = first else {
        return None;
    };
    let mut node = NodeMut::Item(program.items.get_mut(*index)?);
    for step in rest {
        node = match (node, step) {
            (NodeMut::Item(item), Step::Body(b)) => {
                NodeMut::List(item_bodies_mut(&mut item.kind).into_iter().nth(*b)?)
            }
            (NodeMut::Item(item), Step::Slot(s)) => {
                NodeMut::Expr(item_exprs_mut(&mut item.kind).into_iter().nth(*s)?)
            }
            (NodeMut::List(list), Step::Stmt(j)) => NodeMut::Stmt(list.get_mut(*j)?),
            (NodeMut::Stmt(stmt), Step::Body(b)) => {
                NodeMut::List(stmt_bodies_mut(&mut stmt.kind).into_iter().nth(*b)?)
            }
            (NodeMut::Stmt(stmt), Step::Slot(s)) => {
                NodeMut::Expr(stmt_exprs_mut(&mut stmt.kind).into_iter().nth(*s)?)
            }
            (NodeMut::Expr(expr), Step::Slot(s)) => {
                NodeMut::Expr(expr_children_mut(expr).into_iter().nth(*s)?)
            }
            (NodeMut::Expr(expr), Step::Body(b)) => {
                NodeMut::List(expr_bodies_mut(expr).into_iter().nth(*b)?)
            }
            _ => return None,
        };
    }
    Some(node)
}

fn resolve_expr<'a>(program: &'a mut Program, path: &[Step]) -> Option<&'a mut Expr> {
    match resolve(program, path)? {
        NodeMut::Expr(expr) => Some(expr),
        _ => None,
    }
}

// ----------------------------------------------------------- what's shown

/// A piece of a block's row.
enum Part<'e> {
    Text(String),
    /// A value: a reporter block or a literal field.
    Slot(usize, &'e Expr),
    /// Words the person can edit in place.
    Field(FieldKind, String),
    /// A little round variable to drag out.
    Chip(String),
    /// Opens a list to pick from.
    Pick(PickKind, String),
    /// A tiny + or − button.
    Button(ButtonKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldKind {
    /// A number, text, true/false or null in a slot.
    Literal,
    /// A variable name being set or looped with (`which` names it).
    Name(u8),
    EventArg,
    FnName,
    Params,
    Callee,
    Comment,
    /// A backtick text, edited as text.
    Template,
    /// A field name after a value: `x of obj`.
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Event,
    Operator,
    Variable,
    Assign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    AddItem,
    RemoveItem,
    AddElse,
}

/// One segment of a block, top to bottom.
enum Seg<'e> {
    Row(Vec<Part<'e>>),
    Mouth(usize, &'e [Stmt]),
}

/// How a block looks.
struct Look<'e> {
    color: Color32,
    segs: Vec<Seg<'e>>,
}

fn literal_text(expr: &Expr) -> Option<String> {
    Some(match &expr.kind {
        ExprKind::Null => String::new(),
        ExprKind::Bool(value) => value.to_string(),
        ExprKind::Num(value, unit) => {
            let mut text = q0ex::printer::number(*value);
            if let Some(unit) = unit {
                text.push_str(unit.text());
            }
            text
        }
        ExprKind::Str(text) => text.clone(),
        _ => return None,
    })
}

fn is_literal(expr: &Expr) -> bool {
    literal_text(expr).is_some()
}

fn callee_name(callee: &Expr) -> String {
    match &callee.kind {
        ExprKind::Var(name) => name.clone(),
        ExprKind::Field(object, name) => match &object.kind {
            ExprKind::Var(ns) if api::is_namespace(ns) => format!("{ns}.{name}"),
            _ => format!(".{name}"),
        },
        _ => q0ex::printer::print_expr(callee),
    }
}

/// The catalog entry for a call, and whether it is a method on a value.
fn call_entry(callee: &Expr) -> (Option<&'static api::Entry>, bool) {
    match &callee.kind {
        ExprKind::Var(name) => (api::entry(name), false),
        ExprKind::Field(object, name) => match &object.kind {
            ExprKind::Var(ns) if api::is_namespace(ns) => {
                (api::entry(&format!("{ns}.{name}")), false)
            }
            _ => {
                let method = ["obj", "layer", "timeline"]
                    .iter()
                    .find_map(|kind| api::entry(&format!("{kind}.{name}")));
                (method, true)
            }
        },
        _ => (None, false),
    }
}

/// Splits a catalog label into words and `{arg}` slots.
fn label_parts<'e>(
    label: &str,
    names: &[&str],
    args: &'e [Expr],
    offset: usize,
    parts: &mut Vec<Part<'e>>,
) -> Vec<usize> {
    let mut used = Vec::new();
    let mut rest = label;
    while let Some(start) = rest.find('{') {
        let text = rest[..start].trim();
        if !text.is_empty() {
            parts.push(Part::Text(text.to_string()));
        }
        let end = rest[start..]
            .find('}')
            .map_or(rest.len(), |end| start + end);
        let name = &rest[start + 1..end];
        if let Some(index) = names.iter().position(|arg| *arg == name) {
            if let Some(expr) = args.get(index) {
                parts.push(Part::Slot(offset + index, expr));
                used.push(index);
            }
        }
        rest = rest.get(end + 1..).unwrap_or("");
    }
    if !rest.trim().is_empty() {
        parts.push(Part::Text(rest.trim().to_string()));
    }
    used
}

fn call_look<'e>(callee: &'e Expr, args: &'e [Expr]) -> Look<'e> {
    let mut parts = Vec::new();
    let (entry, method) = call_entry(callee);
    let offset = usize::from(matches!(callee.kind, ExprKind::Field(..)));
    if method {
        if let ExprKind::Field(object, _) = &callee.kind {
            parts.push(Part::Slot(0, object));
            parts.push(Part::Text(":".into()));
        }
    }
    let color = match entry {
        Some(entry) => {
            let used = label_parts(entry.label, entry.args, args, offset, &mut parts);
            // Values past the label's words still show, with their names.
            for (index, expr) in args.iter().enumerate() {
                if !used.contains(&index) {
                    if let Some(name) = entry.args.get(index) {
                        parts.push(Part::Text((*name).to_string()));
                    }
                    parts.push(Part::Slot(offset + index, expr));
                }
            }
            entry.cat.color()
        }
        None => {
            if let ExprKind::Field(object, _) = &callee.kind {
                if !method && !matches!(&object.kind, ExprKind::Var(ns) if api::is_namespace(ns)) {
                    parts.push(Part::Slot(0, object));
                }
            }
            parts.push(Part::Field(FieldKind::Callee, callee_name(callee)));
            for (index, expr) in args.iter().enumerate() {
                parts.push(Part::Slot(offset + index, expr));
            }
            Cat::Plugin.color()
        }
    };
    Look {
        color,
        segs: vec![Seg::Row(parts)],
    }
}

fn op_word(op: BinOp) -> &'static str {
    match op {
        BinOp::And => "and",
        BinOp::Or => "or",
        other => other.text(),
    }
}

/// Booleans are six-sided, like Scratch's.
fn is_boolean(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Binary(op, ..) => matches!(
            op,
            BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or
        ),
        ExprKind::Unary(UnOp::Not, _) => true,
        ExprKind::Call { callee, .. } => call_entry(callee)
            .0
            .is_some_and(|entry| entry.label.ends_with('?')),
        _ => false,
    }
}

fn expr_look(expr: &Expr) -> Look<'_> {
    let row = |parts| vec![Seg::Row(parts)];
    match &expr.kind {
        ExprKind::Var(name) => Look {
            color: Cat::Variables.color(),
            segs: row(vec![Part::Pick(PickKind::Variable, name.clone())]),
        },
        ExprKind::Call { callee, args, .. } => call_look(callee, args),
        ExprKind::Binary(op, left, right) => Look {
            color: Cat::Math.color(),
            segs: row(vec![
                Part::Slot(0, left),
                Part::Pick(PickKind::Operator, op_word(*op).into()),
                Part::Slot(1, right),
            ]),
        },
        ExprKind::Unary(op, operand) => Look {
            color: Cat::Math.color(),
            segs: row(vec![
                Part::Text(if *op == UnOp::Not { "not" } else { "minus" }.into()),
                Part::Slot(0, operand),
            ]),
        },
        ExprKind::Field(object, name) => Look {
            color: Cat::Objects.color(),
            segs: row(vec![
                Part::Field(FieldKind::Member, name.clone()),
                Part::Text("of".into()),
                Part::Slot(0, object),
            ]),
        },
        ExprKind::Index(object, index) => Look {
            color: Cat::Lists.color(),
            segs: row(vec![
                Part::Text("item".into()),
                Part::Slot(1, index),
                Part::Text("of".into()),
                Part::Slot(0, object),
            ]),
        },
        ExprKind::Range {
            from,
            to,
            inclusive,
        } => Look {
            color: Cat::Lists.color(),
            segs: row(vec![
                Part::Slot(0, from),
                Part::Text(if *inclusive { "to" } else { "up to" }.into()),
                Part::Slot(1, to),
            ]),
        },
        ExprKind::Pipe(value, target) => Look {
            color: Cat::Math.color(),
            segs: row(vec![
                Part::Slot(0, value),
                Part::Text("then".into()),
                Part::Slot(1, target),
            ]),
        },
        ExprKind::List(items) => {
            let mut parts = vec![Part::Text("list".into())];
            for (index, item) in items.iter().enumerate() {
                parts.push(Part::Slot(index, item));
            }
            parts.push(Part::Button(ButtonKind::AddItem));
            if !items.is_empty() {
                parts.push(Part::Button(ButtonKind::RemoveItem));
            }
            Look {
                color: Cat::Lists.color(),
                segs: row(parts),
            }
        }
        ExprKind::Map(entries) => {
            let mut parts = vec![Part::Text("map".into())];
            for (index, (key, value)) in entries.iter().enumerate() {
                parts.push(Part::Text(format!("{key}:")));
                parts.push(Part::Slot(index, value));
            }
            Look {
                color: Cat::Lists.color(),
                segs: row(parts),
            }
        }
        ExprKind::Template(_) => Look {
            color: Cat::Text.color(),
            segs: row(vec![
                Part::Text("text".into()),
                Part::Field(FieldKind::Template, q0ex::printer::print_expr(expr)),
            ]),
        },
        ExprKind::Lambda { params, body } => Look {
            color: Cat::Plugin.color(),
            segs: vec![
                Seg::Row(vec![
                    Part::Text("function".into()),
                    Part::Field(FieldKind::Params, params.join(", ")),
                ]),
                Seg::Mouth(0, body.as_slice()),
            ],
        },
        _ => Look {
            color: Cat::Math.color(),
            segs: row(vec![Part::Field(
                FieldKind::Literal,
                literal_text(expr).unwrap_or_default(),
            )]),
        },
    }
}

fn assign_word(op: AssignOp) -> (&'static str, &'static str) {
    match op {
        AssignOp::Set => ("set", "to"),
        AssignOp::Add => ("change", "by"),
        AssignOp::Sub => ("lower", "by"),
        AssignOp::Mul => ("multiply", "by"),
        AssignOp::Div => ("divide", "by"),
    }
}

fn stmt_look(stmt: &Stmt) -> Look<'_> {
    let control = Cat::Control.color();
    let row = |parts| Seg::Row(parts);
    match &stmt.kind {
        StmtKind::Expr(expr) => match &expr.kind {
            ExprKind::Call { callee, args, .. } => {
                // The call's own slots sit one step deeper.
                let mut look = call_look(callee, args);
                for seg in &mut look.segs {
                    if let Seg::Row(parts) = seg {
                        for part in parts.iter_mut() {
                            if let Part::Slot(index, _) = part {
                                *index += 1000;
                            }
                        }
                    }
                }
                look
            }
            _ => Look {
                color: Cat::Plugin.color(),
                segs: vec![row(vec![
                    Part::Text("work out".into()),
                    Part::Slot(0, expr),
                ])],
            },
        },
        StmtKind::Assign { target, op, value } => {
            let (verb, link) = assign_word(*op);
            let target_part = match &target.kind {
                ExprKind::Var(name) => Part::Field(FieldKind::Name(0), name.clone()),
                _ => Part::Slot(0, target),
            };
            Look {
                color: Cat::Variables.color(),
                segs: vec![row(vec![
                    Part::Pick(PickKind::Assign, verb.into()),
                    target_part,
                    Part::Text(link.into()),
                    Part::Slot(1, value),
                ])],
            }
        }
        StmtKind::Swap(a, b) => Look {
            color: Cat::Variables.color(),
            segs: vec![row(vec![
                Part::Text("swap".into()),
                Part::Slot(0, a),
                Part::Text("with".into()),
                Part::Slot(1, b),
            ])],
        },
        StmtKind::If {
            branches,
            otherwise,
        } => {
            let mut segs = Vec::new();
            for (index, (cond, body)) in branches.iter().enumerate() {
                let word = if index == 0 { "if" } else { "else if" };
                let mut parts = vec![
                    Part::Text(word.into()),
                    Part::Slot(index, cond),
                    Part::Text("then".into()),
                ];
                if index == 0 && otherwise.is_none() {
                    parts.push(Part::Button(ButtonKind::AddElse));
                }
                segs.push(row(parts));
                segs.push(Seg::Mouth(index, body.as_slice()));
            }
            if let Some(body) = otherwise {
                segs.push(row(vec![Part::Text("else".into())]));
                segs.push(Seg::Mouth(branches.len(), body.as_slice()));
            }
            Look {
                color: control,
                segs,
            }
        }
        StmtKind::While { cond, body } => Look {
            color: control,
            segs: vec![
                row(vec![Part::Text("while".into()), Part::Slot(0, cond)]),
                Seg::Mouth(0, body),
            ],
        },
        StmtKind::Repeat { count, body } => Look {
            color: control,
            segs: vec![
                row(vec![
                    Part::Text("repeat".into()),
                    Part::Slot(0, count),
                    Part::Text("times".into()),
                ]),
                Seg::Mouth(0, body),
            ],
        },
        StmtKind::For {
            var,
            second,
            iter,
            body,
        } => {
            let mut parts = vec![
                Part::Text("for each".into()),
                Part::Field(FieldKind::Name(0), var.clone()),
            ];
            if let Some(second) = second {
                parts.push(Part::Text("and".into()));
                parts.push(Part::Field(FieldKind::Name(1), second.clone()));
            }
            parts.push(Part::Text("in".into()));
            parts.push(Part::Slot(0, iter));
            Look {
                color: control,
                segs: vec![row(parts), Seg::Mouth(0, body)],
            }
        }
        StmtKind::Break => Look {
            color: control,
            segs: vec![row(vec![Part::Text("stop the loop".into())])],
        },
        StmtKind::Continue => Look {
            color: control,
            segs: vec![row(vec![Part::Text("next round".into())])],
        },
        StmtKind::Return(value) => {
            let mut parts = vec![Part::Text("give back".into())];
            if let Some(value) = value {
                parts.push(Part::Slot(0, value));
            }
            Look {
                color: Cat::Plugin.color(),
                segs: vec![row(parts)],
            }
        }
        StmtKind::Try {
            body,
            name,
            handler,
        } => Look {
            color: control,
            segs: vec![
                row(vec![Part::Text("try".into())]),
                Seg::Mouth(0, body),
                row(vec![
                    Part::Text("oops, call it".into()),
                    Part::Field(FieldKind::Name(0), name.clone().unwrap_or_default()),
                ]),
                Seg::Mouth(1, handler),
            ],
        },
        StmtKind::Do(body) => Look {
            color: control,
            segs: vec![row(vec![Part::Text("do!".into())]), Seg::Mouth(0, body)],
        },
        StmtKind::Later { delay, body } => Look {
            color: control,
            segs: vec![
                row(vec![
                    Part::Text("after".into()),
                    Part::Slot(0, delay),
                    Part::Text("seconds".into()),
                ]),
                Seg::Mouth(0, body),
            ],
        },
        StmtKind::Every { interval, body } => Look {
            color: control,
            segs: vec![
                row(vec![
                    Part::Text("every".into()),
                    Part::Slot(0, interval),
                    Part::Text("seconds".into()),
                ]),
                Seg::Mouth(0, body),
            ],
        },
        StmtKind::Fn(def) => Look {
            color: Cat::Plugin.color(),
            segs: vec![
                row(vec![
                    Part::Text("define".into()),
                    Part::Field(FieldKind::FnName, def.name.clone()),
                    Part::Field(FieldKind::Params, def.params.join(", ")),
                ]),
                Seg::Mouth(0, &def.body),
            ],
        },
        StmtKind::Comment(text) => Look {
            color: Color32::from_rgb(229, 210, 120),
            segs: vec![row(vec![Part::Field(FieldKind::Comment, text.clone())])],
        },
    }
}

/// The path step for a part's slot: statements keep their call's slots one
/// step deeper (see `stmt_look`).
fn slot_steps(index: usize) -> Vec<Step> {
    if index >= 1000 {
        vec![Step::Slot(0), Step::Slot(index - 1000)]
    } else {
        vec![Step::Slot(index)]
    }
}

// ----------------------------------------------------------------- sizes

/// Sizes at zoom 1.
const ROW: f32 = 30.0;
const PAD: f32 = 8.0;
const GAP: f32 = 5.0;
const ARM: f32 = 14.0;
const MOUTH_EMPTY: f32 = 20.0;
const FOOT: f32 = 14.0;
const HAT_BUMP: f32 = 14.0;
const NOTCH_X: f32 = 14.0;
const NOTCH_W: f32 = 16.0;
const NOTCH_D: f32 = 4.0;
const FIELD_H: f32 = 20.0;
const SNAP: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape3 {
    Stack,
    Hat,
    Round,
    Hex,
}

/// Something under the pointer.
#[derive(Clone)]
struct Hit {
    rect: Rect,
    path: NodePath,
    what: HitWhat,
}

#[derive(Clone, PartialEq)]
enum HitWhat {
    /// A statement block: drags it and everything under it.
    Stmt,
    /// A hat or other top-level block: drags the whole stack.
    Item,
    /// A reporter block in a slot.
    Expr,
    /// A slot whose literal can be replaced by a dropped reporter.
    Slot,
    Field(FieldKind, String),
    Chip(String),
    Pick(PickKind, String),
    Button(ButtonKind),
}

/// Where a dragged stack may click in.
#[derive(Clone, Debug)]
struct Socket {
    /// The screen point a dragged stack's top-left snaps to.
    at: Pos2,
    target: SocketTarget,
}

#[derive(Clone, Debug)]
enum SocketTarget {
    /// Into a statement list at an index.
    List(NodePath, usize),
    /// After top-level item `index`, as part of its stack.
    AfterItem(usize),
    /// On top of the top-level stack that starts at item `index`.
    BeforeItem(usize),
}

struct Layout<'p> {
    painter: Painter,
    zoom: f32,
    font: FontId,
    hits: Vec<Hit>,
    sockets: Vec<Socket>,
    text_dark: Color32,
    /// Draw nothing, only measure.
    measuring: bool,
    _p: std::marker::PhantomData<&'p ()>,
}

impl Layout<'_> {
    fn text_width(&self, text: &str) -> f32 {
        self.painter
            .layout_no_wrap(text.to_string(), self.font.clone(), Color32::WHITE)
            .size()
            .x
    }

    fn part_size(&mut self, part: &Part<'_>) -> Vec2 {
        let z = self.zoom;
        match part {
            Part::Text(text) => vec2(self.text_width(text), FIELD_H * z),
            Part::Slot(_, expr) => self.expr_size(expr),
            Part::Field(kind, text) => {
                let shown = if text.is_empty() && *kind != FieldKind::Comment {
                    " "
                } else {
                    text
                };
                vec2(
                    (self.text_width(shown) + 14.0 * z).max(24.0 * z),
                    FIELD_H * z,
                )
            }
            Part::Chip(name) => vec2(self.text_width(name) + 14.0 * z, FIELD_H * z),
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

    fn expr_size(&mut self, expr: &Expr) -> Vec2 {
        let z = self.zoom;
        if is_literal(expr) {
            let text = literal_text(expr).unwrap_or_default();
            let shown = if text.is_empty() { " " } else { &text };
            return vec2(
                (self.text_width(shown) + 14.0 * z).max(28.0 * z),
                FIELD_H * z,
            );
        }
        let look = expr_look(expr);
        let (size, _) = self.look_size(&look, Shape3::Round);
        if look.segs.iter().any(|seg| matches!(seg, Seg::Mouth(..))) {
            return size;
        }
        // Reporters sit a little tighter than statements.
        let extra = if is_boolean(expr) { 10.0 * z } else { 4.0 * z };
        vec2(size.x + extra, size.y - 6.0 * z)
    }

    /// The size of a block, and the heights of its segments.
    fn look_size(&mut self, look: &Look<'_>, shape: Shape3) -> (Vec2, Vec<f32>) {
        let z = self.zoom;
        let mut width: f32 = 40.0 * z;
        let mut heights = Vec::new();
        let mut has_mouth = false;
        for seg in &look.segs {
            match seg {
                Seg::Row(parts) => {
                    let size = self.row_size(parts);
                    width = width.max(size.x);
                    heights.push(size.y);
                }
                Seg::Mouth(_, stmts) => {
                    has_mouth = true;
                    let (stack_w, stack_h) = self.stack_size(stmts);
                    width = width.max(ARM * z + stack_w.min(40.0 * z));
                    heights.push(stack_h.max(MOUTH_EMPTY * z));
                }
            }
        }
        let mut height: f32 = heights.iter().sum();
        if has_mouth && matches!(look.segs.last(), Some(Seg::Mouth(..))) {
            height += FOOT * z;
        }
        if shape == Shape3::Hat {
            height += HAT_BUMP * z;
            width = width.max(110.0 * z);
        }
        (vec2(width, height), heights)
    }

    fn stack_size(&mut self, stmts: &[Stmt]) -> (f32, f32) {
        let mut width: f32 = 0.0;
        let mut height = 0.0;
        for stmt in stmts {
            let look = stmt_look(stmt);
            let (size, _) = self.look_size(&look, Shape3::Stack);
            width = width.max(size.x);
            height += size.y;
        }
        (width, height)
    }

    // ------------------------------------------------------------ draw

    /// Draws a block at `at`; gives back its size.
    fn draw_look(
        &mut self,
        look: &Look<'_>,
        shape: Shape3,
        at: Pos2,
        path: &NodePath,
        what: HitWhat,
    ) -> Vec2 {
        let z = self.zoom;
        let (size, heights) = self.look_size(look, shape);
        let top_extra = if shape == Shape3::Hat {
            HAT_BUMP * z
        } else {
            0.0
        };
        let rect = Rect::from_min_size(at, size);
        if !self.measuring {
            let outline = block_outline(look, &heights, shape, rect, z);
            paint_block(&self.painter, &outline, look.color, shape);
        }
        self.hits.push(Hit {
            rect,
            path: path.clone(),
            what,
        });
        let mut y = at.y + top_extra;
        for (seg, height) in look.segs.iter().zip(&heights) {
            match seg {
                Seg::Row(parts) => {
                    self.draw_row(parts, pos2(at.x, y), *height, path, look.color);
                }
                Seg::Mouth(index, stmts) => {
                    let mut list_path = path.clone();
                    list_path.push(Step::Body(*index));
                    self.draw_stack(stmts, pos2(at.x + ARM * z, y), &list_path);
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
        color: Color32,
    ) {
        let z = self.zoom;
        let mut x = at.x + PAD * z;
        for part in parts {
            let size = self.part_size(part);
            let top = at.y + (height - size.y) / 2.0;
            let rect = Rect::from_min_size(pos2(x, top), size);
            match part {
                Part::Text(text) => {
                    if !self.measuring {
                        self.painter.text(
                            pos2(x, at.y + height / 2.0),
                            egui::Align2::LEFT_CENTER,
                            text,
                            self.font.clone(),
                            Color32::WHITE,
                        );
                    }
                }
                Part::Slot(index, expr) => {
                    let mut slot_path = path.clone();
                    slot_path.extend(slot_steps(*index));
                    if is_literal(expr) {
                        let text = literal_text(expr).unwrap_or_default();
                        if !self.measuring {
                            self.paint_field(rect, &text, false);
                        }
                        self.hits.push(Hit {
                            rect,
                            path: slot_path.clone(),
                            what: HitWhat::Slot,
                        });
                        self.hits.push(Hit {
                            rect,
                            path: slot_path,
                            what: HitWhat::Field(FieldKind::Literal, text),
                        });
                    } else {
                        let look = expr_look(expr);
                        let shape = if is_boolean(expr) {
                            Shape3::Hex
                        } else {
                            Shape3::Round
                        };
                        self.draw_reporter(&look, shape, rect, &slot_path);
                    }
                }
                Part::Field(kind, text) => {
                    if !self.measuring {
                        if *kind == FieldKind::Comment {
                            self.painter.text(
                                pos2(x + 4.0 * z, at.y + height / 2.0),
                                egui::Align2::LEFT_CENTER,
                                format!("# {text}"),
                                self.font.clone(),
                                self.text_dark,
                            );
                        } else {
                            self.paint_field(rect, text, *kind != FieldKind::Literal);
                        }
                    }
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Field(kind.clone(), text.clone()),
                    });
                }
                Part::Chip(name) => {
                    if !self.measuring {
                        let chip = Cat::Variables.color();
                        self.painter.rect_filled(rect, rect.height() / 2.0, chip);
                        self.painter.rect_stroke(
                            rect,
                            rect.height() / 2.0,
                            Stroke::new(1.0_f32, darker(chip)),
                        );
                        self.painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            name,
                            self.font.clone(),
                            Color32::WHITE,
                        );
                    }
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Chip(name.clone()),
                    });
                }
                Part::Pick(kind, text) => {
                    if !self.measuring {
                        self.painter.rect_filled(rect, 5.0 * z, darker(color));
                        self.painter.text(
                            pos2(rect.left() + 6.0 * z, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            text,
                            self.font.clone(),
                            Color32::WHITE,
                        );
                        self.painter.text(
                            pos2(rect.right() - 5.0 * z, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            "▾",
                            FontId::proportional(9.0 * z),
                            Color32::WHITE,
                        );
                    }
                    self.hits.push(Hit {
                        rect,
                        path: path.clone(),
                        what: HitWhat::Pick(*kind, text.clone()),
                    });
                }
                Part::Button(kind) => {
                    if !self.measuring {
                        self.painter.circle_filled(
                            rect.center(),
                            rect.width() / 2.0,
                            darker(color),
                        );
                        self.painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            match kind {
                                ButtonKind::RemoveItem => "−",
                                _ => "+",
                            },
                            FontId::proportional(12.0 * z),
                            Color32::WHITE,
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

    fn paint_field(&self, rect: Rect, text: &str, name: bool) {
        let z = self.zoom;
        let fill = if name {
            Color32::from_rgb(250, 250, 250)
        } else {
            Color32::WHITE
        };
        self.painter.rect_filled(rect, rect.height() / 2.0, fill);
        self.painter.rect_stroke(
            rect,
            rect.height() / 2.0,
            Stroke::new(1.0_f32, Color32::from_black_alpha(40)),
        );
        self.painter.text(
            pos2(rect.left() + 7.0 * z, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            self.font.clone(),
            self.text_dark,
        );
    }

    fn draw_reporter(&mut self, look: &Look<'_>, shape: Shape3, rect: Rect, path: &NodePath) {
        let z = self.zoom;
        if !self.measuring {
            match shape {
                Shape3::Hex => {
                    let points = hexagon(rect);
                    self.painter.add(Shape::convex_polygon(
                        points,
                        look.color,
                        Stroke::new(1.0_f32, darker(look.color)),
                    ));
                }
                _ if look.segs.iter().any(|seg| matches!(seg, Seg::Mouth(..))) => {
                    let (_, heights) = self.look_size(look, Shape3::Stack);
                    let outline = block_outline(look, &heights, Shape3::Stack, rect, z);
                    paint_block(&self.painter, &outline, look.color, Shape3::Stack);
                }
                _ => {
                    self.painter
                        .rect_filled(rect, rect.height() / 2.0, look.color);
                    self.painter.rect_stroke(
                        rect,
                        rect.height() / 2.0,
                        Stroke::new(1.0_f32, darker(look.color)),
                    );
                }
            }
        }
        self.hits.push(Hit {
            rect,
            path: path.clone(),
            what: HitWhat::Expr,
        });
        let inset = if shape == Shape3::Hex {
            6.0 * z
        } else {
            2.0 * z
        };
        let mut y = rect.top();
        let has_mouth = look.segs.iter().any(|seg| matches!(seg, Seg::Mouth(..)));
        let (_, heights) = self.look_size(look, Shape3::Stack);
        for (seg, height) in look.segs.iter().zip(&heights) {
            let height = if has_mouth { *height } else { rect.height() };
            match seg {
                Seg::Row(parts) => self.draw_row(
                    parts,
                    pos2(rect.left() + inset - PAD * z / 2.0, y),
                    height,
                    path,
                    look.color,
                ),
                Seg::Mouth(index, stmts) => {
                    let mut list_path = path.clone();
                    list_path.push(Step::Body(*index));
                    self.draw_stack(stmts, pos2(rect.left() + ARM * z, y), &list_path);
                }
            }
            y += height;
        }
    }

    /// Draws statements one under the other; gives back the height.
    fn draw_stack(&mut self, stmts: &[Stmt], at: Pos2, list_path: &NodePath) -> f32 {
        let mut y = at.y;
        self.sockets.push(Socket {
            at: pos2(at.x, y),
            target: SocketTarget::List(list_path.clone(), 0),
        });
        for (index, stmt) in stmts.iter().enumerate() {
            let mut path = list_path.clone();
            path.push(Step::Stmt(index));
            let look = stmt_look(stmt);
            let size = self.draw_look(&look, Shape3::Stack, pos2(at.x, y), &path, HitWhat::Stmt);
            y += size.y;
            self.sockets.push(Socket {
                at: pos2(at.x, y),
                target: SocketTarget::List(list_path.clone(), index + 1),
            });
        }
        y - at.y
    }
}

fn darker(color: Color32) -> Color32 {
    Color32::from_rgb(
        (f32::from(color.r()) * 0.78) as u8,
        (f32::from(color.g()) * 0.78) as u8,
        (f32::from(color.b()) * 0.78) as u8,
    )
}

fn hexagon(rect: Rect) -> Vec<Pos2> {
    let inset = rect.height() / 2.0;
    vec![
        pos2(rect.left(), rect.center().y),
        pos2(rect.left() + inset, rect.top()),
        pos2(rect.right() - inset, rect.top()),
        pos2(rect.right(), rect.center().y),
        pos2(rect.right() - inset, rect.bottom()),
        pos2(rect.left() + inset, rect.bottom()),
    ]
}

/// The outline of a block, with its notch, tab, hat and mouths.
fn block_outline(look: &Look<'_>, heights: &[f32], shape: Shape3, rect: Rect, z: f32) -> Vec<Pos2> {
    let (left, right) = (rect.left(), rect.right());
    let nx = NOTCH_X * z;
    let nw = NOTCH_W * z;
    let nd = NOTCH_D * z;
    let mut points = Vec::new();
    let top = rect.top();
    // Top edge, left to right.
    if shape == Shape3::Hat {
        let bump = HAT_BUMP * z;
        let width = (90.0 * z).min(rect.width());
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let x = left + width * t;
            // sin(pi) lands a hair below zero, and a power of that is NaN.
            let y = top + bump - bump * (t * std::f32::consts::PI).sin().max(0.0).powf(0.7);
            points.push(pos2(x, y));
        }
        points.push(pos2(right, top + bump));
    } else {
        points.push(pos2(left, top));
        points.push(pos2(left + nx, top));
        points.push(pos2(left + nx + 3.0 * z, top + nd));
        points.push(pos2(left + nx + nw - 3.0 * z, top + nd));
        points.push(pos2(left + nx + nw, top));
        points.push(pos2(right, top));
    }
    // Right side with the mouths, top to bottom.
    let mut y = top
        + if shape == Shape3::Hat {
            HAT_BUMP * z
        } else {
            0.0
        };
    let inner = left + ARM * z;
    for (seg, height) in look.segs.iter().zip(heights) {
        match seg {
            Seg::Row(_) => {
                y += height;
                points.push(pos2(right, y));
            }
            Seg::Mouth(..) => {
                // Into the mouth: its top has a tab for the first block.
                points.push(pos2(inner + nx + nw, y));
                points.push(pos2(inner + nx + nw - 3.0 * z, y + nd));
                points.push(pos2(inner + nx + 3.0 * z, y + nd));
                points.push(pos2(inner + nx, y));
                points.push(pos2(inner, y));
                y += height;
                points.push(pos2(inner, y));
                points.push(pos2(inner + nx, y));
                points.push(pos2(inner + nx + 3.0 * z, y + nd));
                points.push(pos2(inner + nx + nw - 3.0 * z, y + nd));
                points.push(pos2(inner + nx + nw, y));
                points.push(pos2(right, y));
            }
        }
    }
    let bottom = rect.bottom();
    if (bottom - y).abs() > 0.5 {
        points.push(pos2(right, bottom));
    }
    // Bottom edge, right to left, with the tab.
    points.push(pos2(left + nx + nw, bottom));
    points.push(pos2(left + nx + nw - 3.0 * z, bottom + nd));
    points.push(pos2(left + nx + 3.0 * z, bottom + nd));
    points.push(pos2(left + nx, bottom));
    points.push(pos2(left, bottom));
    points.dedup_by(|a, b| (*a - *b).length() < 0.01);
    points
}

/// Ear clipping for a simple polygon; blocks are never self-crossing.
fn triangulate(points: &[Pos2]) -> Vec<[u32; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let clockwise = area < 0.0;
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let convex = |a: Pos2, b: Pos2, c: Pos2| {
        let value = cross(a, b, c);
        if clockwise {
            value < 0.0
        } else {
            value > 0.0
        }
    };
    let inside = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(negative && positive)
    };
    let mut indices: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n);
    let mut guard = 0;
    while indices.len() > 3 && guard < n * n {
        guard += 1;
        let m = indices.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (indices[(i + m - 1) % m], indices[i], indices[(i + 1) % m]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if !convex(a, b, c) {
                continue;
            }
            let blocked = indices
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && inside(points[j], a, b, c));
            if blocked {
                continue;
            }
            triangles.push([ia as u32, ib as u32, ic as u32]);
            indices.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if indices.len() == 3 {
        triangles.push([indices[0] as u32, indices[1] as u32, indices[2] as u32]);
    }
    triangles
}

fn fill_polygon(painter: &Painter, points: &[Pos2], color: Color32) {
    let mut mesh = Mesh::default();
    for point in points {
        mesh.colored_vertex(*point, color);
    }
    for [a, b, c] in triangulate(points) {
        mesh.add_triangle(a, b, c);
    }
    painter.add(Shape::mesh(mesh));
}

fn paint_block(painter: &Painter, outline: &[Pos2], color: Color32, _shape: Shape3) {
    fill_polygon(painter, outline, color);
    // A soft light along the top.
    painter.add(PathShape::closed_line(
        outline.to_vec(),
        Stroke::new(1.0_f32, darker(color)),
    ));
}

// ------------------------------------------------------------ the editor

/// What is being dragged.
enum Payload {
    Stmts(Vec<Stmt>),
    Expr(Expr),
    /// A whole top-level stack: the item and the loose statements under it.
    Items(Vec<Item>),
}

struct Drag {
    payload: Payload,
    /// The pointer's place on the dragged block, in screen points at zoom.
    grab: Vec2,
    /// The slot a reporter was taken from, to go back to when it is let
    /// go over nothing.
    origin: Option<NodePath>,
}

struct Editing {
    path: NodePath,
    kind: FieldKind,
    text: String,
    rect: Rect,
    fresh: bool,
}

struct Popup {
    path: NodePath,
    kind: PickKind,
    at: Pos2,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Palette {
    Cat(Cat),
    Functions,
}

pub(crate) struct BlockEditor {
    pan: Vec2,
    zoom: f32,
    palette: Palette,
    drag: Option<Drag>,
    editing: Option<Editing>,
    popup: Option<Popup>,
    /// Flashes where something snapped in.
    snaps: Vec<(Rect, Instant)>,
    new_variable: Option<String>,
    pending: Option<PendingPress>,
}

impl Default for BlockEditor {
    fn default() -> Self {
        Self {
            pan: vec2(20.0, 20.0),
            zoom: 1.0,
            palette: Palette::Cat(Cat::Events),
            drag: None,
            editing: None,
            popup: None,
            snaps: Vec::new(),
            new_variable: None,
            pending: None,
        }
    }
}

/// A value to start a slot with, guessed from its name.
fn default_arg(name: &str) -> &'static str {
    match name {
        "x" | "y" | "x1" | "y1" | "dx" | "dy" | "low" | "min" | "value" | "at" | "from" | "a" => {
            "0"
        }
        "x2" | "y2" | "w" | "h" | "high" | "max" | "to" | "b" => "100",
        "r" | "width" | "px" | "size" => "10",
        "frame" | "n" | "times" | "fps" | "zoom" | "t" | "amount" => "1",
        "color" | "fill" | "line" => "\"#9d4eff\"",
        "on" | "closed" => "true",
        "list" | "points" | "objects" | "options" => "[]",
        "map" => "{}",
        "fn" => "fn() {\n}",
        "markup" => "\"**hello**\"",
        "key" => "\"Space\"",
        "deg" => "45",
        "hue" => "300",
        "sat" | "val" => "1",
        _ => "\"\"",
    }
}

fn template_stmt(src: &str) -> Option<Stmt> {
    match parse(src).ok()?.items.into_iter().next()?.kind {
        ItemKind::Stmt(stmt) => Some(stmt),
        _ => None,
    }
}

/// Words read like fields, without parentheses.
const PROPERTIES: [&str; 9] = [
    "plugin.name",
    "plugin.id",
    "plugin.version",
    "mouse.x",
    "mouse.y",
    "obj.x",
    "obj.kind",
    "obj.name",
    "obj.raw",
];

fn entry_source(entry: &api::Entry) -> String {
    if PROPERTIES.contains(&entry.name) {
        return entry.name.to_string();
    }
    let args = entry
        .args
        .iter()
        .map(|arg| default_arg(arg))
        .collect::<Vec<_>>();
    format!("{}({})", entry.name, args.join(", "))
}

/// The blocks a palette page offers, as code.
fn palette_sources(
    palette: Palette,
    variables: &[String],
    functions: &[String],
) -> Vec<(String, bool)> {
    // (source, is a whole item)
    let mut out: Vec<(String, bool)> = Vec::new();
    match palette {
        Palette::Cat(Cat::Events) => {
            for event in api::EVENTS {
                let mut head = format!("when {}", event.event);
                if let Some(arg) = event.arg {
                    head.push_str(&format!(" {arg:?}"));
                }
                if !event.params.is_empty() {
                    head.push(' ');
                    head.push_str(&event.params.join(", "));
                }
                out.push((format!("{head} {{\n}}"), true));
            }
        }
        Palette::Cat(Cat::Control) => {
            for src in [
                "if true {\n}",
                "if true {\n} else {\n}",
                "repeat 10 {\n}",
                "while true {\n}",
                "for item in [1, 2, 3] {\n}",
                "later! 1 {\n}",
                "every! 1 {\n}",
                "try {\n} oops err {\n}",
                "break",
                "continue",
                "oops! \"something went wrong\"",
                "# a note",
            ] {
                out.push((src.to_string(), false));
            }
        }
        Palette::Cat(Cat::Variables) => {
            let first = variables.first().cloned().unwrap_or_else(|| "score".into());
            for src in [
                format!("{first} = 0"),
                format!("{first} += 1"),
                "a <-> b".to_string(),
            ] {
                out.push((src, false));
            }
            out.push((format!("keep {first} = 0"), true));
            for name in variables {
                out.push((name.clone(), false));
            }
        }
        Palette::Cat(Cat::Math) => {
            for src in [
                "1 + 1",
                "1 - 1",
                "2 * 3",
                "6 / 2",
                "7 % 3",
                "2 ^ 3",
                "1 < 2",
                "1 == 1",
                "true and true",
                "true or false",
                "not true",
            ] {
                out.push((src.to_string(), false));
            }
            for entry in api::ENTRIES.iter().filter(|entry| entry.cat == Cat::Math) {
                out.push((entry_source(entry), false));
            }
        }
        Palette::Cat(Cat::Lists) => {
            for src in ["[1, 2, 3]", "list[0]", "1..10"] {
                out.push((src.to_string(), false));
            }
            for entry in api::ENTRIES.iter().filter(|entry| entry.cat == Cat::Lists) {
                out.push((entry_source(entry), false));
            }
        }
        Palette::Cat(Cat::Objects) => {
            out.push(("for obj in project.selection() {\n}".into(), false));
            out.push(("obj.x".into(), false));
            out.push(("obj.x = 0".into(), false));
            out.push(("obj.rotation += 15".into(), false));
            for entry in api::ENTRIES
                .iter()
                .filter(|entry| entry.cat == Cat::Objects)
            {
                if entry.name != "obj.x" {
                    out.push((entry_source(entry), false));
                }
            }
        }
        Palette::Cat(cat) => {
            for entry in api::ENTRIES.iter().filter(|entry| entry.cat == cat) {
                out.push((entry_source(entry), false));
            }
        }
        Palette::Functions => {
            out.push(("fn my_block(a) {\n}".into(), true));
            out.push(("return 0".into(), false));
            for name in functions {
                out.push((format!("{name}()"), false));
            }
        }
    }
    out
}

/// A fresh copy of a palette block.
fn palette_payload(src: &str, item: bool) -> Option<Payload> {
    if item {
        let program = parse(src).ok()?;
        return Some(Payload::Items(program.items));
    }
    // Calls with `!` become commands; plain values stay values.
    if let Ok(expr) = parse_expr(src) {
        let command = matches!(&expr.kind, ExprKind::Call { callee, .. } if q0ex::parser::bang_name(callee).is_some());
        if !command {
            return Some(Payload::Expr(expr));
        }
        let line = expr.line;
        let ExprKind::Call { callee, args, .. } = expr.kind else {
            return None;
        };
        return Some(Payload::Stmts(vec![Stmt {
            kind: StmtKind::Expr(Expr::new(
                ExprKind::Call {
                    callee,
                    args,
                    bang: true,
                },
                line,
            )),
            line,
        }]));
    }
    template_stmt(src).map(|stmt| Payload::Stmts(vec![stmt]))
}

fn collect_names(program: &Program) -> (Vec<String>, Vec<String>) {
    let mut variables = std::collections::BTreeSet::new();
    let mut functions = std::collections::BTreeSet::new();
    fn walk_stmts(
        stmts: &[Stmt],
        vars: &mut std::collections::BTreeSet<String>,
        fns: &mut std::collections::BTreeSet<String>,
    ) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Assign { target, .. } => {
                    if let ExprKind::Var(name) = &target.kind {
                        vars.insert(name.clone());
                    }
                }
                StmtKind::For { var, second, .. } => {
                    vars.insert(var.clone());
                    if let Some(second) = second {
                        vars.insert(second.clone());
                    }
                }
                StmtKind::Fn(def) => {
                    fns.insert(def.name.clone());
                }
                _ => {}
            }
            for body in stmt_bodies(&stmt.kind) {
                walk_stmts(body, vars, fns);
            }
        }
    }
    for item in &program.items {
        match &item.kind {
            ItemKind::Keep { name, .. } => {
                variables.insert(name.clone());
            }
            ItemKind::Fn(def) => {
                functions.insert(def.name.clone());
                variables.extend(def.params.iter().cloned());
                walk_stmts(&def.body, &mut variables, &mut functions);
            }
            ItemKind::When(when) => {
                variables.extend(when.params.iter().cloned());
                walk_stmts(&when.body, &mut variables, &mut functions);
            }
            ItemKind::Stmt(stmt) => {
                walk_stmts(std::slice::from_ref(stmt), &mut variables, &mut functions)
            }
            _ => {}
        }
    }
    (
        variables.into_iter().collect(),
        functions.into_iter().collect(),
    )
}

fn is_code(item: &Item) -> bool {
    matches!(
        item.kind,
        ItemKind::When(_) | ItemKind::Fn(_) | ItemKind::Keep { .. } | ItemKind::Stmt(_)
    )
}

/// Whether item `index` continues the stack above it.
fn continues_stack(items: &[Item], index: usize) -> bool {
    index > 0
        && items[index].pos.is_none()
        && matches!(items[index].kind, ItemKind::Stmt(_))
        && matches!(items[index - 1].kind, ItemKind::Stmt(_))
}

/// Gives every stack without a place one, below the others.
fn place_stacks(program: &mut Program) {
    let mut y = 20.0_f32;
    for item in &program.items {
        if let Some((_, top)) = item.pos {
            y = y.max(top + 80.0);
        }
    }
    for index in 0..program.items.len() {
        if !is_code(&program.items[index]) || continues_stack(&program.items, index) {
            continue;
        }
        if program.items[index].pos.is_none() {
            program.items[index].pos = Some((20.0, y));
            y += 90.0;
        }
    }
}

impl BlockEditor {
    /// Shows the blocks of `program`; true when it changed.
    pub fn show(&mut self, ui: &mut Ui, program: &mut Program, theme: &Theme) -> bool {
        let mut changed = false;
        let (variables, functions) = collect_names(program);
        let available = ui.available_rect_before_wrap();
        let palette_width = 230.0_f32.min(available.width() * 0.4);
        let palette_rect =
            Rect::from_min_size(available.min, vec2(palette_width, available.height()));
        let canvas_rect = Rect::from_min_max(
            pos2(palette_rect.right() + 4.0, available.top()),
            available.max,
        );
        ui.allocate_rect(available, Sense::hover());
        let canvas = ui.interact(
            canvas_rect,
            ui.id().with("block_canvas"),
            Sense::click_and_drag(),
        );
        // Drags that start on the palette are the editor's too; nothing
        // around it (a window, a panel) may take them.
        ui.interact(
            palette_rect,
            ui.id().with("block_palette"),
            Sense::click_and_drag(),
        );
        let pointer = ui.ctx().pointer_interact_pos();
        let z = self.zoom;
        let font = FontId::proportional(12.5 * z);

        // Zoom with ctrl and the wheel, pan by dragging the empty canvas.
        if canvas.hovered() {
            let (scroll, zoom_delta) = ui
                .ctx()
                .input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
            if zoom_delta != 1.0 {
                let old = self.zoom;
                self.zoom = (self.zoom * zoom_delta).clamp(0.4, 2.5);
                if let Some(pointer) = pointer {
                    let anchor = pointer - canvas_rect.min - self.pan;
                    self.pan -= anchor * (self.zoom / old - 1.0);
                }
            } else if self.drag.is_none() {
                self.pan += scroll;
            }
        }

        // ---------------------------------------------------- the canvas
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 8.0, ui.visuals().extreme_bg_color);
        // A faint dot grid that moves with the pan.
        let spacing = 24.0 * z;
        let dots = ui.visuals().weak_text_color().gamma_multiply(0.25);
        let start = canvas_rect.min
            + vec2(
                self.pan.x.rem_euclid(spacing),
                self.pan.y.rem_euclid(spacing),
            );
        let mut gx = start.x;
        while gx < canvas_rect.right() {
            let mut gy = start.y;
            while gy < canvas_rect.bottom() {
                painter.circle_filled(pos2(gx, gy), 1.0, dots);
                gy += spacing;
            }
            gx += spacing;
        }
        let origin = canvas_rect.min + self.pan;
        let mut layout = Layout {
            painter: painter.clone(),
            zoom: z,
            font: font.clone(),
            hits: Vec::new(),
            sockets: Vec::new(),
            text_dark: Color32::from_rgb(70, 70, 80),
            measuring: false,
            _p: std::marker::PhantomData,
        };
        // Stacks.
        let mut stack_bottom: Option<(Pos2, usize)> = None;
        let mut auto_y = 20.0_f32;
        for index in 0..program.items.len() {
            let item = &program.items[index];
            if !is_code(item) {
                continue;
            }
            let path = vec![Step::Item(index)];
            if continues_stack(&program.items, index) {
                if let (Some((at, _)), ItemKind::Stmt(stmt)) = (stack_bottom, &item.kind) {
                    let look = stmt_look(stmt);
                    let size = layout.draw_look(&look, Shape3::Stack, at, &path, HitWhat::Item);
                    let next = pos2(at.x, at.y + size.y);
                    stack_bottom = Some((next, index));
                    layout.sockets.push(Socket {
                        at: next,
                        target: SocketTarget::AfterItem(index),
                    });
                    // Stacks placed after this one start below it.
                    auto_y = auto_y.max((next.y - origin.y) / z + 30.0);
                }
                continue;
            }
            let (x, y) = item.pos.unwrap_or((20.0, auto_y));
            let at = origin + vec2(x, y) * z;
            let height = match &item.kind {
                ItemKind::When(when) => {
                    let mut parts = Vec::new();
                    let event = api::event(&when.event);
                    let label = event.map_or_else(
                        || format!("when {}", when.event),
                        |event| event.label.to_string(),
                    );
                    let mut rest = label.as_str();
                    if let Some(start) = rest.find("{arg}") {
                        parts.push(Part::Text(rest[..start].trim().to_string()));
                        parts.push(Part::Field(
                            FieldKind::EventArg,
                            when.arg.clone().unwrap_or_default(),
                        ));
                        rest = &rest[start + 5..];
                    } else if let Some(arg) = &when.arg {
                        parts.push(Part::Text(rest.trim().to_string()));
                        parts.push(Part::Field(FieldKind::EventArg, arg.clone()));
                        rest = "";
                    }
                    // Parameter names show as chips instead of words.
                    let words = rest
                        .split_whitespace()
                        .filter(|word| !when.params.iter().any(|param| param == word))
                        .collect::<Vec<_>>();
                    if !words.is_empty() {
                        parts.push(Part::Text(words.join(" ")));
                    }
                    for param in &when.params {
                        parts.push(Part::Chip(param.clone()));
                    }
                    parts.push(Part::Pick(PickKind::Event, String::new()));
                    let look = Look {
                        color: Cat::Events.color(),
                        segs: vec![Seg::Row(parts)],
                    };
                    let size = layout.draw_look(&look, Shape3::Hat, at, &path, HitWhat::Item);
                    let mut body_path = path.clone();
                    body_path.push(Step::Body(0));
                    size.y + layout.draw_stack(&when.body, pos2(at.x, at.y + size.y), &body_path)
                }
                ItemKind::Fn(def) => {
                    let look = Look {
                        color: Cat::Plugin.color(),
                        segs: vec![Seg::Row(vec![
                            Part::Text("define".into()),
                            Part::Field(FieldKind::FnName, def.name.clone()),
                            Part::Field(FieldKind::Params, def.params.join(", ")),
                        ])],
                    };
                    let size = layout.draw_look(&look, Shape3::Hat, at, &path, HitWhat::Item);
                    let mut body_path = path.clone();
                    body_path.push(Step::Body(0));
                    size.y + layout.draw_stack(&def.body, pos2(at.x, at.y + size.y), &body_path)
                }
                ItemKind::Keep { name, value } => {
                    let look = Look {
                        color: Cat::Variables.color(),
                        segs: vec![Seg::Row(vec![
                            Part::Text("remember".into()),
                            Part::Field(FieldKind::Name(0), name.clone()),
                            Part::Text("starting at".into()),
                            Part::Slot(0, value),
                        ])],
                    };
                    layout
                        .draw_look(&look, Shape3::Hat, at, &path, HitWhat::Item)
                        .y
                }
                ItemKind::Stmt(stmt) => {
                    // A loose stack: it runs when the plugin turns on. A note
                    // alone runs nothing.
                    let code = (index..program.items.len())
                        .take_while(|other| {
                            *other == index || continues_stack(&program.items, *other)
                        })
                        .any(|other| {
                            !matches!(
                                program.items[other].kind,
                                ItemKind::Stmt(Stmt {
                                    kind: StmtKind::Comment(_),
                                    ..
                                })
                            )
                        });
                    if code {
                        painter.text(
                            at - vec2(0.0, 4.0 * z),
                            egui::Align2::LEFT_BOTTOM,
                            "runs at start",
                            FontId::proportional(10.0 * z),
                            ui.visuals().weak_text_color(),
                        );
                    }
                    layout.sockets.push(Socket {
                        at,
                        target: SocketTarget::BeforeItem(index),
                    });
                    let look = stmt_look(stmt);
                    let size = layout.draw_look(&look, Shape3::Stack, at, &path, HitWhat::Item);
                    layout.sockets.push(Socket {
                        at: pos2(at.x, at.y + size.y),
                        target: SocketTarget::AfterItem(index),
                    });
                    stack_bottom = Some((pos2(at.x, at.y + size.y), index));
                    size.y
                }
                _ => 0.0,
            };
            if !matches!(item.kind, ItemKind::Stmt(_)) {
                stack_bottom = None;
            }
            auto_y = auto_y.max(y + height / z + 30.0);
        }

        // Snap flashes.
        let now = Instant::now();
        self.snaps
            .retain(|(_, born)| now.duration_since(*born).as_secs_f32() < 0.35);
        for (rect, born) in &self.snaps {
            let t = now.duration_since(*born).as_secs_f32() / 0.35;
            painter.rect_stroke(
                rect.expand(2.0 + 6.0 * t),
                6.0,
                Stroke::new(
                    3.0 * (1.0 - t),
                    Color32::from_white_alpha((200.0 * (1.0 - t)) as u8),
                ),
            );
        }
        if !self.snaps.is_empty() {
            ui.ctx().request_repaint();
        }

        // ---------------------------------------------------- the palette
        let palette_hits = self.palette(ui, palette_rect, &variables, &functions, theme);

        // --------------------------------------------------------- input
        let primary_pressed = ui.ctx().input(|input| input.pointer.primary_pressed());
        let primary_released = ui.ctx().input(|input| input.pointer.primary_released());
        let primary_down = ui.ctx().input(|input| input.pointer.primary_down());

        // Start of a drag: from the palette or from the canvas.
        let mut press_hit: Option<Hit> = None;
        if primary_pressed && self.drag.is_none() && self.editing.is_none() && self.popup.is_none()
        {
            if let Some(pointer) = pointer {
                if palette_rect.contains(pointer) {
                    if let Some((rect, src, item)) = palette_hits
                        .iter()
                        .rev()
                        .find(|(rect, ..)| rect.contains(pointer))
                    {
                        if let Some(payload) = palette_payload(src, *item) {
                            self.drag = Some(Drag {
                                payload,
                                grab: pointer - rect.min,
                                origin: None,
                            });
                        }
                    }
                } else if canvas_rect.contains(pointer) {
                    press_hit = layout
                        .hits
                        .iter()
                        .rev()
                        .find(|hit| hit.rect.contains(pointer))
                        .cloned();
                }
            }
        }
        // A press on the canvas becomes a drag once the pointer moves, or
        // a click when it lets go first.
        if let (Some(hit), Some(start)) = (press_hit, pointer) {
            self.pending = Some(PendingPress { hit, start });
        }
        if let Some(press) = self.pending.clone() {
            let moved = pointer.is_some_and(|pointer| (pointer - press.start).length() > 4.0);
            if primary_down && moved {
                self.pending = None;
                if let Some(drag) = self.begin_drag(program, &press.hit, press.start) {
                    self.drag = Some(drag);
                    changed = true;
                }
            } else if primary_released {
                self.pending = None;
                changed |= self.click(ui, program, &press.hit, &variables);
            } else if !primary_down {
                self.pending = None;
            }
        }

        // While dragging: the block follows the pointer, a ghost shows
        // where it would go.
        if let (Some(drag), Some(pointer)) = (&self.drag, pointer) {
            let at = pointer - drag.grab;
            let over_palette = palette_rect.contains(pointer);
            let target = if over_palette {
                None
            } else {
                self.drop_target(drag, at, pointer, &layout)
            };
            let preview = Layout {
                painter: ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    ui.id().with("dragged"),
                )),
                zoom: z,
                font: font.clone(),
                hits: Vec::new(),
                sockets: Vec::new(),
                text_dark: Color32::from_rgb(70, 70, 80),
                measuring: false,
                _p: std::marker::PhantomData,
            };
            let mut preview = preview;
            if let Some(DropTarget::Socket(socket)) = &target {
                // The grey ghost where it will click in.
                let ghost = canvas_rect
                    .intersect(Rect::from_min_size(socket.at, vec2(120.0 * z, 26.0 * z)));
                painter.rect_filled(ghost, 4.0, Color32::from_black_alpha(60));
            }
            if let Some(DropTarget::Slot(path)) = &target {
                if let Some(hit) = layout
                    .hits
                    .iter()
                    .find(|hit| hit.path == *path && hit.what == HitWhat::Slot)
                {
                    painter.rect_stroke(
                        hit.rect.expand(2.0),
                        hit.rect.height() / 2.0,
                        Stroke::new(2.5_f32, Color32::WHITE),
                    );
                }
            }
            if over_palette {
                let trash = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    ui.id().with("trash"),
                ));
                trash.rect_filled(
                    palette_rect,
                    8.0,
                    Color32::from_rgba_unmultiplied(235, 87, 87, 50),
                );
                trash.text(
                    palette_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "drop here to delete",
                    FontId::proportional(14.0),
                    Color32::WHITE,
                );
            }
            // Shadow, then the block.
            draw_payload(&mut preview, &drag.payload, at + vec2(4.0, 5.0), true);
            draw_payload(&mut preview, &drag.payload, at, false);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.ctx().request_repaint();
            if primary_released {
                let drag = self.drag.take().expect("dragging");
                if !over_palette {
                    let snapped = self.drop(program, drag, target, at, origin);
                    if let Some(rect) = snapped {
                        self.snaps.push((rect, Instant::now()));
                    }
                }
                changed = true;
            }
        } else if self.drag.is_some() && !primary_down {
            self.drag = None;
        }

        // Pan by dragging empty canvas.
        if canvas.dragged() && self.drag.is_none() && self.pending.is_none() {
            self.pan += canvas.drag_delta();
        }

        // Field editing and pickers.
        changed |= self.edit_field(ui, program);
        changed |= self.show_popup(ui, program, &variables);
        if self.new_variable.is_some() {
            changed |= self.variable_prompt(ui, program, palette_rect);
        }

        // Clean up and zoom buttons.
        let tools = Rect::from_min_size(
            canvas_rect.right_top() + vec2(-150.0, 8.0),
            vec2(142.0, 24.0),
        );
        ui.allocate_ui_at_rect(tools, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                    self.zoom = (self.zoom / 1.2).max(0.4);
                }
                if ui.small_button("=").on_hover_text("Normal size").clicked() {
                    self.zoom = 1.0;
                }
                if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                    self.zoom = (self.zoom * 1.2).min(2.5);
                }
                if ui
                    .small_button("Tidy")
                    .on_hover_text("Line the stacks up")
                    .clicked()
                {
                    for item in &mut program.items {
                        if is_code(item) {
                            item.pos = None;
                        }
                    }
                    place_stacks(program);
                    self.pan = vec2(20.0, 20.0);
                    changed = true;
                }
            });
        });
        if changed {
            place_stacks(program);
        }
        changed
    }

    /// Takes what was grabbed out of the program, to carry it.
    fn begin_drag(&mut self, program: &mut Program, hit: &Hit, start: Pos2) -> Option<Drag> {
        let grab = start - hit.rect.min;
        match &hit.what {
            HitWhat::Chip(name) => Some(Drag {
                payload: Payload::Expr(Expr::new(ExprKind::Var(name.clone()), 0)),
                grab: vec2(10.0, 10.0),
                origin: None,
            }),
            HitWhat::Item => {
                let Some(Step::Item(index)) = hit.path.first().cloned() else {
                    return None;
                };
                if hit.path.len() != 1 {
                    return None;
                }
                // A top-level stack: this item and the loose statements under it.
                let mut end = index + 1;
                while end < program.items.len() && continues_stack(&program.items, end) {
                    end += 1;
                }
                let mut items: Vec<Item> = program.items.drain(index..end).collect();
                if let Some(first) = items.first_mut() {
                    first.pos = None;
                }
                Some(Drag {
                    payload: Payload::Items(items),
                    grab,
                    origin: None,
                })
            }
            HitWhat::Stmt => {
                let (Some(Step::Stmt(index)), Some(list_path)) = (
                    hit.path.last().cloned(),
                    hit.path.split_last().map(|(_, rest)| rest),
                ) else {
                    return None;
                };
                let NodeMut::List(list) = resolve(program, list_path)? else {
                    return None;
                };
                let stmts: Vec<Stmt> = list.drain(index..).collect();
                Some(Drag {
                    payload: Payload::Stmts(stmts),
                    grab,
                    origin: None,
                })
            }
            HitWhat::Expr => {
                let expr = resolve_expr(program, &hit.path)?;
                let taken = std::mem::replace(expr, Expr::null(expr.line));
                Some(Drag {
                    payload: Payload::Expr(taken),
                    grab,
                    origin: Some(hit.path.clone()),
                })
            }
            HitWhat::Field(FieldKind::Literal, _) | HitWhat::Slot => None,
            _ => None,
        }
    }

    fn drop_target(
        &self,
        drag: &Drag,
        at: Pos2,
        pointer: Pos2,
        layout: &Layout<'_>,
    ) -> Option<DropTarget> {
        let z = self.zoom;
        match &drag.payload {
            Payload::Stmts(_) | Payload::Items(_) => {
                let hat = matches!(&drag.payload, Payload::Items(items) if items.first().is_some_and(|item| !matches!(item.kind, ItemKind::Stmt(_))));
                if hat {
                    return None;
                }
                layout
                    .sockets
                    .iter()
                    .map(|socket| (socket, (socket.at - at).length()))
                    .filter(|(_, distance)| *distance < SNAP * z)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(socket, _)| DropTarget::Socket(socket.clone()))
            }
            Payload::Expr(_) => layout
                .hits
                .iter()
                .filter(|hit| hit.what == HitWhat::Slot && hit.rect.expand(4.0).contains(pointer))
                .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
                .map(|hit| DropTarget::Slot(hit.path.clone())),
        }
    }

    /// Puts what was carried where it was let go. Gives back where it
    /// snapped in, for the flash.
    fn drop(
        &mut self,
        program: &mut Program,
        drag: Drag,
        target: Option<DropTarget>,
        at: Pos2,
        origin: Pos2,
    ) -> Option<Rect> {
        let z = self.zoom;
        let canvas_pos = (at - origin) / z;
        let stmts_of = |payload: Payload| -> Vec<Stmt> {
            match payload {
                Payload::Stmts(stmts) => stmts,
                Payload::Items(items) => items
                    .into_iter()
                    .filter_map(|item| match item.kind {
                        ItemKind::Stmt(stmt) => Some(stmt),
                        _ => None,
                    })
                    .collect(),
                Payload::Expr(expr) => {
                    let line = expr.line;
                    vec![Stmt {
                        kind: StmtKind::Expr(expr),
                        line,
                    }]
                }
            }
        };
        let origin = drag.origin;
        match (target, drag.payload) {
            (Some(DropTarget::Slot(path)), Payload::Expr(expr)) => {
                let slot = resolve_expr(program, &path)?;
                *slot = expr;
                Some(Rect::from_min_size(at, vec2(60.0, 24.0) * z))
            }
            (Some(DropTarget::Socket(socket)), payload) => {
                let stmts = stmts_of(payload);
                let count = stmts.len();
                match socket.target {
                    SocketTarget::List(path, index) => {
                        let NodeMut::List(list) = resolve(program, &path)? else {
                            return None;
                        };
                        let index = index.min(list.len());
                        for (offset, stmt) in stmts.into_iter().enumerate() {
                            list.insert(index + offset, stmt);
                        }
                    }
                    SocketTarget::AfterItem(index) => {
                        let index = (index + 1).min(program.items.len());
                        for (offset, stmt) in stmts.into_iter().enumerate() {
                            program.items.insert(
                                index + offset,
                                Item {
                                    line: stmt.line,
                                    kind: ItemKind::Stmt(stmt),
                                    pos: None,
                                },
                            );
                        }
                    }
                    SocketTarget::BeforeItem(index) => {
                        let index = index.min(program.items.len());
                        let pos = program.items.get(index).and_then(|item| item.pos);
                        if let Some(item) = program.items.get_mut(index) {
                            item.pos = None;
                        }
                        for (offset, stmt) in stmts.into_iter().enumerate() {
                            program.items.insert(
                                index + offset,
                                Item {
                                    line: stmt.line,
                                    kind: ItemKind::Stmt(stmt),
                                    pos: if offset == 0 { pos } else { None },
                                },
                            );
                        }
                    }
                }
                let _ = count;
                Some(Rect::from_min_size(socket.at, vec2(120.0, 28.0) * z))
            }
            (_, Payload::Expr(expr)) => {
                // A loose value would run as code; it goes back instead.
                if let Some(slot) = origin
                    .as_deref()
                    .and_then(|origin| resolve_expr(program, origin))
                {
                    if matches!(slot.kind, ExprKind::Null) {
                        *slot = expr;
                    }
                }
                None
            }
            (_, Payload::Items(mut items)) => {
                if let Some(first) = items.first_mut() {
                    first.pos = Some((canvas_pos.x, canvas_pos.y));
                }
                program.items.extend(items);
                None
            }
            (_, payload) => {
                let stmts = stmts_of(payload);
                for (offset, stmt) in stmts.into_iter().enumerate() {
                    program.items.push(Item {
                        line: stmt.line,
                        kind: ItemKind::Stmt(stmt),
                        pos: if offset == 0 {
                            Some((canvas_pos.x, canvas_pos.y))
                        } else {
                            None
                        },
                    });
                }
                None
            }
        }
    }

    /// A click without a drag: edit a field, open a picker, press a button.
    fn click(&mut self, ui: &Ui, program: &mut Program, hit: &Hit, _variables: &[String]) -> bool {
        match &hit.what {
            HitWhat::Field(kind, text) => {
                self.editing = Some(Editing {
                    path: hit.path.clone(),
                    kind: kind.clone(),
                    text: text.clone(),
                    rect: hit.rect,
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
                let _ = ui;
                false
            }
            HitWhat::Button(kind) => {
                match (kind, resolve(program, &hit.path)) {
                    (ButtonKind::AddItem | ButtonKind::RemoveItem, Some(NodeMut::Expr(expr))) => {
                        if let ExprKind::List(items) = &mut expr.kind {
                            if *kind == ButtonKind::AddItem {
                                items.push(Expr::new(ExprKind::Num(0.0, None), 0));
                            } else {
                                items.pop();
                            }
                        }
                    }
                    (ButtonKind::AddElse, Some(NodeMut::Stmt(stmt))) => {
                        if let StmtKind::If { otherwise, .. } = &mut stmt.kind {
                            otherwise.get_or_insert_with(|| Rc::new(Vec::new()));
                        }
                    }
                    (ButtonKind::AddElse, Some(NodeMut::Item(item))) => {
                        if let ItemKind::Stmt(Stmt {
                            kind: StmtKind::If { otherwise, .. },
                            ..
                        }) = &mut item.kind
                        {
                            otherwise.get_or_insert_with(|| Rc::new(Vec::new()));
                        }
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    /// The text box over a field being edited.
    fn edit_field(&mut self, ui: &mut Ui, program: &mut Program) -> bool {
        let Some(editing) = self.editing.as_mut() else {
            return false;
        };
        let id = ui.id().with("block_field_edit");
        let rect = Rect::from_min_size(
            editing.rect.min,
            vec2(
                editing.rect.width().max(120.0),
                editing.rect.height().max(22.0),
            ),
        );
        let response = ui.put(
            rect,
            egui::TextEdit::singleline(&mut editing.text)
                .id(id)
                .font(egui::TextStyle::Body),
        );
        if editing.fresh {
            response.request_focus();
            editing.fresh = false;
            return false;
        }
        let done =
            response.lost_focus() || ui.ctx().input(|input| input.key_pressed(egui::Key::Escape));
        if !done {
            return false;
        }
        let editing = self.editing.take().expect("editing");
        if ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)) {
            return false;
        }
        apply_field(program, &editing.path, &editing.kind, editing.text.trim())
    }

    fn show_popup(&mut self, ui: &mut Ui, program: &mut Program, variables: &[String]) -> bool {
        let Some(popup) = &self.popup else {
            return false;
        };
        let mut chosen: Option<String> = None;
        let mut close = false;
        let area = egui::Area::new(ui.id().with("block_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_height(320.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let options: Vec<(String, String)> = match popup.kind {
                            PickKind::Event => api::EVENTS
                                .iter()
                                .map(|event| {
                                    (event.event.to_string(), event.label.replace("{arg}", "…"))
                                })
                                .collect(),
                            PickKind::Operator => [
                                "+", "-", "*", "/", "%", "^", "==", "!=", "<", "<=", ">", ">=",
                                "and", "or",
                            ]
                            .iter()
                            .map(|op| (op.to_string(), op.to_string()))
                            .collect(),
                            PickKind::Variable => variables
                                .iter()
                                .map(|name| (name.clone(), name.clone()))
                                .collect(),
                            PickKind::Assign => [
                                ("=", "set to"),
                                ("+=", "change by"),
                                ("-=", "lower by"),
                                ("*=", "multiply by"),
                                ("/=", "divide by"),
                            ]
                            .iter()
                            .map(|(op, label)| (op.to_string(), label.to_string()))
                            .collect(),
                        };
                        for (value, label) in options {
                            if ui.button(label).clicked() {
                                chosen = Some(value);
                            }
                        }
                        if popup.kind == PickKind::Variable
                            && ui.button("new variable...").clicked()
                        {
                            self.new_variable = Some(String::new());
                            close = true;
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
        if let Some(value) = chosen {
            return apply_pick(program, &popup.path, popup.kind, &value);
        }
        if !close && !clicked_outside {
            self.popup = Some(popup);
        }
        false
    }

    fn variable_prompt(&mut self, ui: &mut Ui, program: &mut Program, near: Rect) -> bool {
        let mut done = None;
        egui::Window::new("New variable")
            .collapsible(false)
            .resizable(false)
            .fixed_pos(near.left_top() + vec2(20.0, 60.0))
            .show(ui.ctx(), |ui| {
                let name = self.new_variable.as_mut().expect("asking");
                ui.label("Its name: letters, digits and _.");
                let response = ui.text_edit_singleline(name);
                response.request_focus();
                ui.horizontal(|ui| {
                    if ui.button("Make it").clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                let name: String = self
                    .new_variable
                    .take()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
                    return false;
                }
                program.items.push(Item {
                    kind: ItemKind::Keep {
                        name,
                        value: Expr::new(ExprKind::Num(0.0, None), 0),
                    },
                    line: 0,
                    pos: None,
                });
                self.palette = Palette::Cat(Cat::Variables);
                true
            }
            Some(false) => {
                self.new_variable = None;
                false
            }
            None => false,
        }
    }

    /// The categories and their blocks; gives back where each block is.
    fn palette(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        variables: &[String],
        functions: &[String],
        theme: &Theme,
    ) -> Vec<(Rect, String, bool)> {
        let _ = theme;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
        // Category dots down the left.
        let column = 66.0;
        let mut y = rect.top() + 6.0;
        let mut categories: Vec<Palette> = Cat::ALL.iter().map(|cat| Palette::Cat(*cat)).collect();
        categories.insert(3, Palette::Functions);
        for palette in categories {
            let (label, color) = match palette {
                Palette::Cat(cat) => (cat.label(), cat.color()),
                Palette::Functions => ("My blocks", Cat::Plugin.color()),
            };
            let cell = Rect::from_min_size(pos2(rect.left() + 2.0, y), vec2(column - 4.0, 34.0));
            let response = ui.interact(cell, ui.id().with(("palette-cat", label)), Sense::click());
            let selected = self.palette == palette;
            if selected || response.hovered() {
                painter.rect_filled(
                    cell,
                    6.0,
                    ui.visuals()
                        .widgets
                        .hovered
                        .bg_fill
                        .gamma_multiply(if selected { 1.0 } else { 0.5 }),
                );
            }
            painter.circle_filled(pos2(cell.center().x, cell.top() + 11.0), 7.0, color);
            painter.circle_stroke(
                pos2(cell.center().x, cell.top() + 11.0),
                7.0,
                Stroke::new(1.0_f32, darker(color)),
            );
            painter.text(
                pos2(cell.center().x, cell.bottom() - 7.0),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(9.0),
                ui.visuals().text_color(),
            );
            if response.clicked() {
                self.palette = palette;
            }
            y += 36.0;
        }
        // The blocks of the chosen category, at a fixed size.
        let list_rect = Rect::from_min_max(pos2(rect.left() + column, rect.top()), rect.max);
        let mut hits = Vec::new();
        let sources = palette_sources(self.palette, variables, functions);
        let scroll_id = ui
            .id()
            .with(("palette-scroll", format!("{:?}", palette_key(self.palette))));
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
            painter: clip.clone(),
            zoom: 0.8,
            font: FontId::proportional(10.5),
            hits: Vec::new(),
            sockets: Vec::new(),
            text_dark: Color32::from_rgb(70, 70, 80),
            measuring: false,
            _p: std::marker::PhantomData,
        };
        let mut y = list_rect.top() + 10.0 - scroll;
        if self.palette == Palette::Cat(Cat::Variables) {
            let button = Rect::from_min_size(
                pos2(list_rect.left() + 8.0, y),
                vec2(list_rect.width() - 16.0, 22.0),
            );
            let response = ui.interact(
                button.intersect(list_rect),
                ui.id().with("new-variable"),
                Sense::click(),
            );
            clip.rect_filled(button, 6.0, ui.visuals().widgets.inactive.bg_fill);
            clip.text(
                button.center(),
                egui::Align2::CENTER_CENTER,
                "Make a variable",
                FontId::proportional(11.0),
                ui.visuals().text_color(),
            );
            if response.clicked() {
                self.new_variable = Some(String::new());
            }
            y += 30.0;
        }
        for (src, item) in sources {
            let Some(payload) = palette_payload(&src, item) else {
                continue;
            };
            let at = pos2(list_rect.left() + 8.0, y + if item { 2.0 } else { 0.0 });
            let before = layout.hits.len();
            let size = draw_payload(&mut layout, &payload, at, false);
            layout.hits.truncate(before);
            let block = Rect::from_min_size(at, size);
            if block.intersects(list_rect) {
                hits.push((block.intersect(list_rect), src.clone(), item));
            }
            y += size.y + 10.0;
        }
        let content = y + scroll - list_rect.top();
        scroll = scroll.clamp(0.0, (content - list_rect.height() + 20.0).max(0.0));
        ui.ctx()
            .data_mut(|data| data.insert_temp(scroll_id, scroll));
        hits
    }
}

fn palette_key(palette: Palette) -> String {
    match palette {
        Palette::Cat(cat) => cat.label().to_string(),
        Palette::Functions => "functions".into(),
    }
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

/// Draws a carried or palette block; gives back its size.
fn draw_payload(layout: &mut Layout<'_>, payload: &Payload, at: Pos2, shadow: bool) -> Vec2 {
    let path: NodePath = vec![Step::Item(usize::MAX)];
    if shadow {
        // A shadow is the outline of the block, dark and see-through.
        let size = measure_payload(layout, payload);
        layout.painter.rect_filled(
            Rect::from_min_size(at, size),
            8.0,
            Color32::from_black_alpha(55),
        );
        return size;
    }
    match payload {
        Payload::Stmts(stmts) => {
            let height = layout.draw_stack(stmts, at, &path);
            let (width, _) = layout.stack_size(stmts);
            vec2(width, height)
        }
        Payload::Expr(expr) => {
            let size = layout.expr_size(expr);
            let rect = Rect::from_min_size(at, size);
            if is_literal(expr) {
                layout.paint_field(rect, &literal_text(expr).unwrap_or_default(), false);
            } else {
                let look = expr_look(expr);
                let shape = if is_boolean(expr) {
                    Shape3::Hex
                } else {
                    Shape3::Round
                };
                layout.draw_reporter(&look, shape, rect, &path);
            }
            size
        }
        Payload::Items(items) => {
            let mut y = at.y;
            let mut width: f32 = 0.0;
            for item in items {
                let size = match &item.kind {
                    ItemKind::When(when) => {
                        let label = api::event(&when.event).map_or_else(
                            || format!("when {}", when.event),
                            |event| {
                                event.label.replace(
                                    "{arg}",
                                    &format!("“{}”", when.arg.clone().unwrap_or_default()),
                                )
                            },
                        );
                        let look = Look {
                            color: Cat::Events.color(),
                            segs: vec![Seg::Row(vec![Part::Text(label)])],
                        };
                        let size = layout.draw_look(
                            &look,
                            Shape3::Hat,
                            pos2(at.x, y),
                            &path,
                            HitWhat::Item,
                        );
                        let body = layout.draw_stack(&when.body, pos2(at.x, y + size.y), &path);
                        vec2(size.x, size.y + body)
                    }
                    ItemKind::Fn(def) => {
                        let look = Look {
                            color: Cat::Plugin.color(),
                            segs: vec![Seg::Row(vec![Part::Text(format!(
                                "define {}({})",
                                def.name,
                                def.params.join(", ")
                            ))])],
                        };
                        let size = layout.draw_look(
                            &look,
                            Shape3::Hat,
                            pos2(at.x, y),
                            &path,
                            HitWhat::Item,
                        );
                        let body = layout.draw_stack(&def.body, pos2(at.x, y + size.y), &path);
                        vec2(size.x, size.y + body)
                    }
                    ItemKind::Keep { name, value } => {
                        let look = Look {
                            color: Cat::Variables.color(),
                            segs: vec![Seg::Row(vec![
                                Part::Text("remember".into()),
                                Part::Field(FieldKind::Name(0), name.clone()),
                                Part::Text("starting at".into()),
                                Part::Slot(0, value),
                            ])],
                        };
                        layout.draw_look(&look, Shape3::Hat, pos2(at.x, y), &path, HitWhat::Item)
                    }
                    ItemKind::Stmt(stmt) => {
                        let look = stmt_look(stmt);
                        layout.draw_look(&look, Shape3::Stack, pos2(at.x, y), &path, HitWhat::Item)
                    }
                    _ => Vec2::ZERO,
                };
                y += size.y;
                width = width.max(size.x);
            }
            vec2(width, y - at.y)
        }
    }
}

fn measure_payload(layout: &mut Layout<'_>, payload: &Payload) -> Vec2 {
    let was = layout.measuring;
    layout.measuring = true;
    let before = (layout.hits.len(), layout.sockets.len());
    let size = draw_payload(layout, payload, Pos2::ZERO, false);
    layout.hits.truncate(before.0);
    layout.sockets.truncate(before.1);
    layout.measuring = was;
    size
}

/// Writes an edited field back into the program.
fn apply_field(program: &mut Program, path: &[Step], kind: &FieldKind, text: &str) -> bool {
    let name_ok = |name: &str| q0ex::printer::is_plain_name(name);
    match kind {
        FieldKind::Literal => {
            let Some(expr) = resolve_expr(program, path) else {
                return false;
            };
            let line = expr.line;
            let new = match &expr.kind {
                ExprKind::Num(..) => match parse_expr(text) {
                    Ok(
                        parsed @ Expr {
                            kind: ExprKind::Num(..),
                            ..
                        },
                    ) => parsed.kind,
                    _ => return false,
                },
                ExprKind::Bool(_) => match text {
                    "true" | "yes" | "on" => ExprKind::Bool(true),
                    "false" | "no" | "off" => ExprKind::Bool(false),
                    _ => ExprKind::Str(text.to_string()),
                },
                _ => match parse_expr(text) {
                    Ok(Expr {
                        kind: ExprKind::Num(value, unit),
                        ..
                    }) => ExprKind::Num(value, unit),
                    _ if text.is_empty() => ExprKind::Null,
                    _ if text == "true" || text == "false" => ExprKind::Bool(text == "true"),
                    _ => ExprKind::Str(text.to_string()),
                },
            };
            *expr = Expr::new(new, line);
            true
        }
        FieldKind::Template => {
            let Some(expr) = resolve_expr(program, path) else {
                return false;
            };
            let source = if text.starts_with('`') {
                text.to_string()
            } else {
                format!("`{text}`")
            };
            match parse_expr(&source) {
                Ok(parsed) => {
                    *expr = parsed;
                    true
                }
                Err(_) => false,
            }
        }
        FieldKind::Member => {
            let Some(expr) = resolve_expr(program, path) else {
                return false;
            };
            if let ExprKind::Field(_, name) = &mut expr.kind {
                if name_ok(text.trim_end_matches('!')) {
                    *name = text.to_string();
                    return true;
                }
            }
            false
        }
        FieldKind::Callee => {
            let Some(expr) = resolve_expr(program, path.get(..path.len()).unwrap_or(path)) else {
                return apply_callee_in_stmt(program, path, text);
            };
            set_callee(expr, text)
        }
        _ => apply_name_field(program, path, kind, text),
    }
}

fn set_callee(expr: &mut Expr, text: &str) -> bool {
    let ExprKind::Call { callee, .. } = &mut expr.kind else {
        return false;
    };
    let Ok(parsed) = parse_expr(text) else {
        return false;
    };
    if !matches!(parsed.kind, ExprKind::Var(_) | ExprKind::Field(..)) {
        return false;
    }
    **callee = parsed;
    true
}

fn apply_callee_in_stmt(program: &mut Program, path: &[Step], text: &str) -> bool {
    match resolve(program, path) {
        Some(NodeMut::Stmt(stmt)) => match &mut stmt.kind {
            StmtKind::Expr(expr) => set_callee(expr, text),
            _ => false,
        },
        Some(NodeMut::Item(item)) => match &mut item.kind {
            ItemKind::Stmt(Stmt {
                kind: StmtKind::Expr(expr),
                ..
            }) => set_callee(expr, text),
            _ => false,
        },
        _ => false,
    }
}

/// Names, event words, function names and parameter lists.
fn apply_name_field(program: &mut Program, path: &[Step], kind: &FieldKind, text: &str) -> bool {
    let name_ok = q0ex::printer::is_plain_name(text);
    let params: Option<Vec<String>> = {
        let list = text
            .split(',')
            .map(str::trim)
            .filter(|param| !param.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        list.iter()
            .all(|param| q0ex::printer::is_plain_name(param))
            .then_some(list)
    };
    let stmt_kind: Option<&mut StmtKind> = match resolve(program, path) {
        Some(NodeMut::Item(item)) => match &mut item.kind {
            ItemKind::When(when) => {
                return match kind {
                    FieldKind::EventArg => {
                        when.arg = (!text.is_empty()).then(|| text.to_string());
                        true
                    }
                    _ => false,
                };
            }
            ItemKind::Fn(def) => {
                return match kind {
                    FieldKind::FnName if name_ok => {
                        def.name = text.to_string();
                        true
                    }
                    FieldKind::Params => match params {
                        Some(params) => {
                            def.params = params;
                            true
                        }
                        None => false,
                    },
                    _ => false,
                };
            }
            ItemKind::Keep { name, .. } => {
                if name_ok {
                    *name = text.to_string();
                    return true;
                }
                return false;
            }
            ItemKind::Stmt(stmt) => Some(&mut stmt.kind),
            _ => None,
        },
        Some(NodeMut::Stmt(stmt)) => Some(&mut stmt.kind),
        Some(NodeMut::Expr(expr)) => {
            if let (ExprKind::Lambda { params: old, .. }, FieldKind::Params) =
                (&mut expr.kind, kind)
            {
                if let Some(params) = params {
                    *old = params;
                    return true;
                }
            }
            return false;
        }
        _ => None,
    };
    let Some(stmt_kind) = stmt_kind else {
        return false;
    };
    match (stmt_kind, kind) {
        (StmtKind::Assign { target, .. }, FieldKind::Name(0)) if name_ok => {
            target.kind = ExprKind::Var(text.to_string());
            true
        }
        (StmtKind::For { var, .. }, FieldKind::Name(0)) if name_ok => {
            *var = text.to_string();
            true
        }
        (StmtKind::For { second, .. }, FieldKind::Name(1)) => {
            *second = (name_ok).then(|| text.to_string());
            true
        }
        (StmtKind::Try { name, .. }, FieldKind::Name(0)) => {
            *name = name_ok.then(|| text.to_string());
            true
        }
        (StmtKind::Fn(def), FieldKind::FnName) if name_ok => {
            def.name = text.to_string();
            true
        }
        (StmtKind::Fn(def), FieldKind::Params) => match params {
            Some(params) => {
                def.params = params;
                true
            }
            None => false,
        },
        (StmtKind::Comment(comment), FieldKind::Comment) => {
            *comment = text.to_string();
            true
        }
        (StmtKind::Expr(expr), FieldKind::Callee) => set_callee(expr, text),
        _ => false,
    }
}

fn apply_pick(program: &mut Program, path: &[Step], kind: PickKind, value: &str) -> bool {
    match kind {
        PickKind::Event => match resolve(program, path) {
            Some(NodeMut::Item(Item {
                kind: ItemKind::When(when),
                ..
            })) => {
                when.event = value.to_string();
                if let Some(event) = api::event(value) {
                    when.params = event.params.iter().map(|param| param.to_string()).collect();
                    when.arg = event
                        .arg
                        .map(|arg| when.arg.clone().unwrap_or_else(|| arg.to_string()));
                }
                true
            }
            _ => false,
        },
        PickKind::Operator => {
            let Some(expr) = resolve_expr(program, path) else {
                return false;
            };
            let op = match value {
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Rem,
                "^" => BinOp::Pow,
                "==" => BinOp::Eq,
                "!=" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                "and" => BinOp::And,
                _ => BinOp::Or,
            };
            if let ExprKind::Binary(old, ..) = &mut expr.kind {
                *old = op;
                return true;
            }
            false
        }
        PickKind::Variable => {
            let Some(expr) = resolve_expr(program, path) else {
                return false;
            };
            expr.kind = ExprKind::Var(value.to_string());
            true
        }
        PickKind::Assign => {
            let op = match value {
                "+=" => AssignOp::Add,
                "-=" => AssignOp::Sub,
                "*=" => AssignOp::Mul,
                "/=" => AssignOp::Div,
                _ => AssignOp::Set,
            };
            let kind = match resolve(program, path) {
                Some(NodeMut::Stmt(stmt)) => &mut stmt.kind,
                Some(NodeMut::Item(Item {
                    kind: ItemKind::Stmt(stmt),
                    ..
                })) => &mut stmt.kind,
                _ => return false,
            };
            if let StmtKind::Assign { op: old, .. } = kind {
                *old = op;
                return true;
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(src: &str) -> Program {
        parse(src).unwrap()
    }

    #[test]
    fn paths_reach_bodies_slots_and_call_arguments() {
        let mut program = program("when menu \"Go\" {\n  if x > 1 {\n    say! \"hi\", 2\n  }\n}");
        let path = vec![
            Step::Item(0),
            Step::Body(0),
            Step::Stmt(0),
            Step::Body(0),
            Step::Stmt(0),
            Step::Slot(0),
            Step::Slot(1),
        ];
        let expr = resolve_expr(&mut program, &path).unwrap();
        assert_eq!(expr.kind, ExprKind::Num(2.0, None));
        let cond = resolve_expr(
            &mut program,
            &[Step::Item(0), Step::Body(0), Step::Stmt(0), Step::Slot(0)],
        )
        .unwrap();
        assert!(matches!(cond.kind, ExprKind::Binary(BinOp::Gt, ..)));
    }

    #[test]
    fn editing_a_literal_keeps_its_type() {
        let mut program = program("x = 5\ny = \"a\"");
        assert!(apply_field(
            &mut program,
            &[Step::Item(0), Step::Slot(1)],
            &FieldKind::Literal,
            "500ms"
        ));
        assert!(!apply_field(
            &mut program,
            &[Step::Item(0), Step::Slot(1)],
            &FieldKind::Literal,
            "nope"
        ));
        assert!(apply_field(
            &mut program,
            &[Step::Item(1), Step::Slot(1)],
            &FieldKind::Literal,
            "hello"
        ));
        let printed = q0ex::printer::print(&program);
        assert!(printed.contains("x = 500ms"), "{printed}");
        assert!(printed.contains("y = \"hello\""), "{printed}");
    }

    #[test]
    fn palette_blocks_are_real_code() {
        for palette in [
            Palette::Cat(Cat::Events),
            Palette::Cat(Cat::Control),
            Palette::Cat(Cat::Variables),
            Palette::Functions,
        ]
        .into_iter()
        .chain(Cat::ALL.into_iter().map(Palette::Cat))
        {
            for (src, item) in palette_sources(palette, &["score".into()], &["twice".into()]) {
                assert!(palette_payload(&src, item).is_some(), "{src}");
            }
        }
    }

    #[test]
    fn dropping_a_stack_into_a_mouth_inserts_it() {
        let mut program = program("when menu \"Go\" {\n  say! 1\n}");
        let mut editor = BlockEditor::default();
        let drag = Drag {
            payload: palette_payload("if true {\n}", false).unwrap(),
            grab: Vec2::ZERO,
            origin: None,
        };
        let target = DropTarget::Socket(Socket {
            at: Pos2::ZERO,
            target: SocketTarget::List(vec![Step::Item(0), Step::Body(0)], 1),
        });
        editor.drop(&mut program, drag, Some(target), Pos2::ZERO, Pos2::ZERO);
        let printed = q0ex::printer::print(&program);
        assert!(printed.contains("  say! 1\n  if true {"), "{printed}");
    }

    /// Runs frames of the block editor with real pointer events.
    struct Harness {
        ctx: egui::Context,
        editor: BlockEditor,
        program: Program,
        theme: Theme,
    }

    impl Harness {
        fn new(src: &str) -> Self {
            let mut harness = Self {
                ctx: egui::Context::default(),
                editor: BlockEditor::default(),
                program: program(src),
                theme: Theme::default(),
            };
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> bool {
            let mut changed = false;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))),
                events,
                ..Default::default()
            };
            let (editor, program, theme) = (&mut self.editor, &mut self.program, &self.theme);
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        changed |= editor.show(ui, program, theme);
                    });
            });
            changed
        }

        fn drag(&mut self, from: Pos2, to: Pos2) -> bool {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut changed = self.frame(vec![egui::Event::PointerMoved(from)]);
            changed |= self.frame(vec![button(from, true)]);
            changed |= self.frame(vec![egui::Event::PointerMoved(from + (to - from) * 0.5)]);
            changed |= self.frame(vec![egui::Event::PointerMoved(to)]);
            changed |= self.frame(vec![button(to, false)]);
            changed |= self.frame(Vec::new());
            changed
        }
    }

    #[test]
    fn a_hat_dragged_from_the_palette_lands_on_the_canvas() {
        let mut harness = Harness::new("x = 1");
        // The first block of the Events page: when this plugin turns on.
        let changed = harness.drag(pos2(100.0, 24.0), pos2(600.0, 300.0));
        assert!(changed);
        let printed = q0ex::printer::print(&harness.program);
        assert!(printed.contains("when enabled {"), "{printed}");
        assert!(harness
            .program
            .items
            .iter()
            .any(|item| matches!(item.kind, ItemKind::When(_)) && item.pos.is_some()));
    }

    #[test]
    fn a_block_dropped_on_the_palette_is_deleted() {
        let mut harness = Harness::new("#@ 400 200\nsay! \"bye\"");
        let printed = q0ex::printer::print(&harness.program);
        assert!(printed.contains("say!"), "{printed}");
        // The stack sits at 400, 200 on a canvas that starts after the palette.
        let canvas_left = 230.0 + 4.0;
        let block = pos2(canvas_left + 20.0 + 400.0 + 12.0, 20.0 + 200.0 + 12.0);
        assert!(harness.drag(block, pos2(60.0, 300.0)));
        assert!(!q0ex::printer::print(&harness.program).contains("say!"));
    }

    #[test]
    fn hats_triangulate_whole() {
        let look = Look {
            color: Color32::RED,
            segs: vec![Seg::Row(Vec::new())],
        };
        for width in [110.0, 160.0, 300.0] {
            let rect = Rect::from_min_size(pos2(40.0, 60.0), vec2(width, 44.0));
            let outline = block_outline(&look, &[30.0], Shape3::Hat, rect, 1.0);
            let triangles = triangulate(&outline);
            assert_eq!(triangles.len(), outline.len() - 2, "{width}: {outline:?}");
        }
    }

    #[test]
    fn outlines_triangulate_whole() {
        let look = Look {
            color: Color32::RED,
            segs: vec![
                Seg::Row(Vec::new()),
                Seg::Mouth(0, &[]),
                Seg::Row(Vec::new()),
                Seg::Mouth(1, &[]),
            ],
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 140.0));
        let outline = block_outline(&look, &[30.0, 20.0, 30.0, 20.0], Shape3::Stack, rect, 1.0);
        let triangles = triangulate(&outline);
        assert_eq!(
            triangles.len(),
            outline.len() - 2,
            "every point but two makes a triangle"
        );
    }
}
