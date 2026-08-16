use std::collections::BTreeSet;

use q0vm::{BytecodeError, Chunk, Opcode, Value as VmValue};

use super::parser::{
    self, BinaryOp, Block, Diagnostic, Program, Statement, TimelineSignal, UnaryOp, Value,
    Visibility,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledProgram {
    pub chunk: Chunk,
    pub diagnostics: Vec<Diagnostic>,
    pub fatal: bool,
}

pub fn compile_source(source: &str) -> CompiledProgram {
    let program = parser::parse(source);
    compile(&program)
}

pub fn compile(program: &Program) -> CompiledProgram {
    let mut compiler = Compiler {
        chunk: Chunk::new(),
        diagnostics: program.diagnostics.clone(),
        fatal: false,
    };
    compiler.compile_statements(
        &program.statements,
        &program.statement_lines,
        &CompileContext::top_level(),
    );
    let halt_line = program.statement_lines.last().copied().unwrap_or(0);
    compiler.chunk.emit_op(Opcode::Halt, halt_line);
    CompiledProgram {
        chunk: compiler.chunk,
        diagnostics: compiler.diagnostics,
        fatal: compiler.fatal,
    }
}

#[derive(Debug, Clone, Default)]
struct CompileContext {
    locals: Option<BTreeSet<String>>,
    in_function: bool,
}

impl CompileContext {
    fn top_level() -> Self {
        Self::default()
    }

    fn function(params: &[String], body: Option<&Block>) -> Self {
        let mut locals = params.iter().cloned().collect::<BTreeSet<_>>();
        if let Some(body) = body {
            collect_assigned_names(body, &mut locals);
        }
        Self {
            locals: Some(locals),
            in_function: true,
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals
            .as_ref()
            .is_some_and(|locals| locals.contains(name))
    }
}

struct Compiler {
    chunk: Chunk,
    diagnostics: Vec<Diagnostic>,
    fatal: bool,
}

impl Compiler {
    fn compile_statements(
        &mut self,
        statements: &[Statement],
        lines: &[usize],
        context: &CompileContext,
    ) {
        for (index, statement) in statements.iter().enumerate() {
            if self.fatal {
                break;
            }
            let line = lines.get(index).copied().unwrap_or(0);
            self.compile_statement(statement, line, context);
        }
    }

    fn compile_block(&mut self, block: &Block, context: &CompileContext) {
        self.compile_statements(&block.statements, &block.statement_lines, context);
    }

    fn compile_statement(&mut self, statement: &Statement, line: usize, context: &CompileContext) {
        match statement {
            Statement::Assignment { name, value } => {
                self.compile_value(value, line, context);
                let name_constant = self.name_constant(name, line);
                self.chunk.emit_op(
                    if context.is_local(name) {
                        Opcode::StoreLocal
                    } else {
                        Opcode::StoreGlobal
                    },
                    line,
                );
                self.chunk.emit_u32(name_constant);
            }
            Statement::Expression { value } => {
                self.compile_value(value, line, context);
                self.chunk.emit_op(Opcode::Pop, line);
            }
            Statement::TimelineSignal { kind, target } => {
                self.compile_value(target, line, context);
                self.chunk.emit_op(
                    match kind {
                        TimelineSignal::GoRun => Opcode::GoRun,
                        TimelineSignal::GoStop => Opcode::GoStop,
                    },
                    line,
                );
            }
            Statement::ShellCommand { command, args } => {
                if let Some(argc) = self.compile_args(args, line, "q0shell command", context) {
                    let command = self.name_constant(command, line);
                    self.chunk.emit_op(Opcode::Shell, line);
                    self.chunk.emit_u32(command);
                    self.chunk.emit_u8(argc);
                }
            }
            Statement::RigCommand { command, args } => {
                if let Some(argc) = self.compile_args(args, line, "q0.rig command", context) {
                    let command = self.name_constant(command, line);
                    self.chunk.emit_op(Opcode::Rig, line);
                    self.chunk.emit_u32(command);
                    self.chunk.emit_u8(argc);
                }
            }
            Statement::SceneCommand { command, args } => {
                if let Some(argc) = self.compile_args(args, line, "q0.scene command", context) {
                    let command = self.name_constant(command, line);
                    self.chunk.emit_op(Opcode::Scene, line);
                    self.chunk.emit_u32(command);
                    self.chunk.emit_u8(argc);
                }
            }
            Statement::Listen { event, handler } | Statement::Unlisten { event, handler } => {
                let event = self.name_constant(event, line);
                let handler = self.name_constant(handler, line);
                self.chunk.emit_op(
                    if matches!(statement, Statement::Listen { .. }) {
                        Opcode::Listen
                    } else {
                        Opcode::Unlisten
                    },
                    line,
                );
                self.chunk.emit_u32(event);
                self.chunk.emit_u32(handler);
            }
            Statement::Function {
                visibility,
                name,
                params,
                body,
            } => self.compile_function(*visibility, name, params, body.as_ref(), line),
            Statement::DoBlock { body } => self.compile_block(body, context),
            Statement::If {
                condition,
                then_body,
                else_body,
            } => self.compile_if(condition, then_body, else_body.as_ref(), line, context),
            Statement::While { condition, body } => {
                self.compile_while(condition, body, line, context)
            }
            Statement::Return { value } => {
                if !context.in_function {
                    self.diagnostics.push(Diagnostic {
                        line,
                        message: "return is only valid inside a function body".to_string(),
                    });
                    return;
                }
                match value {
                    Some(value) => self.compile_value(value, line, context),
                    None => self.emit_constant(VmValue::Null, line),
                }
                self.chunk.emit_op(Opcode::Return, line);
            }
            Statement::Import { library } => {
                let library = self.name_constant(library, line);
                self.chunk.emit_op(Opcode::Import, line);
                self.chunk.emit_u32(library);
            }
            Statement::Unknown { text } => {
                self.diagnostics.push(Diagnostic {
                    line,
                    message: format!("cannot compile unknown statement `{text}`"),
                });
            }
        }
    }

    fn compile_function(
        &mut self,
        visibility: Visibility,
        name: &str,
        params: &[String],
        body: Option<&Block>,
        line: usize,
    ) {
        let Ok(argc) = u8::try_from(params.len()) else {
            self.diagnostics.push(Diagnostic {
                line,
                message: "q0lang function has more than 255 parameters".to_string(),
            });
            self.fatal = true;
            return;
        };
        let name_constant = self.name_constant(name, line);
        let param_constants = params
            .iter()
            .map(|param| self.name_constant(param, line))
            .collect::<Vec<_>>();
        self.chunk.emit_op(
            match visibility {
                Visibility::Public => Opcode::DefinePublic,
                Visibility::Private => Opcode::DefinePrivate,
            },
            line,
        );
        self.chunk.emit_u32(name_constant);
        self.chunk.emit_u8(argc);
        for param in param_constants {
            self.chunk.emit_u32(param);
        }
        let end_patch = self.chunk.current_offset();
        self.chunk.emit_u32(0);

        if let Some(body) = body {
            let context = CompileContext::function(params, Some(body));
            self.compile_block(body, &context);
        }
        self.emit_constant(VmValue::Null, line);
        self.chunk.emit_op(Opcode::Return, line);
        let body_end = self.chunk.current_offset();
        self.patch_offset(end_patch, body_end, line);
    }

    fn compile_if(
        &mut self,
        condition: &Value,
        then_body: &Block,
        else_body: Option<&Block>,
        line: usize,
        context: &CompileContext,
    ) {
        self.compile_value(condition, line, context);
        let false_patch = self.emit_jump_placeholder(Opcode::JumpIfFalse, line);
        self.compile_block(then_body, context);
        if let Some(else_body) = else_body {
            let end_patch = self.emit_jump_placeholder(Opcode::Jump, line);
            let else_start = self.chunk.current_offset();
            self.patch_offset(false_patch, else_start, line);
            self.compile_block(else_body, context);
            let end = self.chunk.current_offset();
            self.patch_offset(end_patch, end, line);
        } else {
            let end = self.chunk.current_offset();
            self.patch_offset(false_patch, end, line);
        }
    }

    fn compile_while(
        &mut self,
        condition: &Value,
        body: &Block,
        line: usize,
        context: &CompileContext,
    ) {
        let loop_start = self.chunk.current_offset();
        self.compile_value(condition, line, context);
        let exit_patch = self.emit_jump_placeholder(Opcode::JumpIfFalse, line);
        self.compile_block(body, context);
        self.chunk.emit_op(Opcode::Jump, line);
        let loop_start = self.offset_u32(loop_start, line);
        self.chunk.emit_u32(loop_start);
        let exit = self.chunk.current_offset();
        self.patch_offset(exit_patch, exit, line);
    }

    fn compile_args(
        &mut self,
        args: &[Value],
        line: usize,
        owner: &str,
        context: &CompileContext,
    ) -> Option<u8> {
        let Ok(argc) = u8::try_from(args.len()) else {
            self.diagnostics.push(Diagnostic {
                line,
                message: format!("{owner} has more than 255 arguments"),
            });
            self.fatal = true;
            return None;
        };
        for arg in args {
            self.compile_value(arg, line, context);
        }
        Some(argc)
    }

    fn compile_value(&mut self, value: &Value, line: usize, context: &CompileContext) {
        match value {
            Value::Ident(name) => {
                let constant = self.name_constant(name, line);
                self.chunk.emit_op(
                    if context.is_local(name) {
                        Opcode::LoadLocal
                    } else {
                        Opcode::LoadGlobal
                    },
                    line,
                );
                self.chunk.emit_u32(constant);
            }
            Value::Number(raw) => match raw.parse::<f64>() {
                Ok(value) => self.emit_constant(VmValue::Number(value), line),
                Err(_) => {
                    let constant = self.name_constant(raw, line);
                    self.chunk.emit_op(Opcode::PushRaw, line);
                    self.chunk.emit_u32(constant);
                }
            },
            Value::String(text) => self.emit_constant(VmValue::String(text.clone()), line),
            Value::Bool(value) => self.emit_constant(VmValue::Bool(*value), line),
            Value::Null => self.emit_constant(VmValue::Null, line),
            Value::Unary { op, value } => {
                self.compile_value(value, line, context);
                self.chunk.emit_op(
                    match op {
                        UnaryOp::Neg => Opcode::Neg,
                        UnaryOp::Not => Opcode::Not,
                    },
                    line,
                );
            }
            Value::Binary { op, left, right } => match op {
                BinaryOp::And => self.compile_and(left, right, line, context),
                BinaryOp::Or => self.compile_or(left, right, line, context),
                _ => {
                    self.compile_value(left, line, context);
                    self.compile_value(right, line, context);
                    self.chunk.emit_op(
                        match op {
                            BinaryOp::Add => Opcode::Add,
                            BinaryOp::Sub => Opcode::Sub,
                            BinaryOp::Mul => Opcode::Mul,
                            BinaryOp::Div => Opcode::Div,
                            BinaryOp::Rem => Opcode::Rem,
                            BinaryOp::Pow => Opcode::Pow,
                            BinaryOp::Equal => Opcode::Equal,
                            BinaryOp::NotEqual => Opcode::NotEqual,
                            BinaryOp::Less => Opcode::Less,
                            BinaryOp::LessEqual => Opcode::LessEqual,
                            BinaryOp::Greater => Opcode::Greater,
                            BinaryOp::GreaterEqual => Opcode::GreaterEqual,
                            BinaryOp::And | BinaryOp::Or => unreachable!(),
                        },
                        line,
                    );
                }
            },
            Value::Call { name, args } => {
                let Some(argc) = self.compile_args(args, line, "q0lang call", context) else {
                    return;
                };
                let name_constant = self.name_constant(name, line);
                self.chunk.emit_op(
                    if is_math_call(name) {
                        Opcode::CallMath
                    } else {
                        Opcode::CallUser
                    },
                    line,
                );
                self.chunk.emit_u32(name_constant);
                self.chunk.emit_u8(argc);
            }
            Value::Raw(raw) => {
                let constant = self.name_constant(raw, line);
                self.chunk.emit_op(Opcode::PushRaw, line);
                self.chunk.emit_u32(constant);
            }
        }
    }

    fn compile_and(&mut self, left: &Value, right: &Value, line: usize, context: &CompileContext) {
        self.compile_value(left, line, context);
        let false_patch = self.emit_jump_placeholder(Opcode::JumpIfFalse, line);
        self.compile_value(right, line, context);
        self.chunk.emit_op(Opcode::ToBool, line);
        let end_patch = self.emit_jump_placeholder(Opcode::Jump, line);
        let false_start = self.chunk.current_offset();
        self.patch_offset(false_patch, false_start, line);
        self.emit_constant(VmValue::Bool(false), line);
        let end = self.chunk.current_offset();
        self.patch_offset(end_patch, end, line);
    }

    fn compile_or(&mut self, left: &Value, right: &Value, line: usize, context: &CompileContext) {
        self.compile_value(left, line, context);
        let right_patch = self.emit_jump_placeholder(Opcode::JumpIfFalse, line);
        self.emit_constant(VmValue::Bool(true), line);
        let end_patch = self.emit_jump_placeholder(Opcode::Jump, line);
        let right_start = self.chunk.current_offset();
        self.patch_offset(right_patch, right_start, line);
        self.compile_value(right, line, context);
        self.chunk.emit_op(Opcode::ToBool, line);
        let end = self.chunk.current_offset();
        self.patch_offset(end_patch, end, line);
    }

    fn emit_constant(&mut self, value: VmValue, line: usize) {
        let constant = self.constant(value, line);
        self.chunk.emit_op(Opcode::Constant, line);
        self.chunk.emit_u32(constant);
    }

    fn emit_jump_placeholder(&mut self, opcode: Opcode, line: usize) -> usize {
        self.chunk.emit_op(opcode, line);
        let operand = self.chunk.current_offset();
        self.chunk.emit_u32(0);
        operand
    }

    fn patch_offset(&mut self, operand_offset: usize, target: usize, line: usize) {
        let target = self.offset_u32(target, line);
        if let Err(error) = self.chunk.patch_u32(operand_offset, target) {
            self.bytecode_error(line, error);
        }
    }

    fn offset_u32(&mut self, offset: usize, line: usize) -> u32 {
        match u32::try_from(offset) {
            Ok(offset) => offset,
            Err(_) => {
                self.diagnostics.push(Diagnostic {
                    line,
                    message: "qvm bytecode offset exceeds u32".to_string(),
                });
                self.fatal = true;
                0
            }
        }
    }

    fn name_constant(&mut self, value: &str, line: usize) -> u32 {
        self.constant(VmValue::String(value.to_string()), line)
    }

    fn constant(&mut self, value: VmValue, line: usize) -> u32 {
        match self.chunk.add_constant(value) {
            Ok(index) => index,
            Err(error) => {
                self.bytecode_error(line, error);
                0
            }
        }
    }

    fn bytecode_error(&mut self, line: usize, error: BytecodeError) {
        self.diagnostics.push(Diagnostic {
            line,
            message: format!("qvm compile error: {error}"),
        });
        self.fatal = true;
    }
}

fn is_math_call(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "sqrt"
            | "abs"
            | "floor"
            | "ceil"
            | "round"
            | "min"
            | "max"
            | "clamp"
            | "lerp"
    )
}

fn collect_assigned_names(block: &Block, out: &mut BTreeSet<String>) {
    for statement in &block.statements {
        match statement {
            Statement::Assignment { name, .. } => {
                // Dotted identifiers are host/member properties (player.x, time.dt, ...),
                // not lexical locals. Functions may write them without shadowing the host binding.
                if !name.contains('.') {
                    out.insert(name.clone());
                }
            }
            Statement::DoBlock { body } | Statement::While { body, .. } => {
                collect_assigned_names(body, out);
            }
            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, out);
                if let Some(else_body) = else_body {
                    collect_assigned_names(else_body, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_emits_real_qvm_bytecode_for_current_language() {
        let compiled = compile_source(
            r#"import q0.math
x = 2 + 3 * 4
gorun! x
q0shell.print! "x=", x
"#,
        );
        assert!(!compiled.fatal);
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        assert!(!compiled.chunk.code.is_empty());
        assert!(compiled.chunk.code.contains(&(Opcode::Mul as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::Add as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::StoreGlobal as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::GoRun as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::Shell as u8)));
        assert_eq!(compiled.chunk.code.last(), Some(&(Opcode::Halt as u8)));
    }

    #[test]
    fn scene_switch_compiles_to_scene_host_opcode() {
        let compiled = compile_source("import q0.scene\nq0scene.switch! \"room_2\"\n");
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        assert!(compiled.chunk.code.contains(&(Opcode::Scene as u8)));
    }

    #[test]
    fn do_block_is_real_ast_and_flattens_to_bytecode() {
        let compiled = compile_source(
            r#"do! {
  frame = 7
  gorun! frame
}
"#,
        );
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        assert!(compiled.chunk.code.contains(&(Opcode::StoreGlobal as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::GoRun as u8)));
        assert_eq!(compiled.chunk.line_at(0), 2);
    }

    #[test]
    fn control_flow_compiles_to_real_jumps_and_boolean_ops() {
        let compiled = compile_source(
            r#"x = 0
while x < 3 {
  if x == 1 or false {
    x = x + 1
  } else {
    x = x + 1
  }
}
"#,
        );
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        assert!(compiled.chunk.code.contains(&(Opcode::Jump as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::JumpIfFalse as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::Less as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::Equal as u8)));
    }

    #[test]
    fn dotted_member_assignment_inside_function_stays_global() {
        let compiled = compile_source(
            r#"pb func move dx {
  local = dx + 1
  player.x = player.x + local
  return local
}
player.x = 10
result = move(2)
"#,
        );
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        let mut vm = q0vm::Vm::new();
        let report = vm.execute(&compiled.chunk, q0vm::DEFAULT_INSTRUCTION_BUDGET);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(vm.state.vars.get("player.x"), Some(&VmValue::Number(13.0)));
        assert_eq!(vm.state.vars.get("result"), Some(&VmValue::Number(3.0)));
        assert!(!vm.state.vars.contains_key("local"));
    }

    #[test]
    fn function_body_compiles_inline_and_call_uses_qvm_call_opcode() {
        let compiled = compile_source(
            r#"pb func plus_one x {
  return x + 1
}
result = plus_one(4)
"#,
        );
        assert!(
            compiled.diagnostics.is_empty(),
            "{:?}",
            compiled.diagnostics
        );
        assert!(compiled.chunk.code.contains(&(Opcode::DefinePublic as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::LoadLocal as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::Return as u8)));
        assert!(compiled.chunk.code.contains(&(Opcode::CallUser as u8)));
    }
}
