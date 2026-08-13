//! Minimal q0lang runtime shared by editor and player.
//!
//! This layer executes the parser AST into a small isolated state. It does not
//! touch the editor/player timeline directly yet; instead it emits actions
//! that the host can apply later.

use std::collections::{BTreeMap, BTreeSet};

use super::parser::{
    self, BinaryOp, Diagnostic, Program, Statement, TimelineSignal, UnaryOp, Value, Visibility,
};

const MAX_DO_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Runtime {
    pub vars: BTreeMap<String, RuntimeValue>,
    pub listeners: BTreeMap<String, Vec<String>>,
    pub imported_libraries: BTreeSet<String>,
    pub public_functions: BTreeMap<String, FunctionSignature>,
    pub private_functions: BTreeMap<String, FunctionSignature>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            vars: BTreeMap::new(),
            listeners: BTreeMap::new(),
            imported_libraries: BTreeSet::new(),
            public_functions: BTreeMap::new(),
            private_functions: BTreeMap::new(),
        }
    }

    pub fn execute_source(&mut self, source: &str) -> ExecutionReport {
        let program = parser::parse(source);
        self.execute_program(&program)
    }

    pub fn execute_program(&mut self, program: &Program) -> ExecutionReport {
        let mut report = ExecutionReport {
            actions: Vec::new(),
            diagnostics: program.diagnostics.clone(),
        };
        self.execute_statements(&program.statements, &mut report, 0);
        report
    }

    fn execute_statements(
        &mut self,
        statements: &[Statement],
        report: &mut ExecutionReport,
        depth: usize,
    ) {
        if depth > MAX_DO_DEPTH {
            report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "do! nesting is too deep for the script summary",
            ));
            return;
        }

        for statement in statements {
            match statement {
                Statement::Assignment { name, value } => {
                    let value = self.resolve_runtime_value(value, report);
                    self.vars.insert(name.clone(), value);
                }
                Statement::TimelineSignal { kind, target } => {
                    let target = self.resolve_timeline_target(target, report);
                    let action = match kind {
                        TimelineSignal::GoRun => RuntimeAction::GoRun(target),
                        TimelineSignal::GoStop => RuntimeAction::GoStop(target),
                    };
                    report.actions.push(action);
                }
                Statement::ShellCommand { command, args } => {
                    let args = args
                        .iter()
                        .map(|arg| self.resolve_runtime_value(arg, report))
                        .collect();
                    report.actions.push(RuntimeAction::ShellCommand {
                        command: command.clone(),
                        args,
                    });
                }
                Statement::RigCommand { command, args } => {
                    let args = args
                        .iter()
                        .map(|arg| self.resolve_runtime_value(arg, report))
                        .collect::<Vec<_>>();
                    self.emit_rig_command(command, &args, report);
                }
                Statement::Listen { event, handler } => {
                    let handlers = self.listeners.entry(event.clone()).or_default();
                    if !handlers.iter().any(|h| h == handler) {
                        handlers.push(handler.clone());
                    }
                }
                Statement::Unlisten { event, handler } => {
                    if let Some(handlers) = self.listeners.get_mut(event) {
                        handlers.retain(|h| h != handler);
                    }
                    if self
                        .listeners
                        .get(event)
                        .map(|handlers| handlers.is_empty())
                        .unwrap_or(false)
                    {
                        self.listeners.remove(event);
                    }
                }
                Statement::Function {
                    visibility,
                    name,
                    params,
                } => {
                    let signature = FunctionSignature {
                        name: name.clone(),
                        params: params.clone(),
                    };
                    match visibility {
                        Visibility::Public => {
                            self.public_functions.insert(name.clone(), signature);
                        }
                        Visibility::Private => {
                            self.private_functions.insert(name.clone(), signature);
                        }
                    }
                }
                Statement::DoBlock { body } => {
                    let nested = parser::parse(body);
                    report.diagnostics.extend(nested.diagnostics);
                    self.execute_statements(&nested.statements, report, depth + 1);
                }
                Statement::Import { library } => {
                    self.imported_libraries.insert(library.clone());
                }
                Statement::Unknown { text } => {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        format!("cannot execute unknown statement `{text}`"),
                    ));
                }
            }
        }
    }

    fn emit_rig_command(&self, command: &str, args: &[RuntimeValue], report: &mut ExecutionReport) {
        let control_name = |value: &RuntimeValue| match value {
            RuntimeValue::String(name) | RuntimeValue::Ident(name) | RuntimeValue::Raw(name) => {
                Some(name.clone())
            }
            RuntimeValue::Number(_) => None,
        };
        match (command, args) {
            ("position", [name, RuntimeValue::Number(x), RuntimeValue::Number(y)]) => {
                if let Some(control) = control_name(name) {
                    report.actions.push(RuntimeAction::RigSetPosition {
                        control,
                        x: *x,
                        y: *y,
                    });
                } else {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        "q0rig.position! expects a control name followed by x, y numbers",
                    ));
                }
            }
            ("value", [name, RuntimeValue::Number(value)]) => {
                if let Some(control) = control_name(name) {
                    report.actions.push(RuntimeAction::RigSetValue {
                        control,
                        value: *value,
                    });
                } else {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        "q0rig.value! expects a control name followed by a number",
                    ));
                }
            }
            ("reset", [name]) => {
                if let Some(control) = control_name(name) {
                    report.actions.push(RuntimeAction::RigReset { control });
                } else {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        "q0rig.reset! expects a control name",
                    ));
                }
            }
            ("pose", [name, RuntimeValue::Number(weight)]) => {
                if let Some(pose) = control_name(name) {
                    if weight.is_finite() {
                        report.actions.push(RuntimeAction::RigSetPose {
                            pose,
                            weight: weight.clamp(0.0, 1.0),
                        });
                    } else {
                        report.diagnostics.push(RuntimeDiagnostic::at_line(
                            0,
                            "q0rig.pose! weight must be finite",
                        ));
                    }
                } else {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        "q0rig.pose! expects a pose name followed by a weight",
                    ));
                }
            }
            ("pose_reset", [name]) => {
                if let Some(pose) = control_name(name) {
                    report.actions.push(RuntimeAction::RigResetPose { pose });
                } else {
                    report.diagnostics.push(RuntimeDiagnostic::at_line(
                        0,
                        "q0rig.pose_reset! expects a pose name",
                    ));
                }
            }
            ("position", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "q0rig.position! expects: q0rig.position! control, x, y",
            )),
            ("value", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "q0rig.value! expects: q0rig.value! control, value",
            )),
            ("reset", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "q0rig.reset! expects: q0rig.reset! control",
            )),
            ("pose", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "q0rig.pose! expects: q0rig.pose! pose, weight",
            )),
            ("pose_reset", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                "q0rig.pose_reset! expects: q0rig.pose_reset! pose",
            )),
            _ => report.diagnostics.push(RuntimeDiagnostic::at_line(
                0,
                format!("unknown q0.rig command `q0rig.{command}!`"),
            )),
        }
    }

    fn resolve_runtime_value(&self, value: &Value, report: &mut ExecutionReport) -> RuntimeValue {
        match value {
            Value::Ident(name) => {
                self.vars
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| match name.as_str() {
                        "pi" => RuntimeValue::Number(std::f64::consts::PI),
                        "tau" => RuntimeValue::Number(std::f64::consts::TAU),
                        _ => RuntimeValue::Ident(name.clone()),
                    })
            }
            Value::Number(raw) => raw
                .parse::<f64>()
                .map(RuntimeValue::Number)
                .unwrap_or_else(|_| RuntimeValue::Raw(raw.clone())),
            Value::String(text) => RuntimeValue::String(text.clone()),
            Value::Unary { op, value } => {
                let value = self.resolve_runtime_value(value, report);
                match (op, value) {
                    (UnaryOp::Neg, RuntimeValue::Number(n)) => RuntimeValue::Number(-n),
                    (UnaryOp::Neg, other) => {
                        report.diagnostics.push(RuntimeDiagnostic::at_line(
                            0,
                            format!("cannot negate `{}`", other.kind_name()),
                        ));
                        RuntimeValue::Raw("nan".to_string())
                    }
                }
            }
            Value::Binary { op, left, right } => {
                let left = self.resolve_runtime_value(left, report);
                let right = self.resolve_runtime_value(right, report);
                self.eval_binary(*op, left, right, report)
            }
            Value::Call { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.resolve_runtime_value(arg, report))
                    .collect::<Vec<_>>();
                self.eval_call(name, &args, report)
            }
            Value::Raw(raw) => RuntimeValue::Raw(raw.clone()),
        }
    }

    fn resolve_timeline_target(
        &self,
        value: &Value,
        report: &mut ExecutionReport,
    ) -> TimelineTarget {
        match self.resolve_runtime_value(value, report) {
            RuntimeValue::Number(n) if n >= 0.0 => TimelineTarget::Frame(n as u16),
            RuntimeValue::String(label) | RuntimeValue::Ident(label) | RuntimeValue::Raw(label) => {
                TimelineTarget::Label(label)
            }
            RuntimeValue::Number(n) => TimelineTarget::Label(n.to_string()),
        }
    }

    fn eval_binary(
        &self,
        op: BinaryOp,
        left: RuntimeValue,
        right: RuntimeValue,
        report: &mut ExecutionReport,
    ) -> RuntimeValue {
        match (left, right) {
            (RuntimeValue::Number(a), RuntimeValue::Number(b)) => {
                let value = match op {
                    BinaryOp::Add => a + b,
                    BinaryOp::Sub => a - b,
                    BinaryOp::Mul => a * b,
                    BinaryOp::Div => a / b,
                    BinaryOp::Rem => a % b,
                    BinaryOp::Pow => a.powf(b),
                };
                RuntimeValue::Number(value)
            }
            (RuntimeValue::String(a), RuntimeValue::String(b)) if matches!(op, BinaryOp::Add) => {
                RuntimeValue::String(format!("{a}{b}"))
            }
            (RuntimeValue::String(a), b) if matches!(op, BinaryOp::Add) => {
                RuntimeValue::String(format!("{a}{}", b.display_lossy()))
            }
            (a, RuntimeValue::String(b)) if matches!(op, BinaryOp::Add) => {
                RuntimeValue::String(format!("{}{b}", a.display_lossy()))
            }
            (a, b) => {
                report.diagnostics.push(RuntimeDiagnostic::at_line(
                    0,
                    format!(
                        "cannot apply {:?} to `{}` and `{}`",
                        op,
                        a.kind_name(),
                        b.kind_name()
                    ),
                ));
                RuntimeValue::Raw("nan".to_string())
            }
        }
    }

    fn eval_call(
        &self,
        name: &str,
        args: &[RuntimeValue],
        report: &mut ExecutionReport,
    ) -> RuntimeValue {
        let nums = args
            .iter()
            .map(|arg| match arg {
                RuntimeValue::Number(n) => Some(*n),
                _ => None,
            })
            .collect::<Vec<_>>();
        let wrong_args = || {
            RuntimeDiagnostic::at_line(
                0,
                format!("bad args for math call `{name}`; q0.math wants numbers here"),
            )
        };
        let value = match (name, nums.as_slice()) {
            ("sin", [Some(a)]) => a.sin(),
            ("cos", [Some(a)]) => a.cos(),
            ("tan", [Some(a)]) => a.tan(),
            ("sqrt", [Some(a)]) => a.sqrt(),
            ("abs", [Some(a)]) => a.abs(),
            ("floor", [Some(a)]) => a.floor(),
            ("ceil", [Some(a)]) => a.ceil(),
            ("round", [Some(a)]) => a.round(),
            ("min", [Some(a), Some(b)]) => a.min(*b),
            ("max", [Some(a), Some(b)]) => a.max(*b),
            ("clamp", [Some(x), Some(lo), Some(hi)]) => x.clamp(*lo, *hi),
            ("lerp", [Some(a), Some(b), Some(t)]) => a + (b - a) * t,
            _ => {
                report.diagnostics.push(wrong_args());
                return RuntimeValue::Raw(format!("{name}(?)"));
            }
        };
        RuntimeValue::Number(value)
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    pub name: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    Number(f64),
    String(String),
    Ident(String),
    Raw(String),
}

impl RuntimeValue {
    fn kind_name(&self) -> &'static str {
        match self {
            RuntimeValue::Number(_) => "number",
            RuntimeValue::String(_) => "string",
            RuntimeValue::Ident(_) => "ident",
            RuntimeValue::Raw(_) => "raw",
        }
    }

    pub fn display_lossy(&self) -> String {
        match self {
            RuntimeValue::Number(n) => n.to_string(),
            RuntimeValue::String(s) | RuntimeValue::Ident(s) | RuntimeValue::Raw(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeAction {
    GoRun(TimelineTarget),
    GoStop(TimelineTarget),
    ShellCommand {
        command: String,
        args: Vec<RuntimeValue>,
    },
    RigSetPosition {
        control: String,
        x: f64,
        y: f64,
    },
    RigSetValue {
        control: String,
        value: f64,
    },
    RigReset {
        control: String,
    },
    RigSetPose {
        pose: String,
        weight: f64,
    },
    RigResetPose {
        pose: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineTarget {
    Frame(u16),
    Label(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionReport {
    pub actions: Vec<RuntimeAction>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

pub type RuntimeDiagnostic = Diagnostic;

trait RuntimeDiagnosticExt {
    fn at_line(line: usize, message: impl Into<String>) -> RuntimeDiagnostic;
}

impl RuntimeDiagnosticExt for RuntimeDiagnostic {
    fn at_line(line: usize, message: impl Into<String>) -> RuntimeDiagnostic {
        RuntimeDiagnostic {
            line,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_assignments_and_timeline_actions() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"start = 2
label = "intro"
gorun! start
gostop! label
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(runtime.vars["start"], RuntimeValue::Number(2.0));
        assert_eq!(
            report.actions,
            vec![
                RuntimeAction::GoRun(TimelineTarget::Frame(2)),
                RuntimeAction::GoStop(TimelineTarget::Label("intro".to_string())),
            ]
        );
    }

    #[test]
    fn tracks_listeners_without_duplicates_and_removes_them() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"listen mouse.click onClick
listen mouse.click onClick
listen mouse.move onMove
xlisten mouse.click onClick
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        assert!(!runtime.listeners.contains_key("mouse.click"));
        assert_eq!(runtime.listeners["mouse.move"], vec!["onMove".to_string()]);
    }

    #[test]
    fn executes_do_blocks() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"do! {
  frame = 3 + 4
  gorun! frame
}
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(runtime.vars["frame"], RuntimeValue::Number(7.0));
        assert_eq!(
            report.actions,
            vec![RuntimeAction::GoRun(TimelineTarget::Frame(7))]
        );
    }

    #[test]
    fn evaluates_math_core_without_timeline_host() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"import q0.math
x = sin(pi / 2)
y = clamp(lerp(0, 10, 0.75), 2, 6)
name = "Q0" + " tools"
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        match runtime.vars["x"] {
            RuntimeValue::Number(n) => assert!((n - 1.0).abs() < 0.000001),
            _ => panic!("x should be a number"),
        }
        assert_eq!(runtime.vars["y"], RuntimeValue::Number(6.0));
        assert_eq!(
            runtime.vars["name"],
            RuntimeValue::String("Q0 tools".to_string())
        );
    }

    #[test]
    fn emits_q0shell_actions_without_touching_filesystem() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"q0shell.write! "out.txt", "frame=" + round(1.5)
q0shell.delete! "old.txt"
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.actions,
            vec![
                RuntimeAction::ShellCommand {
                    command: "write".to_string(),
                    args: vec![
                        RuntimeValue::String("out.txt".to_string()),
                        RuntimeValue::String("frame=2".to_string()),
                    ],
                },
                RuntimeAction::ShellCommand {
                    command: "delete".to_string(),
                    args: vec![RuntimeValue::String("old.txt".to_string())],
                },
            ]
        );
    }

    #[test]
    fn registers_function_signatures_and_imports() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"import q0.mouse
pb func openDoor actor speed
pr func cacheFrame frame
"#,
        );

        assert_eq!(report.diagnostics, Vec::new());
        assert!(runtime.imported_libraries.contains("q0.mouse"));
        assert_eq!(
            runtime.public_functions["openDoor"].params,
            vec!["actor".to_string(), "speed".to_string()]
        );
        assert_eq!(
            runtime.private_functions["cacheFrame"].params,
            vec!["frame".to_string()]
        );
    }

    #[test]
    fn emits_semantic_rig_runtime_actions() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"import q0.rig
base = 10
q0rig.position! "look", base + 2, 7
q0rig.value! "arm blend", 0.75
q0rig.reset! "look"
q0rig.pose! "wave", 0.6
q0rig.pose_reset! "wave"
"#,
        );
        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.actions,
            vec![
                RuntimeAction::RigSetPosition {
                    control: "look".into(),
                    x: 12.0,
                    y: 7.0,
                },
                RuntimeAction::RigSetValue {
                    control: "arm blend".into(),
                    value: 0.75,
                },
                RuntimeAction::RigReset {
                    control: "look".into(),
                },
                RuntimeAction::RigSetPose {
                    pose: "wave".into(),
                    weight: 0.6,
                },
                RuntimeAction::RigResetPose {
                    pose: "wave".into(),
                },
            ]
        );
    }

    #[test]
    fn reports_bad_rig_runtime_command_arguments_without_emitting_action() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source("q0rig.position! 12, 'bad'\n");
        assert!(report.actions.is_empty());
        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0]
            .message
            .contains("q0rig.position! expects"));
    }
}
