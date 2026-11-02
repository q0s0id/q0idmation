//! What q0ex values are made of.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::ast::Block;
use crate::printer::number;

pub type List = Rc<RefCell<Vec<Value>>>;
pub type Map = Rc<RefCell<BTreeMap<String, Value>>>;

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    /// Lists and maps are shared: two names for one list see each other's
    /// changes.
    List(List),
    Map(Map),
    Func(Rc<Function>),
    /// A built-in or host function, or a namespace, by its full name:
    /// `len`, `editor`, `editor.goto!`.
    Native(Rc<str>),
    /// Something that lives in the editor: a layer, an object, an asset.
    Handle(Rc<Handle>),
}

/// A function written in q0ex, with the scope it was made in.
pub struct Function {
    pub name: String,
    pub params: Vec<String>,
    pub body: Block,
    pub env: Option<Rc<Env>>,
}

/// Local variables of one call, and the calls around it for closures.
#[derive(Default)]
pub struct Env {
    pub vars: RefCell<HashMap<String, Value>>,
    pub parent: Option<Rc<Env>>,
}

impl Env {
    pub fn child(parent: Option<Rc<Env>>) -> Rc<Env> {
        Rc::new(Env {
            vars: RefCell::new(HashMap::new()),
            parent,
        })
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.vars.borrow().get(name) {
            return Some(value.clone());
        }
        self.parent.as_ref()?.lookup(name)
    }

    /// Sets `name` where it already lives; false when it lives nowhere.
    pub fn assign_existing(&self, name: &str, value: Value) -> Result<(), Value> {
        if let Some(slot) = self.vars.borrow_mut().get_mut(name) {
            *slot = value;
            return Ok(());
        }
        match &self.parent {
            Some(parent) => parent.assign_existing(name, value),
            None => Err(value),
        }
    }
}

/// A reference to an editor object. The host decides what `kind` and `ids`
/// mean; q0ex only carries them around.
#[derive(Debug, Clone, PartialEq)]
pub struct Handle {
    pub kind: String,
    pub ids: Vec<i64>,
}

impl Value {
    pub fn str(text: impl AsRef<str>) -> Self {
        Self::Str(Rc::from(text.as_ref()))
    }

    pub fn list(items: Vec<Value>) -> Self {
        Self::List(Rc::new(RefCell::new(items)))
    }

    pub fn map(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self::Map(Rc::new(RefCell::new(entries.into_iter().collect())))
    }

    pub fn handle(kind: &str, ids: Vec<i64>) -> Self {
        Self::Handle(Rc::new(Handle {
            kind: kind.into(),
            ids,
        }))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Num(_) => "number",
            Self::Str(_) => "text",
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Func(_) | Self::Native(_) => "function",
            Self::Handle(_) => "object",
        }
    }

    /// `false`, `null`, `0`, `nan`, `""` and empty lists and maps are false.
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Num(value) => *value != 0.0 && !value.is_nan(),
            Self::Str(text) => !text.is_empty(),
            Self::List(items) => !items.borrow().is_empty(),
            Self::Map(entries) => !entries.borrow().is_empty(),
            Self::Func(_) | Self::Native(_) | Self::Handle(_) => true,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The text `say!` and `+` with text use.
    pub fn to_text(&self) -> String {
        match self {
            Self::Str(text) => text.to_string(),
            other => other.repr(),
        }
    }

    /// How the value looks written down: text in quotes.
    pub fn repr(&self) -> String {
        self.repr_depth(0)
    }

    fn repr_depth(&self, depth: usize) -> String {
        if depth > 8 {
            return "...".into();
        }
        match self {
            Self::Null => "null".into(),
            Self::Bool(value) => value.to_string(),
            Self::Num(value) => {
                if value.is_nan() {
                    "nan".into()
                } else if value.is_infinite() {
                    if *value > 0.0 {
                        "infinity"
                    } else {
                        "-infinity"
                    }
                    .into()
                } else {
                    number(*value)
                }
            }
            Self::Str(text) => crate::printer::quote(text),
            Self::List(items) => {
                let items = items.borrow();
                let shown = items
                    .iter()
                    .take(50)
                    .map(|item| item.repr_depth(depth + 1))
                    .collect::<Vec<_>>();
                let more = if items.len() > 50 {
                    format!(", ... {} more", items.len() - 50)
                } else {
                    String::new()
                };
                format!("[{}{more}]", shown.join(", "))
            }
            Self::Map(entries) => {
                let entries = entries.borrow();
                let shown = entries
                    .iter()
                    .take(50)
                    .map(|(key, value)| format!("{key}: {}", value.repr_depth(depth + 1)))
                    .collect::<Vec<_>>();
                format!("{{{}}}", shown.join(", "))
            }
            Self::Func(function) => format!("<fn {}>", function.name),
            Self::Native(name) => format!("<{name}>"),
            Self::Handle(handle) => {
                let ids = handle
                    .ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>();
                format!("<{} {}>", handle.kind, ids.join(" "))
            }
        }
    }

    /// `==`: by value for lists and maps, never across types.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Num(a), Self::Num(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::List(a), Self::List(b)) => {
                Rc::ptr_eq(a, b) || {
                    let (a, b) = (a.borrow(), b.borrow());
                    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
                }
            }
            (Self::Map(a), Self::Map(b)) => {
                Rc::ptr_eq(a, b) || {
                    let (a, b) = (a.borrow(), b.borrow());
                    a.len() == b.len()
                        && a.iter()
                            .zip(b.iter())
                            .all(|((ka, va), (kb, vb))| ka == kb && va.equals(vb))
                }
            }
            (Self::Func(a), Self::Func(b)) => Rc::ptr_eq(a, b),
            (Self::Native(a), Self::Native(b)) => a == b,
            (Self::Handle(a), Self::Handle(b)) => a == b,
            _ => false,
        }
    }

    /// A deep copy: lists and maps no longer shared.
    pub fn deep_copy(&self) -> Value {
        match self {
            Self::List(items) => Value::list(items.borrow().iter().map(Value::deep_copy).collect()),
            Self::Map(entries) => Value::map(
                entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.deep_copy())),
            ),
            other => other.clone(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_depth(0)
    }

    fn to_json_depth(&self, depth: usize) -> serde_json::Value {
        use serde_json::Value as J;
        if depth > 64 {
            return J::Null;
        }
        match self {
            Self::Null | Self::Func(_) | Self::Native(_) => J::Null,
            Self::Bool(value) => J::Bool(*value),
            Self::Num(value) => {
                if value.fract() == 0.0 && value.abs() < 9.0e15 {
                    J::from(*value as i64)
                } else {
                    serde_json::Number::from_f64(*value).map_or(J::Null, J::Number)
                }
            }
            Self::Str(text) => J::String(text.to_string()),
            Self::List(items) => J::Array(
                items
                    .borrow()
                    .iter()
                    .map(|item| item.to_json_depth(depth + 1))
                    .collect(),
            ),
            Self::Map(entries) => J::Object(
                entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json_depth(depth + 1)))
                    .collect(),
            ),
            Self::Handle(handle) => {
                let mut object = serde_json::Map::new();
                object.insert("$object".into(), J::String(handle.kind.clone()));
                object.insert("ids".into(), J::from(handle.ids.clone()));
                J::Object(object)
            }
        }
    }

    pub fn from_json(json: &serde_json::Value) -> Value {
        use serde_json::Value as J;
        match json {
            J::Null => Value::Null,
            J::Bool(value) => Value::Bool(*value),
            J::Number(number) => Value::Num(number.as_f64().unwrap_or(0.0)),
            J::String(text) => Value::str(text),
            J::Array(items) => Value::list(items.iter().map(Value::from_json).collect()),
            J::Object(entries) => {
                if let (Some(J::String(kind)), Some(J::Array(ids)), 2) =
                    (entries.get("$object"), entries.get("ids"), entries.len())
                {
                    return Value::handle(kind, ids.iter().filter_map(J::as_i64).collect());
                }
                Value::map(
                    entries
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::from_json(value))),
                )
            }
        }
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.repr())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Num(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::str(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::str(value)
    }
}
