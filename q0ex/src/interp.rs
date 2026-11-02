//! Runs q0ex.
//!
//! One [`Interp`] per plugin holds its globals, its `when` handlers and its
//! timers. Every run gets a step budget, so a plugin stuck in a loop stops
//! with an error instead of freezing the editor.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::stdlib;
pub use crate::value::Handle;
use crate::value::{Env, Function, Value};

/// What the editor lends the language: everything outside q0ex itself.
pub trait Host {
    /// Whether `name` (`editor`, `project`, `ui`, ...) is one of the host's
    /// namespaces or functions.
    fn has_namespace(&self, name: &str) -> bool;
    /// Calls a host function by its full name: `editor.goto!`, `say!`.
    fn call(&mut self, it: &mut Interp, name: &str, args: Vec<Value>) -> Result<Value, String>;
    /// Reads `handle.field`.
    fn get(&mut self, it: &mut Interp, handle: &Handle, field: &str) -> Result<Value, String>;
    /// Writes `handle.field = value`.
    fn set(
        &mut self,
        it: &mut Interp,
        handle: &Handle,
        field: &str,
        value: Value,
    ) -> Result<(), String>;
    /// Calls `handle.name(args)`.
    fn method(
        &mut self,
        it: &mut Interp,
        handle: &Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value, String>;
    /// Reads a namespace property like `config.size`; `None` when `path`
    /// is a function instead, which then stays a function value.
    fn property(&mut self, _it: &mut Interp, _path: &str) -> Option<Result<Value, String>> {
        None
    }
}

/// A host with nothing in it; `say!` and `print!` collect their text.
#[derive(Default)]
pub struct QuietHost {
    pub said: Vec<String>,
}

impl Host for QuietHost {
    fn has_namespace(&self, name: &str) -> bool {
        matches!(name, "say!" | "print!")
    }

    fn call(&mut self, _: &mut Interp, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match name {
            "say!" | "print!" => {
                let text = args
                    .iter()
                    .map(Value::to_text)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.said.push(text);
                Ok(Value::Null)
            }
            _ => Err(format!("there is no function `{name}`")),
        }
    }

    fn get(&mut self, _: &mut Interp, handle: &Handle, field: &str) -> Result<Value, String> {
        Err(format!("a {} has no `{field}`", handle.kind))
    }

    fn set(
        &mut self,
        _: &mut Interp,
        handle: &Handle,
        field: &str,
        _: Value,
    ) -> Result<(), String> {
        Err(format!("a {} has no `{field}`", handle.kind))
    }

    fn method(
        &mut self,
        _: &mut Interp,
        handle: &Handle,
        name: &str,
        _: Vec<Value>,
    ) -> Result<Value, String> {
        Err(format!("a {} cannot `{name}`", handle.kind))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunError {
    pub line: u32,
    pub message: String,
    /// Out of time or too deep: `try` cannot catch it.
    pub fatal: bool,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for RunError {}

enum Flow {
    Break,
    Continue,
    Return(Value),
    Error(RunError),
}

type R<T> = Result<T, Flow>;

/// A `when` block, ready to run.
#[derive(Debug, Clone)]
pub struct Handler {
    pub event: String,
    pub arg: Option<String>,
    pub params: Vec<String>,
    pub body: Block,
    pub line: u32,
}

struct Timer {
    due: f64,
    every: Option<f64>,
    func: Rc<Function>,
}

/// Calls deeper than this are almost surely endless recursion.
const MAX_DEPTH: u32 = 200;
/// Stack a run may use, in bytes. Plugins run on the editor's own thread,
/// whose stack is small; a debug build uses far more of it per call.
const STACK_LIMIT: usize = 384 * 1024;

/// Roughly where the stack is now.
#[inline(never)]
fn stack_here() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}
/// Timers waiting at once.
const MAX_TIMERS: usize = 1000;
/// Items a range may make.
const MAX_RANGE: f64 = 1_000_000.0;

pub struct Interp {
    pub globals: HashMap<String, Value>,
    pub handlers: Vec<Handler>,
    /// `keep` variables, saved when the plugin stops.
    pub keep_names: Vec<String>,
    /// Seconds since the plugin started, moved by [`Interp::advance`].
    pub clock: f64,
    timers: Vec<Timer>,
    steps: u64,
    depth: u32,
    stack_base: usize,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

fn error(line: u32, message: impl Into<String>) -> Flow {
    Flow::Error(RunError {
        line,
        message: message.into(),
        fatal: false,
    })
}

impl Interp {
    pub fn new() -> Self {
        Self {
            globals: HashMap::new(),
            handlers: Vec::new(),
            keep_names: Vec::new(),
            clock: 0.0,
            timers: Vec::new(),
            steps: 0,
            depth: 0,
            stack_base: 0,
        }
    }

    /// Steps left in the current run.
    pub fn steps_left(&self) -> u64 {
        self.steps
    }

    /// Sets up `program`: its functions and handlers, then runs its
    /// top-level statements. `kept` holds `keep` values saved last time.
    pub fn load(
        &mut self,
        program: &Program,
        kept: &HashMap<String, Value>,
        host: &mut dyn Host,
        budget: u64,
    ) -> Result<(), RunError> {
        self.steps = budget;
        self.stack_base = stack_here();
        // Functions first, so code above a function can call it.
        for item in &program.items {
            if let ItemKind::Fn(def) = &item.kind {
                self.globals
                    .insert(def.name.clone(), Self::function(def, None));
            }
        }
        for item in &program.items {
            match &item.kind {
                ItemKind::When(when) => self.handlers.push(Handler {
                    event: when.event.clone(),
                    arg: when.arg.clone(),
                    params: when.params.clone(),
                    body: when.body.clone(),
                    line: item.line,
                }),
                ItemKind::Keep { name, value } => {
                    let value = match kept.get(name) {
                        Some(value) => value.clone(),
                        None => self.eval(value, &None, host).map_err(Self::flow_error)?,
                    };
                    self.globals.insert(name.clone(), value);
                    if !self.keep_names.contains(name) {
                        self.keep_names.push(name.clone());
                    }
                }
                ItemKind::Stmt(stmt) => match self.exec(stmt, &None, host) {
                    Ok(()) => {}
                    Err(Flow::Error(error)) => return Err(error),
                    Err(Flow::Return(_)) => break,
                    Err(Flow::Break | Flow::Continue) => {
                        return Err(RunError {
                            line: stmt.line,
                            message: "break and continue belong inside a loop".into(),
                            fatal: false,
                        })
                    }
                },
                _ => {}
            }
        }
        Ok(())
    }

    fn flow_error(flow: Flow) -> RunError {
        match flow {
            Flow::Error(error) => error,
            _ => RunError {
                line: 0,
                message: "break, continue and return belong inside blocks".into(),
                fatal: false,
            },
        }
    }

    fn function(def: &FnDef, env: Option<Rc<Env>>) -> Value {
        Value::Func(Rc::new(Function {
            name: def.name.clone(),
            params: def.params.clone(),
            body: def.body.clone(),
            env,
        }))
    }

    /// The handlers for `event`, with their quoted argument.
    pub fn handlers_for<'a>(
        &'a self,
        event: &'a str,
    ) -> impl Iterator<Item = (usize, &'a Handler)> + 'a {
        self.handlers
            .iter()
            .enumerate()
            .filter(move |(_, handler)| handler.event == event)
    }

    pub fn has_handler(&self, event: &str) -> bool {
        self.handlers.iter().any(|handler| handler.event == event)
    }

    /// Runs handler `index`, passing `args` to its parameters.
    pub fn run_handler(
        &mut self,
        index: usize,
        args: Vec<Value>,
        host: &mut dyn Host,
        budget: u64,
    ) -> Result<Value, RunError> {
        let Some(handler) = self.handlers.get(index).cloned() else {
            return Ok(Value::Null);
        };
        self.steps = budget;
        self.depth = 0;
        self.stack_base = stack_here();
        let env = Env::child(None);
        for (index, name) in handler.params.iter().enumerate() {
            env.vars
                .borrow_mut()
                .insert(name.clone(), args.get(index).cloned().unwrap_or_default());
        }
        match self.exec_block(&handler.body, &Some(env), host) {
            Ok(()) | Err(Flow::Return(Value::Null)) => Ok(Value::Null),
            Err(Flow::Return(value)) => Ok(value),
            Err(Flow::Error(error)) => Err(error),
            Err(Flow::Break | Flow::Continue) => Ok(Value::Null),
        }
    }

    /// Calls a function value from the host, inside a run already going:
    /// `ui.row! fn() { ... }` runs its function this way.
    pub fn call_value(
        &mut self,
        func: &Value,
        args: Vec<Value>,
        host: &mut dyn Host,
    ) -> Result<Value, RunError> {
        self.call(func, args, host, 0).map_err(Self::flow_error)
    }

    /// Calls a global function by name with a fresh budget.
    pub fn call_global(
        &mut self,
        name: &str,
        args: Vec<Value>,
        host: &mut dyn Host,
        budget: u64,
    ) -> Result<Value, RunError> {
        let Some(func) = self.globals.get(name).cloned() else {
            return Err(RunError {
                line: 0,
                message: format!("there is no function `{name}`"),
                fatal: false,
            });
        };
        self.steps = budget;
        self.depth = 0;
        self.stack_base = stack_here();
        self.call_value(&func, args, host)
    }

    /// Moves the clock on by `dt` seconds and runs the timers that are due.
    pub fn advance(&mut self, dt: f64, host: &mut dyn Host, budget: u64) -> Vec<RunError> {
        self.clock += dt.max(0.0);
        let mut errors = Vec::new();
        let mut runs = 0;
        while let Some(index) = self
            .timers
            .iter()
            .enumerate()
            .filter(|(_, timer)| timer.due <= self.clock)
            .min_by(|a, b| a.1.due.total_cmp(&b.1.due))
            .map(|(index, _)| index)
        {
            runs += 1;
            if runs > MAX_TIMERS {
                break;
            }
            let timer = self.timers.swap_remove(index);
            self.steps = budget;
            self.depth = 0;
            self.stack_base = stack_here();
            let env = Env::child(timer.func.env.clone());
            let outcome = self.exec_block(&timer.func.body, &Some(env), host);
            let keep_going = match outcome {
                Ok(()) | Err(Flow::Continue | Flow::Return(_)) => true,
                Err(Flow::Break) => false,
                Err(Flow::Error(error)) => {
                    errors.push(error);
                    false
                }
            };
            if let (Some(every), true) = (timer.every, keep_going) {
                // A timer that fell behind does not try to catch up.
                let due = (timer.due + every).max(self.clock - every);
                self.timers.push(Timer {
                    due: due.max(timer.due + 0.001),
                    ..timer
                });
            }
        }
        errors
    }

    pub fn has_timers(&self) -> bool {
        !self.timers.is_empty()
    }

    pub fn clear_timers(&mut self) {
        self.timers.clear();
    }

    /// The `keep` variables as they are now.
    pub fn kept(&self) -> HashMap<String, Value> {
        self.keep_names
            .iter()
            .filter_map(|name| Some((name.clone(), self.globals.get(name)?.clone())))
            .collect()
    }

    fn tick(&mut self, line: u32) -> R<()> {
        if self.steps == 0 {
            return Err(Flow::Error(RunError {
                line,
                message: "this ran too long and was stopped (an endless loop?)".into(),
                fatal: true,
            }));
        }
        self.steps -= 1;
        Ok(())
    }

    // ----------------------------------------------------------- names

    fn lookup(&self, name: &str, env: &Option<Rc<Env>>, host: &dyn Host, line: u32) -> R<Value> {
        if let Some(value) = env.as_ref().and_then(|env| env.lookup(name)) {
            return Ok(value);
        }
        if let Some(value) = self.globals.get(name) {
            return Ok(value.clone());
        }
        if let Some(value) = stdlib::constant(name) {
            return Ok(value);
        }
        if stdlib::has(name) || host.has_namespace(name) {
            return Ok(Value::Native(Rc::from(name)));
        }
        Err(error(line, format!("`{name}` is not defined")))
    }

    fn assign_name(&mut self, name: &str, value: Value, env: &Option<Rc<Env>>) {
        let value = match env {
            Some(env) => match env.assign_existing(name, value) {
                Ok(()) => return,
                Err(value) => value,
            },
            None => value,
        };
        if let Some(slot) = self.globals.get_mut(name) {
            *slot = value;
            return;
        }
        match env {
            Some(env) => {
                env.vars.borrow_mut().insert(name.to_string(), value);
            }
            None => {
                self.globals.insert(name.to_string(), value);
            }
        }
    }

    // ------------------------------------------------------- statements

    fn exec_block(&mut self, stmts: &[Stmt], env: &Option<Rc<Env>>, host: &mut dyn Host) -> R<()> {
        for stmt in stmts {
            self.exec(stmt, env, host)?;
        }
        Ok(())
    }

    fn exec(&mut self, stmt: &Stmt, env: &Option<Rc<Env>>, host: &mut dyn Host) -> R<()> {
        let line = stmt.line;
        if !matches!(stmt.kind, StmtKind::Comment(_)) {
            self.tick(line)?;
        }
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.eval(expr, env, host)?;
            }
            StmtKind::Assign { target, op, value } => {
                let value = self.eval(value, env, host)?;
                let value = match op {
                    AssignOp::Set => value,
                    op => {
                        let current = self.eval(target, env, host)?;
                        let bin = match op {
                            AssignOp::Add => BinOp::Add,
                            AssignOp::Sub => BinOp::Sub,
                            AssignOp::Mul => BinOp::Mul,
                            _ => BinOp::Div,
                        };
                        self.binary(bin, current, value, line)?
                    }
                };
                self.assign(target, value, env, host)?;
            }
            StmtKind::Swap(a, b) => {
                let first = self.eval(a, env, host)?;
                let second = self.eval(b, env, host)?;
                self.assign(a, second, env, host)?;
                self.assign(b, first, env, host)?;
            }
            StmtKind::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    if self.eval(cond, env, host)?.truthy() {
                        return self.exec_block(body, env, host);
                    }
                }
                if let Some(body) = otherwise {
                    self.exec_block(body, env, host)?;
                }
            }
            StmtKind::While { cond, body } => {
                while self.eval(cond, env, host)?.truthy() {
                    self.tick(line)?;
                    match self.exec_block(body, env, host) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(other) => return Err(other),
                    }
                }
            }
            StmtKind::Repeat { count, body } => {
                let count = self.eval(count, env, host)?;
                let Some(count) = count.as_num() else {
                    return Err(error(
                        line,
                        format!("repeat needs a number, not {}", count.type_name()),
                    ));
                };
                let mut done = 0.0;
                while done < count.floor() {
                    done += 1.0;
                    self.tick(line)?;
                    match self.exec_block(body, env, host) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(other) => return Err(other),
                    }
                }
            }
            StmtKind::For {
                var,
                second,
                iter,
                body,
            } => {
                let iter = self.eval(iter, env, host)?;
                let pairs: Vec<(Value, Value)> = match &iter {
                    Value::List(items) => items
                        .borrow()
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.clone(), Value::Num(index as f64)))
                        .collect(),
                    Value::Map(entries) => entries
                        .borrow()
                        .iter()
                        .map(|(key, value)| (Value::str(key), value.clone()))
                        .collect(),
                    Value::Str(text) => text
                        .chars()
                        .enumerate()
                        .map(|(index, c)| (Value::str(c.to_string()), Value::Num(index as f64)))
                        .collect(),
                    Value::Num(n) => (0..n.clamp(0.0, MAX_RANGE) as i64)
                        .map(|i| (Value::Num(i as f64), Value::Num(i as f64)))
                        .collect(),
                    other => {
                        return Err(error(
                            line,
                            format!(
                                "for goes through a list, a map or text, not {}",
                                other.type_name()
                            ),
                        ))
                    }
                };
                for (first, other) in pairs {
                    self.tick(line)?;
                    self.assign_name(var, first, env);
                    if let Some(second) = second {
                        self.assign_name(second, other, env);
                    }
                    match self.exec_block(body, env, host) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(other) => return Err(other),
                    }
                }
            }
            StmtKind::Break => return Err(Flow::Break),
            StmtKind::Continue => return Err(Flow::Continue),
            StmtKind::Return(value) => {
                let value = match value {
                    Some(value) => self.eval(value, env, host)?,
                    None => Value::Null,
                };
                return Err(Flow::Return(value));
            }
            StmtKind::Try {
                body,
                name,
                handler,
            } => match self.exec_block(body, env, host) {
                Err(Flow::Error(failure)) if !failure.fatal => {
                    if let Some(name) = name {
                        self.assign_name(name, Value::str(&failure.message), env);
                    }
                    self.exec_block(handler, env, host)?;
                }
                other => other?,
            },
            StmtKind::Do(body) => self.exec_block(body, env, host)?,
            StmtKind::Later { delay, body }
            | StmtKind::Every {
                interval: delay,
                body,
            } => {
                let seconds = self.eval(delay, env, host)?;
                let Some(seconds) = seconds.as_num().filter(|s| s.is_finite()) else {
                    return Err(error(line, "a timer needs a time, like 2s or 500ms"));
                };
                if self.timers.len() >= MAX_TIMERS {
                    return Err(error(line, "too many timers are waiting"));
                }
                let every = matches!(stmt.kind, StmtKind::Every { .. });
                // An every! faster than a frame still waits at least a moment.
                let seconds = if every {
                    seconds.max(0.001)
                } else {
                    seconds.max(0.0)
                };
                self.timers.push(Timer {
                    due: self.clock + seconds,
                    every: every.then_some(seconds),
                    func: Rc::new(Function {
                        name: if every { "every!" } else { "later!" }.into(),
                        params: Vec::new(),
                        body: body.clone(),
                        env: env.clone(),
                    }),
                });
            }
            StmtKind::Fn(def) => {
                let func = Self::function(def, env.clone());
                self.assign_name(&def.name, func, env);
            }
            StmtKind::Comment(_) => {}
        }
        Ok(())
    }

    fn assign(
        &mut self,
        target: &Expr,
        value: Value,
        env: &Option<Rc<Env>>,
        host: &mut dyn Host,
    ) -> R<()> {
        let line = target.line;
        match &target.kind {
            ExprKind::Var(name) => {
                // A plugin may reuse a built-in name for its own variable.
                self.assign_name(name, value, env);
                Ok(())
            }
            ExprKind::Field(object, field) => {
                let object = self.eval(object, env, host)?;
                match object {
                    Value::Map(entries) => {
                        entries.borrow_mut().insert(field.clone(), value);
                        Ok(())
                    }
                    Value::Handle(handle) => host
                        .set(self, &handle, field, value)
                        .map_err(|message| error(line, message)),
                    Value::Native(name) => host
                        .call(self, &format!("{name}.{field}="), vec![value])
                        .map(|_| ())
                        .map_err(|message| error(line, message)),
                    other => Err(error(
                        line,
                        format!("a {} has no fields to set", other.type_name()),
                    )),
                }
            }
            ExprKind::Index(object, index) => {
                let object = self.eval(object, env, host)?;
                let index = self.eval(index, env, host)?;
                match (object, index) {
                    (Value::List(items), Value::Num(index)) => {
                        let mut items = items.borrow_mut();
                        let len = items.len() as i64;
                        let at = if index < 0.0 {
                            len + index as i64
                        } else {
                            index as i64
                        };
                        if at < 0 || at >= len {
                            return Err(error(
                                line,
                                format!("item {index} is past the end of a list of {len}"),
                            ));
                        }
                        items[at as usize] = value;
                        Ok(())
                    }
                    (Value::Map(entries), key) => {
                        entries.borrow_mut().insert(key.to_text(), value);
                        Ok(())
                    }
                    (object, index) => Err(error(
                        line,
                        format!(
                            "cannot set item {} of a {}",
                            index.repr(),
                            object.type_name()
                        ),
                    )),
                }
            }
            _ => Err(error(line, "only a name, a field or an item can be set")),
        }
    }

    // ------------------------------------------------------ expressions

    fn eval(&mut self, expr: &Expr, env: &Option<Rc<Env>>, host: &mut dyn Host) -> R<Value> {
        let line = expr.line;
        Ok(match &expr.kind {
            ExprKind::Null => Value::Null,
            ExprKind::Bool(value) => Value::Bool(*value),
            ExprKind::Num(value, unit) => Value::Num(value * unit.map_or(1.0, Unit::scale)),
            ExprKind::Str(text) => Value::str(text),
            ExprKind::Template(parts) => {
                let mut text = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Text(piece) => text.push_str(piece),
                        TemplatePart::Expr(inner) => {
                            text.push_str(&self.eval(inner, env, host)?.to_text())
                        }
                    }
                }
                Value::str(text)
            }
            ExprKind::Var(name) => self.lookup(name, env, host, line)?,
            ExprKind::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env, host)?);
                }
                Value::list(values)
            }
            ExprKind::Map(entries) => {
                let mut values = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    values.push((key.clone(), self.eval(value, env, host)?));
                }
                Value::map(values)
            }
            ExprKind::Field(object, field) => {
                let object = self.eval(object, env, host)?;
                self.field(object, field, host, line)?
            }
            ExprKind::Index(object, index) => {
                let object = self.eval(object, env, host)?;
                let index = self.eval(index, env, host)?;
                index_value(&object, &index).map_err(|message| error(line, message))?
            }
            ExprKind::Call { callee, args, .. } => {
                // `thing.method(args)` on an editor object goes to the host.
                if let ExprKind::Field(object, name) = &callee.kind {
                    let object = self.eval(object, env, host)?;
                    let args = self.eval_args(args, env, host)?;
                    if let Value::Handle(handle) = &object {
                        return host
                            .method(self, handle, name, args)
                            .map_err(|message| error(line, message));
                    }
                    let func = self.field(object, name, host, line)?;
                    return self.call(&func, args, host, line);
                }
                let func = self.eval(callee, env, host)?;
                let args = self.eval_args(args, env, host)?;
                self.call(&func, args, host, line)?
            }
            ExprKind::Unary(op, operand) => {
                let value = self.eval(operand, env, host)?;
                match op {
                    UnOp::Not => Value::Bool(!value.truthy()),
                    UnOp::Neg => match value {
                        Value::Num(n) => Value::Num(-n),
                        other => {
                            return Err(error(
                                line,
                                format!("cannot negate a {}", other.type_name()),
                            ))
                        }
                    },
                }
            }
            ExprKind::Binary(BinOp::And, left, right) => {
                let left = self.eval(left, env, host)?;
                if !left.truthy() {
                    left
                } else {
                    self.eval(right, env, host)?
                }
            }
            ExprKind::Binary(BinOp::Or, left, right) => {
                let left = self.eval(left, env, host)?;
                if left.truthy() {
                    left
                } else {
                    self.eval(right, env, host)?
                }
            }
            ExprKind::Binary(op, left, right) => {
                let left = self.eval(left, env, host)?;
                let right = self.eval(right, env, host)?;
                self.binary(*op, left, right, line)?
            }
            ExprKind::Range {
                from,
                to,
                inclusive,
            } => {
                let from = self.eval(from, env, host)?;
                let to = self.eval(to, env, host)?;
                let (Some(from), Some(to)) = (from.as_num(), to.as_num()) else {
                    return Err(error(line, "a range goes between two numbers"));
                };
                stdlib::range(from, to, if from <= to { 1.0 } else { -1.0 }, *inclusive)
                    .map_err(|message| error(line, message))?
            }
            ExprKind::Pipe(value, target) => {
                let value = self.eval(value, env, host)?;
                match &target.kind {
                    ExprKind::Call { callee, args, .. } => {
                        let mut all = vec![value];
                        all.extend(self.eval_args(args, env, host)?);
                        if let ExprKind::Field(object, name) = &callee.kind {
                            let object = self.eval(object, env, host)?;
                            if let Value::Handle(handle) = &object {
                                return host
                                    .method(self, handle, name, all)
                                    .map_err(|message| error(line, message));
                            }
                            let func = self.field(object, name, host, line)?;
                            return self.call(&func, all, host, line);
                        }
                        let func = self.eval(callee, env, host)?;
                        self.call(&func, all, host, line)?
                    }
                    _ => {
                        let func = self.eval(target, env, host)?;
                        self.call(&func, vec![value], host, line)?
                    }
                }
            }
            ExprKind::Lambda { params, body } => Value::Func(Rc::new(Function {
                name: "fn".into(),
                params: params.clone(),
                body: body.clone(),
                env: env.clone(),
            })),
        })
    }

    fn eval_args(
        &mut self,
        args: &[Expr],
        env: &Option<Rc<Env>>,
        host: &mut dyn Host,
    ) -> R<Vec<Value>> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(arg, env, host)?);
        }
        Ok(values)
    }

    fn field(&mut self, object: Value, field: &str, host: &mut dyn Host, line: u32) -> R<Value> {
        match object {
            Value::Map(entries) => Ok(entries.borrow().get(field).cloned().unwrap_or_default()),
            Value::Native(name) => {
                let path = format!("{name}.{field}");
                match host.property(self, &path) {
                    Some(value) => value.map_err(|message| error(line, message)),
                    None => Ok(Value::Native(Rc::from(path))),
                }
            }
            Value::Handle(handle) => host
                .get(self, &handle, field)
                .map_err(|message| error(line, message)),
            Value::Null => Err(error(line, format!("cannot read `{field}` of null"))),
            other => Err(error(
                line,
                format!("a {} has no `{field}`", other.type_name()),
            )),
        }
    }

    fn call(&mut self, func: &Value, args: Vec<Value>, host: &mut dyn Host, line: u32) -> R<Value> {
        match func {
            Value::Func(function) => {
                self.tick(line)?;
                let used = self.stack_base.abs_diff(stack_here());
                if self.depth >= MAX_DEPTH || (self.stack_base != 0 && used > STACK_LIMIT) {
                    return Err(Flow::Error(RunError {
                        line,
                        message: "calls went too deep (a function calling itself forever?)".into(),
                        fatal: true,
                    }));
                }
                let env = Env::child(function.env.clone());
                for (index, name) in function.params.iter().enumerate() {
                    env.vars
                        .borrow_mut()
                        .insert(name.clone(), args.get(index).cloned().unwrap_or_default());
                }
                self.depth += 1;
                let outcome = self.exec_block(&function.body, &Some(env), host);
                self.depth -= 1;
                match outcome {
                    Ok(()) => Ok(Value::Null),
                    Err(Flow::Return(value)) => Ok(value),
                    Err(Flow::Break | Flow::Continue) => Ok(Value::Null),
                    Err(error) => Err(error),
                }
            }
            Value::Native(name) => {
                self.tick(line)?;
                let outcome = if stdlib::has(name) {
                    stdlib::call(self, name, args, host)
                } else {
                    host.call(self, name, args)
                };
                outcome.map_err(|message| {
                    // A fatal error from inside a callback keeps its line.
                    error(line, message)
                })
            }
            other => Err(error(
                line,
                format!("a {} cannot be called", other.type_name()),
            )),
        }
    }

    /// Calls a function value for the standard library (`map`, `sort!`).
    pub(crate) fn call_for_stdlib(
        &mut self,
        func: &Value,
        args: Vec<Value>,
        host: &mut dyn Host,
    ) -> Result<Value, String> {
        self.call(func, args, host, 0).map_err(|flow| match flow {
            Flow::Error(error) => error.message,
            _ => "break and continue belong inside a loop".into(),
        })
    }

    fn binary(&mut self, op: BinOp, left: Value, right: Value, line: u32) -> R<Value> {
        stdlib::binary(op, &left, &right).map_err(|message| error(line, message))
    }
}

/// `object[index]`.
pub fn index_value(object: &Value, index: &Value) -> Result<Value, String> {
    match (object, index) {
        (Value::List(items), Value::Num(index)) => {
            let items = items.borrow();
            let len = items.len() as i64;
            let at = if *index < 0.0 {
                len + *index as i64
            } else {
                *index as i64
            };
            Ok(if at >= 0 && at < len {
                items[at as usize].clone()
            } else {
                Value::Null
            })
        }
        (Value::Map(entries), key) => Ok(entries
            .borrow()
            .get(&key.to_text())
            .cloned()
            .unwrap_or_default()),
        (Value::Str(text), Value::Num(index)) => {
            let len = text.chars().count() as i64;
            let at = if *index < 0.0 {
                len + *index as i64
            } else {
                *index as i64
            };
            Ok(if at >= 0 && at < len {
                Value::str(
                    text.chars()
                        .nth(at as usize)
                        .unwrap_or_default()
                        .to_string(),
                )
            } else {
                Value::Null
            })
        }
        (object, index) => Err(format!(
            "cannot take item {} of a {}",
            index.repr(),
            object.type_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn run(src: &str) -> (Interp, QuietHost) {
        let program = parse(src).unwrap_or_else(|error| panic!("{error}"));
        let mut it = Interp::new();
        let mut host = QuietHost::default();
        it.load(&program, &HashMap::new(), &mut host, 100_000)
            .unwrap_or_else(|error| panic!("{error}"));
        (it, host)
    }

    fn said(src: &str) -> Vec<String> {
        run(src).1.said
    }

    #[test]
    fn arithmetic_text_and_templates() {
        assert_eq!(
            said("x = 2 + 3 * 4\nsay! x, \"a\" + 1, `x is {x - 4}!`, -2 ^ 2, 7 % 4, \"ab\" * 3"),
            ["14 a1 x is 10! -4 3 ababab"]
        );
    }

    #[test]
    fn functions_closures_and_scopes() {
        let out = said(
            "count = 0\nfn bump(n) {\n  count += n\n  temp = 1\n  return count\n}\nbump(2)\nbump(3)\nsay! count\n\
             fn counter() {\n  n = 0\n  return fn() {\n    n += 1\n    return n\n  }\n}\nc = counter()\nc()\nsay! c()",
        );
        assert_eq!(out, ["5", "2"]);
        let (it, _) = run("fn f() {\n  temp = 1\n}\nf()");
        assert!(!it.globals.contains_key("temp"), "locals stay local");
    }

    #[test]
    fn loops_ranges_and_lists() {
        assert_eq!(
            said("total = 0\nfor i in 1..4 { total += i }\nfor i in 0..<3 { total += 100 }\nsay! total"),
            ["310"]
        );
        assert_eq!(
            said("l = [3, 1, 2]\nsort! l\npush! l, 9\nsay! l, len(l), l[-1], map(l, fn(x) {\n  return x * 2\n}) |> sum()"),
            ["[1, 2, 3, 9] 4 9 30"]
        );
        assert_eq!(
            said("n = 0\nrepeat 5 {\n  n += 1\n  if n == 3 { break }\n}\nsay! n"),
            ["3"]
        );
        assert_eq!(
            said("m = {a: 1, b: 2}\nfor k, v in m { say! k, v }\nm.c = 3\nsay! keys(m), m[\"c\"]"),
            ["a 1", "b 2", "[\"a\", \"b\", \"c\"] 3"]
        );
    }

    #[test]
    fn swaps_pipes_and_and_or() {
        assert_eq!(
            said("a = 1\nb = 2\na <-> b\nsay! a, b, null or \"default\", 0 and 5, [1, 2, 3] |> len()"),
            ["2 1 default 0 3"]
        );
    }

    #[test]
    fn try_catches_oops_but_not_endless_loops() {
        assert_eq!(
            said("try {\n  oops! \"bad thing\"\n  say! \"never\"\n} oops why {\n  say! `caught: {why}`\n}"),
            ["caught: bad thing"]
        );
        let program = parse("try {\n  while true { }\n} oops {\n  say! \"no\"\n}").unwrap();
        let mut it = Interp::new();
        let mut host = QuietHost::default();
        let error = it
            .load(&program, &HashMap::new(), &mut host, 10_000)
            .unwrap_err();
        assert!(error.fatal);
        assert!(host.said.is_empty());
    }

    #[test]
    fn recursion_is_capped() {
        let program = parse("fn f(n) {\n  return f(n + 1)\n}\nf(0)").unwrap();
        let mut it = Interp::new();
        let error = it
            .load(
                &program,
                &HashMap::new(),
                &mut QuietHost::default(),
                1_000_000,
            )
            .unwrap_err();
        assert!(error.message.contains("deep"), "{error}");
    }

    #[test]
    fn handlers_get_their_arguments() {
        let (mut it, mut host) =
            run("total = 0\nwhen frame n {\n  total += n\n  say! `frame {n}`\n}");
        let (index, handler) = it.handlers_for("frame").next().unwrap();
        assert_eq!(handler.params, ["n"]);
        it.run_handler(index, vec![Value::Num(7.0)], &mut host, 1000)
            .unwrap();
        assert_eq!(host.said, ["frame 7"]);
        assert_eq!(it.globals["total"], Value::Num(7.0));
    }

    #[test]
    fn timers_wait_repeat_and_stop_with_break() {
        let (mut it, mut host) = run(
            "ticks = 0\nlater! 500ms { say! \"later\" }\nevery! 1s {\n  ticks += 1\n  if ticks == 3 { break }\n}",
        );
        assert!(it.advance(0.4, &mut host, 1000).is_empty());
        assert!(host.said.is_empty());
        it.advance(0.2, &mut host, 1000);
        assert_eq!(host.said, ["later"]);
        for _ in 0..10 {
            it.advance(1.0, &mut host, 1000);
        }
        assert_eq!(it.globals["ticks"], Value::Num(3.0));
        assert!(!it.has_timers());
    }

    #[test]
    fn keep_uses_the_saved_value() {
        let program = parse("keep clicks = 0\nclicks += 1").unwrap();
        let mut it = Interp::new();
        let kept = HashMap::from([("clicks".to_string(), Value::Num(41.0))]);
        it.load(&program, &kept, &mut QuietHost::default(), 1000)
            .unwrap();
        assert_eq!(it.kept()["clicks"], Value::Num(42.0));
    }

    #[test]
    fn unknown_names_are_errors_with_lines() {
        let program = parse("x = 1\ny = x + nope").unwrap();
        let error = Interp::new()
            .load(&program, &HashMap::new(), &mut QuietHost::default(), 1000)
            .unwrap_err();
        assert_eq!(error.line, 2);
        assert!(error.message.contains("nope"));
    }

    #[test]
    fn do_runs_in_place() {
        assert_eq!(said("do! {\n  x = 4\n}\nsay! x"), ["4"]);
    }
}
