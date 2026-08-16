//! q0lang compatibility facade backed by qvm.
//!
//! Source is parsed into the shared AST, compiled to qvm bytecode, and only
//! then executed. Hosts receive semantic actions and decide which capabilities
//! are allowed. There is no direct AST interpreter in this module.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use q0vm::{FunctionBytecode, HostAction, Vm, VmState};

use super::compiler;
use super::parser::{Diagnostic, Program};

pub use q0vm::{FunctionSignature, Value as RuntimeValue};

const MAX_COMPILED_SOURCE_CACHE_ENTRIES: usize = 4096;
const MAX_CACHED_SOURCE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Runtime {
    pub vars: BTreeMap<String, RuntimeValue>,
    pub listeners: BTreeMap<String, Vec<String>>,
    pub imported_libraries: BTreeSet<String>,
    pub public_functions: BTreeMap<String, FunctionSignature>,
    pub private_functions: BTreeMap<String, FunctionSignature>,
    compiled_functions: BTreeMap<String, FunctionBytecode>,
    compiled_source_cache: BTreeMap<String, Arc<compiler::CompiledProgram>>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            vars: BTreeMap::new(),
            listeners: BTreeMap::new(),
            imported_libraries: BTreeSet::new(),
            public_functions: BTreeMap::new(),
            private_functions: BTreeMap::new(),
            compiled_functions: BTreeMap::new(),
            compiled_source_cache: BTreeMap::new(),
        }
    }

    pub fn execute_source(&mut self, source: &str) -> ExecutionReport {
        self.execute_source_with_budget(source, q0vm::DEFAULT_INSTRUCTION_BUDGET)
    }

    pub fn execute_source_with_budget(
        &mut self,
        source: &str,
        instruction_budget: usize,
    ) -> ExecutionReport {
        let compiled = if let Some(compiled) = self.compiled_source_cache.get(source) {
            Arc::clone(compiled)
        } else {
            let compiled = Arc::new(compiler::compile_source(source));
            if source.len() <= MAX_CACHED_SOURCE_BYTES
                && self.compiled_source_cache.len() < MAX_COMPILED_SOURCE_CACHE_ENTRIES
            {
                self.compiled_source_cache
                    .insert(source.to_string(), Arc::clone(&compiled));
            }
            compiled
        };
        self.execute_compiled(&compiled, instruction_budget)
    }

    pub fn execute_program(&mut self, program: &Program) -> ExecutionReport {
        self.execute_program_with_budget(program, q0vm::DEFAULT_INSTRUCTION_BUDGET)
    }

    pub fn execute_program_with_budget(
        &mut self,
        program: &Program,
        instruction_budget: usize,
    ) -> ExecutionReport {
        let compiled = compiler::compile(program);
        self.execute_compiled(&compiled, instruction_budget)
    }

    fn execute_compiled(
        &mut self,
        compiled: &compiler::CompiledProgram,
        instruction_budget: usize,
    ) -> ExecutionReport {
        let mut report = ExecutionReport {
            actions: Vec::new(),
            diagnostics: compiled.diagnostics.clone(),
        };
        if compiled.fatal {
            return report;
        }

        let state = VmState {
            vars: std::mem::take(&mut self.vars),
            listeners: std::mem::take(&mut self.listeners),
            imported_libraries: std::mem::take(&mut self.imported_libraries),
            public_functions: std::mem::take(&mut self.public_functions),
            private_functions: std::mem::take(&mut self.private_functions),
            functions: std::mem::take(&mut self.compiled_functions),
        };
        let mut vm = Vm::with_state(state);
        let vm_report = vm.execute(&compiled.chunk, instruction_budget);
        self.install_vm_state(vm.state);
        report.diagnostics.extend(
            vm_report
                .diagnostics
                .into_iter()
                .map(|diagnostic| Diagnostic {
                    line: diagnostic.line,
                    message: diagnostic.message,
                }),
        );

        for action in vm_report.actions {
            match action {
                HostAction::GoRun(target) => report
                    .actions
                    .push(RuntimeAction::GoRun(resolve_timeline_target(target))),
                HostAction::GoStop(target) => report
                    .actions
                    .push(RuntimeAction::GoStop(resolve_timeline_target(target))),
                HostAction::ShellCommand { command, args } => {
                    report
                        .actions
                        .push(RuntimeAction::ShellCommand { command, args });
                }
                HostAction::RigCommand { command, args } => {
                    emit_rig_command(&command, &args, &mut report);
                }
                HostAction::SceneCommand { command, args } => {
                    emit_scene_command(&command, &args, &mut report);
                }
            }
        }
        report
    }

    fn install_vm_state(&mut self, state: VmState) {
        self.vars = state.vars;
        self.listeners = state.listeners;
        self.imported_libraries = state.imported_libraries;
        self.public_functions = state.public_functions;
        self.private_functions = state.private_functions;
        self.compiled_functions = state.functions;
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_timeline_target(value: RuntimeValue) -> TimelineTarget {
    match value {
        RuntimeValue::Number(number) if number >= 0.0 => TimelineTarget::Frame(number as u16),
        RuntimeValue::String(label) | RuntimeValue::Ident(label) | RuntimeValue::Raw(label) => {
            TimelineTarget::Label(label)
        }
        RuntimeValue::Number(number) => TimelineTarget::Label(number.to_string()),
        RuntimeValue::Bool(value) => TimelineTarget::Label(value.to_string()),
        RuntimeValue::Null => TimelineTarget::Label("null".to_string()),
    }
}

fn runtime_name(value: &RuntimeValue) -> Option<String> {
    match value {
        RuntimeValue::String(name) | RuntimeValue::Ident(name) | RuntimeValue::Raw(name) => {
            Some(name.clone())
        }
        RuntimeValue::Number(_) | RuntimeValue::Bool(_) | RuntimeValue::Null => None,
    }
}

fn emit_scene_command(command: &str, args: &[RuntimeValue], report: &mut ExecutionReport) {
    match (command, args) {
        ("switch", [alias]) => {
            if let Some(alias) = runtime_name(alias) {
                report.actions.push(RuntimeAction::SceneSwitch { alias });
            } else {
                report.diagnostics.push(RuntimeDiagnostic::at_line(
                    0,
                    "q0scene.switch! expects a bundled scene alias",
                ));
            }
        }
        ("switch", _) => report.diagnostics.push(RuntimeDiagnostic::at_line(
            0,
            "q0scene.switch! expects: q0scene.switch! scene_alias",
        )),
        _ => report.diagnostics.push(RuntimeDiagnostic::at_line(
            0,
            format!("unknown q0.scene command `q0scene.{command}!`"),
        )),
    }
}

fn emit_rig_command(command: &str, args: &[RuntimeValue], report: &mut ExecutionReport) {
    let control_name = runtime_name;
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

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeAction {
    GoRun(TimelineTarget),
    GoStop(TimelineTarget),
    ShellCommand {
        command: String,
        args: Vec<RuntimeValue>,
    },
    SceneSwitch {
        alias: String,
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
    fn emits_semantic_scene_switch_action() {
        let mut runtime = Runtime::new();
        let report =
            runtime.execute_source("import q0.scene\nq0scene.switch! \"startinggamepart2\"\n");
        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.actions,
            vec![RuntimeAction::SceneSwitch {
                alias: "startinggamepart2".into(),
            }]
        );
    }

    #[test]
    fn scene_switch_inside_function_emits_when_the_function_is_called() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            "pb func leave_room x {\n  q0scene.switch! \"room_2\"\n  return x\n}\nvalue = leave_room(7)\n",
        );
        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.actions,
            vec![RuntimeAction::SceneSwitch {
                alias: "room_2".into(),
            }]
        );
        assert_eq!(runtime.vars["value"], RuntimeValue::Number(7.0));
    }

    #[test]
    fn rejects_non_name_scene_switch_argument() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source("q0scene.switch! 12\n");
        assert!(report.actions.is_empty());
        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0]
            .message
            .contains("q0scene.switch! expects"));
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

    #[test]
    fn runtime_errors_report_qvm_source_line() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source("ok = 1\nbad = -\"text\"\n");
        assert_eq!(runtime.vars["ok"], RuntimeValue::Number(1.0));
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 2);
        assert!(report.diagnostics[0].message.contains("cannot negate"));
    }

    #[test]
    fn source_bytecode_is_cached_across_repeated_frame_style_execution() {
        let mut runtime = Runtime::new();
        let source = "counter = counter + 1\n";
        runtime
            .vars
            .insert("counter".into(), RuntimeValue::Number(0.0));
        assert!(runtime.execute_source(source).diagnostics.is_empty());
        assert_eq!(runtime.compiled_source_cache.len(), 1);
        let first = Arc::clone(
            runtime
                .compiled_source_cache
                .get(source)
                .expect("cached chunk"),
        );
        assert!(runtime.execute_source(source).diagnostics.is_empty());
        let second = Arc::clone(
            runtime
                .compiled_source_cache
                .get(source)
                .expect("same cached chunk"),
        );
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(runtime.vars["counter"], RuntimeValue::Number(2.0));
    }

    #[test]
    fn executes_if_while_bool_and_real_function_return() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(
            r#"pb func climb x limit {
  while x < limit {
    x = x + 1
  }
  if x == limit and not false {
    return x
  }
  return 0
}
result = climb(1, 3)
"#,
        );
        assert_eq!(report.diagnostics, Vec::new(), "{:?}", report.diagnostics);
        assert_eq!(runtime.vars["result"], RuntimeValue::Number(3.0));
        assert!(!runtime.vars.contains_key("x"));
        assert!(!runtime.vars.contains_key("limit"));
    }

    #[test]
    fn executable_function_survives_into_later_frame_style_source() {
        let mut runtime = Runtime::new();
        let init = runtime.execute_source(
            r#"pb func plus_one x {
  return x + 1
}
"#,
        );
        assert_eq!(init.diagnostics, Vec::new(), "{:?}", init.diagnostics);
        assert!(runtime.compiled_functions.contains_key("plus_one"));

        let frame = runtime.execute_source("result = plus_one(9)\n");
        assert_eq!(frame.diagnostics, Vec::new(), "{:?}", frame.diagnostics);
        assert_eq!(runtime.vars["result"], RuntimeValue::Number(10.0));
    }

    #[test]
    fn boolean_short_circuit_skips_unreached_function_call() {
        let mut runtime = Runtime::new();
        let report = runtime
            .execute_source("a = false and missing_function()\nb = true or missing_function()\n");
        assert_eq!(report.diagnostics, Vec::new(), "{:?}", report.diagnostics);
        assert_eq!(runtime.vars["a"], RuntimeValue::Bool(false));
        assert_eq!(runtime.vars["b"], RuntimeValue::Bool(true));
    }

    #[test]
    fn recursive_function_is_bounded_by_qvm_instruction_budget() {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source_with_budget(
            r#"pr func forever {
  return forever()
}
result = forever()
"#,
            48,
        );
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("instruction budget exceeded")));
    }
}
