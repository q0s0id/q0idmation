use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const BYTECODE_MAGIC: [u8; 4] = *b"QVM\0";
pub const BYTECODE_VERSION: u16 = 1;
pub const DEFAULT_INSTRUCTION_BUDGET: usize = 100_000;
pub const DEFAULT_STACK_LIMIT: usize = 4_096;
pub const DEFAULT_CALL_DEPTH_LIMIT: usize = 128;
const MAX_CONSTANTS: usize = 65_536;
const MAX_CODE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_LINE_ENTRIES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Ident(String),
    Raw(String),
}

impl Value {
    pub fn display_lossy(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::String(value) | Self::Ident(value) | Self::Raw(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::Null => "null".to_string(),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Bool(_) => "bool",
            Self::Null => "null",
            Self::Ident(_) => "ident",
            Self::Raw(_) => "raw",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Null => false,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) | Self::Ident(value) | Self::Raw(value) => !value.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    pub name: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBytecode {
    pub visibility: Visibility,
    pub signature: FunctionSignature,
    pub chunk: Chunk,
    pub entry: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostAction {
    GoRun(Value),
    GoStop(Value),
    ShellCommand { command: String, args: Vec<Value> },
    RigCommand { command: String, args: Vec<Value> },
    SceneCommand { command: String, args: Vec<Value> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VmState {
    pub vars: BTreeMap<String, Value>,
    pub listeners: BTreeMap<String, Vec<String>>,
    pub imported_libraries: BTreeSet<String>,
    pub public_functions: BTreeMap<String, FunctionSignature>,
    pub private_functions: BTreeMap<String, FunctionSignature>,
    pub functions: BTreeMap<String, FunctionBytecode>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExecutionReport {
    pub actions: Vec<HostAction>,
    pub diagnostics: Vec<Diagnostic>,
    pub instructions_executed: usize,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Halt = 0,
    Constant = 1,
    LoadGlobal = 2,
    StoreGlobal = 3,
    Neg = 4,
    Add = 5,
    Sub = 6,
    Mul = 7,
    Div = 8,
    Rem = 9,
    Pow = 10,
    CallMath = 11,
    GoRun = 12,
    GoStop = 13,
    Shell = 14,
    Rig = 15,
    Listen = 16,
    Unlisten = 17,
    Import = 18,
    DefinePublic = 19,
    DefinePrivate = 20,
    PushRaw = 21,
    Pop = 22,
    Jump = 23,
    JumpIfFalse = 24,
    LoadLocal = 25,
    StoreLocal = 26,
    Equal = 27,
    NotEqual = 28,
    Less = 29,
    LessEqual = 30,
    Greater = 31,
    GreaterEqual = 32,
    Not = 33,
    ToBool = 34,
    CallUser = 35,
    Return = 36,
    Scene = 37,
}

impl TryFrom<u8> for Opcode {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Halt,
            1 => Self::Constant,
            2 => Self::LoadGlobal,
            3 => Self::StoreGlobal,
            4 => Self::Neg,
            5 => Self::Add,
            6 => Self::Sub,
            7 => Self::Mul,
            8 => Self::Div,
            9 => Self::Rem,
            10 => Self::Pow,
            11 => Self::CallMath,
            12 => Self::GoRun,
            13 => Self::GoStop,
            14 => Self::Shell,
            15 => Self::Rig,
            16 => Self::Listen,
            17 => Self::Unlisten,
            18 => Self::Import,
            19 => Self::DefinePublic,
            20 => Self::DefinePrivate,
            21 => Self::PushRaw,
            22 => Self::Pop,
            23 => Self::Jump,
            24 => Self::JumpIfFalse,
            25 => Self::LoadLocal,
            26 => Self::StoreLocal,
            27 => Self::Equal,
            28 => Self::NotEqual,
            29 => Self::Less,
            30 => Self::LessEqual,
            31 => Self::Greater,
            32 => Self::GreaterEqual,
            33 => Self::Not,
            34 => Self::ToBool,
            35 => Self::CallUser,
            36 => Self::Return,
            37 => Self::Scene,
            _ => return Err(()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEntry {
    pub offset: u32,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: Vec<Value>,
    pub lines: Vec<LineEntry>,
}

impl Chunk {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_constant(&mut self, value: Value) -> Result<u32, BytecodeError> {
        if self.constants.len() >= MAX_CONSTANTS {
            return Err(BytecodeError::Limit("constant pool exceeds 65536 entries"));
        }
        self.constants.push(value);
        Ok((self.constants.len() - 1) as u32)
    }

    pub fn emit_op(&mut self, opcode: Opcode, line: usize) -> usize {
        let offset = self.code.len();
        self.record_line(offset, line);
        self.code.push(opcode as u8);
        offset
    }

    pub fn emit_u8(&mut self, value: u8) {
        self.code.push(value);
    }

    pub fn emit_u32(&mut self, value: u32) {
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    pub fn patch_u32(&mut self, operand_offset: usize, value: u32) -> Result<(), BytecodeError> {
        let end = operand_offset
            .checked_add(4)
            .ok_or(BytecodeError::Malformed("patch offset overflow"))?;
        let dst = self
            .code
            .get_mut(operand_offset..end)
            .ok_or(BytecodeError::Malformed("patch outside bytecode"))?;
        dst.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    pub fn current_offset(&self) -> usize {
        self.code.len()
    }

    pub fn line_at(&self, offset: usize) -> usize {
        match self
            .lines
            .binary_search_by_key(&(offset as u32), |entry| entry.offset)
        {
            Ok(index) => self.lines[index].line as usize,
            Err(0) => 0,
            Err(index) => self.lines[index - 1].line as usize,
        }
    }

    fn record_line(&mut self, offset: usize, line: usize) {
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        let line = u32::try_from(line).unwrap_or(u32::MAX);
        if self.lines.last().is_some_and(|entry| entry.line == line) {
            return;
        }
        self.lines.push(LineEntry { offset, line });
    }

    pub fn encode(&self) -> Result<Vec<u8>, BytecodeError> {
        if self.code.len() > MAX_CODE_BYTES {
            return Err(BytecodeError::Limit("qvm bytecode exceeds 16 MiB"));
        }
        if self.lines.len() > MAX_LINE_ENTRIES {
            return Err(BytecodeError::Limit("qvm line table is too large"));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&BYTECODE_MAGIC);
        out.extend_from_slice(&BYTECODE_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.constants.len() as u32).to_le_bytes());
        for constant in &self.constants {
            match constant {
                Value::Number(value) => {
                    out.push(0);
                    out.extend_from_slice(&value.to_le_bytes());
                }
                Value::String(value) => {
                    out.push(1);
                    write_string(&mut out, value)?;
                }
                Value::Ident(value) => {
                    out.push(2);
                    write_string(&mut out, value)?;
                }
                Value::Raw(value) => {
                    out.push(3);
                    write_string(&mut out, value)?;
                }
                Value::Bool(value) => {
                    out.push(4);
                    out.push(u8::from(*value));
                }
                Value::Null => out.push(5),
            }
        }
        out.extend_from_slice(&(self.code.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.code);
        out.extend_from_slice(&(self.lines.len() as u32).to_le_bytes());
        for entry in &self.lines {
            out.extend_from_slice(&entry.offset.to_le_bytes());
            out.extend_from_slice(&entry.line.to_le_bytes());
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BytecodeError> {
        let mut cursor = Cursor::new(bytes);
        if cursor.read_exact(4)? != BYTECODE_MAGIC {
            return Err(BytecodeError::Malformed("bad qvm bytecode magic"));
        }
        let version = cursor.read_u16()?;
        if version != BYTECODE_VERSION {
            return Err(BytecodeError::UnsupportedVersion(version));
        }
        let constant_count = cursor.read_u32()? as usize;
        if constant_count > MAX_CONSTANTS {
            return Err(BytecodeError::Limit("constant pool exceeds 65536 entries"));
        }
        let mut constants = Vec::with_capacity(constant_count);
        for _ in 0..constant_count {
            let tag = cursor.read_u8()?;
            let value = match tag {
                0 => Value::Number(f64::from_le_bytes(cursor.read_array::<8>()?)),
                1 => Value::String(cursor.read_string()?),
                2 => Value::Ident(cursor.read_string()?),
                3 => Value::Raw(cursor.read_string()?),
                4 => match cursor.read_u8()? {
                    0 => Value::Bool(false),
                    1 => Value::Bool(true),
                    _ => return Err(BytecodeError::Malformed("invalid qvm bool constant")),
                },
                5 => Value::Null,
                _ => return Err(BytecodeError::Malformed("unknown qvm constant tag")),
            };
            constants.push(value);
        }
        let code_len = cursor.read_u32()? as usize;
        if code_len > MAX_CODE_BYTES {
            return Err(BytecodeError::Limit("qvm bytecode exceeds 16 MiB"));
        }
        let code = cursor.read_exact(code_len)?.to_vec();
        let line_count = cursor.read_u32()? as usize;
        if line_count > MAX_LINE_ENTRIES {
            return Err(BytecodeError::Limit("qvm line table is too large"));
        }
        let mut lines = Vec::with_capacity(line_count);
        let mut previous = None;
        for _ in 0..line_count {
            let entry = LineEntry {
                offset: cursor.read_u32()?,
                line: cursor.read_u32()?,
            };
            if entry.offset as usize >= code.len() && !code.is_empty() {
                return Err(BytecodeError::Malformed(
                    "qvm line entry points outside code",
                ));
            }
            if previous.is_some_and(|offset| entry.offset <= offset) {
                return Err(BytecodeError::Malformed("qvm line table is not ordered"));
            }
            previous = Some(entry.offset);
            lines.push(entry);
        }
        if !cursor.is_empty() {
            return Err(BytecodeError::Malformed("trailing bytes after qvm chunk"));
        }
        Ok(Self {
            code,
            constants,
            lines,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeError {
    Malformed(&'static str),
    UnsupportedVersion(u16),
    Limit(&'static str),
    Utf8,
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) | Self::Limit(message) => f.write_str(message),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported qvm bytecode version {version}")
            }
            Self::Utf8 => f.write_str("qvm bytecode contains invalid utf-8"),
        }
    }
}

impl std::error::Error for BytecodeError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Vm {
    pub state: VmState,
    stack: Vec<Value>,
    locals: Vec<BTreeMap<String, Value>>,
    ip: usize,
    stack_limit: usize,
    call_depth_limit: usize,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        Self {
            state: VmState::default(),
            stack: Vec::new(),
            locals: Vec::new(),
            ip: 0,
            stack_limit: DEFAULT_STACK_LIMIT,
            call_depth_limit: DEFAULT_CALL_DEPTH_LIMIT,
        }
    }

    pub fn with_state(state: VmState) -> Self {
        Self {
            state,
            ..Self::new()
        }
    }

    pub fn set_stack_limit(&mut self, limit: usize) {
        self.stack_limit = limit.max(1);
    }

    pub fn set_call_depth_limit(&mut self, limit: usize) {
        self.call_depth_limit = limit.max(1);
    }

    pub fn execute(&mut self, chunk: &Chunk, instruction_budget: usize) -> ExecutionReport {
        let mut report = ExecutionReport::default();
        self.stack.clear();
        self.locals.clear();
        self.ip = 0;
        let budget = instruction_budget.max(1);
        let result = self.run_range(chunk, chunk.code.len(), budget, &mut report);
        if let Err((line, message)) = result {
            report.diagnostics.push(Diagnostic::new(line, message));
        }
        report
    }

    fn run_range(
        &mut self,
        chunk: &Chunk,
        end: usize,
        budget: usize,
        report: &mut ExecutionReport,
    ) -> Result<RunSignal, (usize, String)> {
        while self.ip < end {
            if report.instructions_executed >= budget {
                if report.diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .message
                        .contains("qvm instruction budget exceeded")
                }) {
                    return Ok(RunSignal::Halt);
                }
                return Err((
                    chunk.line_at(self.ip),
                    format!("qvm instruction budget exceeded ({budget})"),
                ));
            }
            let instruction_offset = self.ip;
            let line = chunk.line_at(instruction_offset);
            let raw = self.read_u8(chunk).map_err(|message| (line, message))?;
            let opcode =
                Opcode::try_from(raw).map_err(|()| (line, format!("unknown qvm opcode {raw}")))?;
            report.instructions_executed += 1;

            let result = match opcode {
                Opcode::Halt => return Ok(RunSignal::Halt),
                Opcode::Return => {
                    let value = self.pop().map_err(|message| (line, message))?;
                    return Ok(RunSignal::Return(value));
                }
                Opcode::Constant => self.op_constant(chunk),
                Opcode::LoadGlobal => self.op_load_global(chunk),
                Opcode::StoreGlobal => self.op_store_global(chunk),
                Opcode::LoadLocal => self.op_load_local(chunk),
                Opcode::StoreLocal => self.op_store_local(chunk),
                Opcode::PushRaw => self.op_push_raw(chunk),
                Opcode::Neg => self.op_neg(report, line),
                Opcode::Not => self.op_not(),
                Opcode::ToBool => self.op_to_bool(),
                Opcode::Add => self.op_binary(BinaryKind::Add, report, line),
                Opcode::Sub => self.op_binary(BinaryKind::Sub, report, line),
                Opcode::Mul => self.op_binary(BinaryKind::Mul, report, line),
                Opcode::Div => self.op_binary(BinaryKind::Div, report, line),
                Opcode::Rem => self.op_binary(BinaryKind::Rem, report, line),
                Opcode::Pow => self.op_binary(BinaryKind::Pow, report, line),
                Opcode::Equal => self.op_equality(false),
                Opcode::NotEqual => self.op_equality(true),
                Opcode::Less => self.op_ordered_compare(CompareKind::Less, report, line),
                Opcode::LessEqual => self.op_ordered_compare(CompareKind::LessEqual, report, line),
                Opcode::Greater => self.op_ordered_compare(CompareKind::Greater, report, line),
                Opcode::GreaterEqual => {
                    self.op_ordered_compare(CompareKind::GreaterEqual, report, line)
                }
                Opcode::CallMath => self.op_call_math(chunk, report, line),
                Opcode::CallUser => self.op_call_user(chunk, budget, report, line),
                Opcode::GoRun => self.op_timeline(true, report),
                Opcode::GoStop => self.op_timeline(false, report),
                Opcode::Shell => self.op_host_call(chunk, HostChannel::Shell, report),
                Opcode::Rig => self.op_host_call(chunk, HostChannel::Rig, report),
                Opcode::Scene => self.op_host_call(chunk, HostChannel::Scene, report),
                Opcode::Listen => self.op_listener(chunk, false),
                Opcode::Unlisten => self.op_listener(chunk, true),
                Opcode::Import => self.op_import(chunk),
                Opcode::DefinePublic => self.op_define_function(chunk, Visibility::Public),
                Opcode::DefinePrivate => self.op_define_function(chunk, Visibility::Private),
                Opcode::Pop => self.pop().map(|_| ()),
                Opcode::Jump => self.op_jump(chunk),
                Opcode::JumpIfFalse => self.op_jump_if_false(chunk),
            };
            result.map_err(|message| (line, message))?;
        }
        Ok(RunSignal::Halt)
    }

    fn op_constant(&mut self, chunk: &Chunk) -> Result<(), String> {
        let index = self.read_u32(chunk)? as usize;
        let value = chunk
            .constants
            .get(index)
            .cloned()
            .ok_or_else(|| "qvm constant index outside pool".to_string())?;
        self.push(value)
    }

    fn op_push_raw(&mut self, chunk: &Chunk) -> Result<(), String> {
        let value = self.read_named_constant(chunk)?;
        self.push(Value::Raw(value))
    }

    fn op_load_global(&mut self, chunk: &Chunk) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let value = self
            .state
            .vars
            .get(&name)
            .cloned()
            .unwrap_or(match name.as_str() {
                "pi" => Value::Number(std::f64::consts::PI),
                "tau" => Value::Number(std::f64::consts::TAU),
                _ => Value::Ident(name),
            });
        self.push(value)
    }

    fn op_store_global(&mut self, chunk: &Chunk) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let value = self.pop()?;
        self.state.vars.insert(name, value);
        Ok(())
    }

    fn op_load_local(&mut self, chunk: &Chunk) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let value = self
            .locals
            .last()
            .and_then(|locals| locals.get(&name))
            .cloned()
            .ok_or_else(|| format!("unknown qvm local `{name}`"))?;
        self.push(value)
    }

    fn op_store_local(&mut self, chunk: &Chunk) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let value = self.pop()?;
        let locals = self
            .locals
            .last_mut()
            .ok_or_else(|| "qvm local store outside function".to_string())?;
        locals.insert(name, value);
        Ok(())
    }

    fn op_neg(&mut self, report: &mut ExecutionReport, line: usize) -> Result<(), String> {
        match self.pop()? {
            Value::Number(value) => self.push(Value::Number(-value)),
            other => {
                report.diagnostics.push(Diagnostic::new(
                    line,
                    format!("cannot negate `{}`", other.kind_name()),
                ));
                self.push(Value::Raw("nan".to_string()))
            }
        }
    }

    fn op_not(&mut self) -> Result<(), String> {
        let value = self.pop()?;
        self.push(Value::Bool(!value.is_truthy()))
    }

    fn op_to_bool(&mut self) -> Result<(), String> {
        let value = self.pop()?;
        self.push(Value::Bool(value.is_truthy()))
    }

    fn op_binary(
        &mut self,
        kind: BinaryKind,
        report: &mut ExecutionReport,
        line: usize,
    ) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (left, right) {
            (Value::Number(a), Value::Number(b)) => Value::Number(match kind {
                BinaryKind::Add => a + b,
                BinaryKind::Sub => a - b,
                BinaryKind::Mul => a * b,
                BinaryKind::Div => a / b,
                BinaryKind::Rem => a % b,
                BinaryKind::Pow => a.powf(b),
            }),
            (Value::String(a), Value::String(b)) if kind == BinaryKind::Add => {
                Value::String(format!("{a}{b}"))
            }
            (Value::String(a), b) if kind == BinaryKind::Add => {
                Value::String(format!("{a}{}", b.display_lossy()))
            }
            (a, Value::String(b)) if kind == BinaryKind::Add => {
                Value::String(format!("{}{b}", a.display_lossy()))
            }
            (a, b) => {
                report.diagnostics.push(Diagnostic::new(
                    line,
                    format!(
                        "cannot apply {} to `{}` and `{}`",
                        kind.label(),
                        a.kind_name(),
                        b.kind_name()
                    ),
                ));
                Value::Raw("nan".to_string())
            }
        };
        self.push(result)
    }

    fn op_equality(&mut self, negate: bool) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let equal = values_equal(&left, &right);
        self.push(Value::Bool(if negate { !equal } else { equal }))
    }

    fn op_ordered_compare(
        &mut self,
        kind: CompareKind,
        report: &mut ExecutionReport,
        line: usize,
    ) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let value = match (&left, &right) {
            (Value::Number(a), Value::Number(b)) => kind.compare_partial(*a, *b),
            (Value::String(a), Value::String(b)) => Some(kind.compare_ord(a.cmp(b))),
            _ => None,
        };
        match value {
            Some(value) => self.push(Value::Bool(value)),
            None => {
                report.diagnostics.push(Diagnostic::new(
                    line,
                    format!(
                        "cannot compare `{}` and `{}` with {}",
                        left.kind_name(),
                        right.kind_name(),
                        kind.label()
                    ),
                ));
                self.push(Value::Bool(false))
            }
        }
    }

    fn op_call_math(
        &mut self,
        chunk: &Chunk,
        report: &mut ExecutionReport,
        line: usize,
    ) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let argc = self.read_u8(chunk)? as usize;
        let args = self.pop_args(argc)?;
        let number = |index: usize| match args.get(index) {
            Some(Value::Number(value)) => Some(*value),
            _ => None,
        };
        let result = match (name.as_str(), argc) {
            ("sin", 1) => number(0).map(f64::sin),
            ("cos", 1) => number(0).map(f64::cos),
            ("tan", 1) => number(0).map(f64::tan),
            ("sqrt", 1) => number(0).map(f64::sqrt),
            ("abs", 1) => number(0).map(f64::abs),
            ("floor", 1) => number(0).map(f64::floor),
            ("ceil", 1) => number(0).map(f64::ceil),
            ("round", 1) => number(0).map(f64::round),
            ("min", 2) => number(0).zip(number(1)).map(|(a, b)| a.min(b)),
            ("max", 2) => number(0).zip(number(1)).map(|(a, b)| a.max(b)),
            ("clamp", 3) => number(0)
                .zip(number(1))
                .zip(number(2))
                .and_then(|((x, lo), hi)| {
                    if lo.is_nan() || hi.is_nan() || lo > hi {
                        None
                    } else {
                        Some(x.clamp(lo, hi))
                    }
                }),
            ("lerp", 3) => number(0)
                .zip(number(1))
                .zip(number(2))
                .map(|((a, b), t)| a + (b - a) * t),
            _ => None,
        };
        match result {
            Some(value) => self.push(Value::Number(value)),
            None => {
                report.diagnostics.push(Diagnostic::new(
                    line,
                    format!("bad args for math call `{name}`; q0.math wants numbers here"),
                ));
                self.push(Value::Raw(format!("{name}(?)")))
            }
        }
    }

    fn op_call_user(
        &mut self,
        chunk: &Chunk,
        budget: usize,
        report: &mut ExecutionReport,
        line: usize,
    ) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let argc = self.read_u8(chunk)? as usize;
        let args = self.pop_args(argc)?;
        let function = self
            .state
            .functions
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("unknown q0lang function `{name}`"))?;
        if function.signature.params.len() != args.len() {
            report.diagnostics.push(Diagnostic::new(
                line,
                format!(
                    "function `{name}` expects {} argument(s), got {}",
                    function.signature.params.len(),
                    args.len()
                ),
            ));
            return self.push(Value::Null);
        }
        if self.locals.len() >= self.call_depth_limit {
            return Err(format!(
                "qvm call depth limit exceeded ({})",
                self.call_depth_limit
            ));
        }

        let locals = function
            .signature
            .params
            .iter()
            .cloned()
            .zip(args)
            .collect::<BTreeMap<_, _>>();
        let return_ip = self.ip;
        let stack_base = self.stack.len();
        self.locals.push(locals);
        self.ip = function.entry;
        let nested = self.run_range(&function.chunk, function.end, budget, report);
        self.locals.pop();
        self.ip = return_ip;
        self.stack.truncate(stack_base);

        match nested {
            Ok(RunSignal::Return(value)) => self.push(value),
            Ok(RunSignal::Halt) => self.push(Value::Null),
            Err((nested_line, message)) => {
                report
                    .diagnostics
                    .push(Diagnostic::new(nested_line, message));
                self.push(Value::Null)
            }
        }
    }

    fn op_timeline(&mut self, run: bool, report: &mut ExecutionReport) -> Result<(), String> {
        let target = self.pop()?;
        report.actions.push(if run {
            HostAction::GoRun(target)
        } else {
            HostAction::GoStop(target)
        });
        Ok(())
    }

    fn op_host_call(
        &mut self,
        chunk: &Chunk,
        channel: HostChannel,
        report: &mut ExecutionReport,
    ) -> Result<(), String> {
        let command = self.read_named_constant(chunk)?;
        let argc = self.read_u8(chunk)? as usize;
        let args = self.pop_args(argc)?;
        report.actions.push(match channel {
            HostChannel::Shell => HostAction::ShellCommand { command, args },
            HostChannel::Rig => HostAction::RigCommand { command, args },
            HostChannel::Scene => HostAction::SceneCommand { command, args },
        });
        Ok(())
    }

    fn op_listener(&mut self, chunk: &Chunk, remove: bool) -> Result<(), String> {
        let event = self.read_named_constant(chunk)?;
        let handler = self.read_named_constant(chunk)?;
        if remove {
            if let Some(handlers) = self.state.listeners.get_mut(&event) {
                handlers.retain(|existing| existing != &handler);
            }
            if self.state.listeners.get(&event).is_some_and(Vec::is_empty) {
                self.state.listeners.remove(&event);
            }
        } else {
            let handlers = self.state.listeners.entry(event).or_default();
            if !handlers.iter().any(|existing| existing == &handler) {
                handlers.push(handler);
            }
        }
        Ok(())
    }

    fn op_import(&mut self, chunk: &Chunk) -> Result<(), String> {
        let library = self.read_named_constant(chunk)?;
        self.state.imported_libraries.insert(library);
        Ok(())
    }

    fn op_define_function(&mut self, chunk: &Chunk, visibility: Visibility) -> Result<(), String> {
        let name = self.read_named_constant(chunk)?;
        let argc = self.read_u8(chunk)? as usize;
        let mut params = Vec::with_capacity(argc);
        for _ in 0..argc {
            params.push(self.read_named_constant(chunk)?);
        }
        let body_end = self.read_u32(chunk)? as usize;
        if body_end < self.ip || body_end > chunk.code.len() {
            return Err("qvm function body range is invalid".to_string());
        }
        let signature = FunctionSignature {
            name: name.clone(),
            params,
        };
        let function = FunctionBytecode {
            visibility,
            signature: signature.clone(),
            chunk: chunk.clone(),
            entry: self.ip,
            end: body_end,
        };
        self.state.functions.insert(name.clone(), function);
        match visibility {
            Visibility::Public => {
                self.state.public_functions.insert(name, signature);
            }
            Visibility::Private => {
                self.state.private_functions.insert(name, signature);
            }
        }
        self.ip = body_end;
        Ok(())
    }

    fn op_jump(&mut self, chunk: &Chunk) -> Result<(), String> {
        let target = self.read_u32(chunk)? as usize;
        self.set_ip(chunk, target)
    }

    fn op_jump_if_false(&mut self, chunk: &Chunk) -> Result<(), String> {
        let target = self.read_u32(chunk)? as usize;
        let condition = self.pop()?;
        if !condition.is_truthy() {
            self.set_ip(chunk, target)?;
        }
        Ok(())
    }

    fn set_ip(&mut self, chunk: &Chunk, target: usize) -> Result<(), String> {
        if target > chunk.code.len() {
            return Err("qvm jump target outside bytecode".to_string());
        }
        self.ip = target;
        Ok(())
    }

    fn push(&mut self, value: Value) -> Result<(), String> {
        if self.stack.len() >= self.stack_limit {
            return Err(format!("qvm stack limit exceeded ({})", self.stack_limit));
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Result<Value, String> {
        self.stack
            .pop()
            .ok_or_else(|| "qvm operand stack underflow".to_string())
    }

    fn pop_args(&mut self, count: usize) -> Result<Vec<Value>, String> {
        if self.stack.len() < count {
            return Err("qvm operand stack underflow while collecting arguments".to_string());
        }
        let start = self.stack.len() - count;
        Ok(self.stack.drain(start..).collect())
    }

    fn read_named_constant(&mut self, chunk: &Chunk) -> Result<String, String> {
        let index = self.read_u32(chunk)? as usize;
        match chunk.constants.get(index) {
            Some(Value::String(value)) | Some(Value::Ident(value)) | Some(Value::Raw(value)) => {
                Ok(value.clone())
            }
            Some(Value::Number(_) | Value::Bool(_) | Value::Null) => {
                Err("qvm name operand points to a non-string constant".to_string())
            }
            None => Err("qvm constant index outside pool".to_string()),
        }
    }

    fn read_u8(&mut self, chunk: &Chunk) -> Result<u8, String> {
        let value = *chunk
            .code
            .get(self.ip)
            .ok_or_else(|| "truncated qvm bytecode operand".to_string())?;
        self.ip += 1;
        Ok(value)
    }

    fn read_u32(&mut self, chunk: &Chunk) -> Result<u32, String> {
        let end = self
            .ip
            .checked_add(4)
            .ok_or_else(|| "qvm instruction pointer overflow".to_string())?;
        let bytes = chunk
            .code
            .get(self.ip..end)
            .ok_or_else(|| "truncated qvm bytecode operand".to_string())?;
        self.ip = end;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("four-byte slice"),
        ))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RunSignal {
    Halt,
    Return(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostChannel {
    Shell,
    Rig,
    Scene,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryKind {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

impl BinaryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::Mul => "Mul",
            Self::Div => "Div",
            Self::Rem => "Rem",
            Self::Pow => "Pow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompareKind {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl CompareKind {
    fn label(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
        }
    }

    fn compare_partial(self, a: f64, b: f64) -> Option<bool> {
        if a.is_nan() || b.is_nan() {
            return None;
        }
        Some(match self {
            Self::Less => a < b,
            Self::LessEqual => a <= b,
            Self::Greater => a > b,
            Self::GreaterEqual => a >= b,
        })
    }

    fn compare_ord(self, ordering: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Less => ordering == Less,
            Self::LessEqual => matches!(ordering, Less | Equal),
            Self::Greater => ordering == Greater,
            Self::GreaterEqual => matches!(ordering, Greater | Equal),
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        (Value::Ident(a), Value::Ident(b)) => a == b,
        (Value::Raw(a), Value::Raw(b)) => a == b,
        _ => false,
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), BytecodeError> {
    let bytes = value.as_bytes();
    if bytes.len() > MAX_STRING_BYTES {
        return Err(BytecodeError::Limit("qvm string constant exceeds 1 MiB"));
    }
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], BytecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(BytecodeError::Malformed("qvm cursor overflow"))?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(BytecodeError::Malformed("truncated qvm bytecode"))?;
        self.pos = end;
        Ok(bytes)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], BytecodeError> {
        self.read_exact(N)?
            .try_into()
            .map_err(|_| BytecodeError::Malformed("bad fixed qvm field"))
    }

    fn read_u8(&mut self) -> Result<u8, BytecodeError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, BytecodeError> {
        Ok(u16::from_le_bytes(self.read_array::<2>()?))
    }

    fn read_u32(&mut self) -> Result<u32, BytecodeError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    fn read_string(&mut self) -> Result<String, BytecodeError> {
        let len = self.read_u32()? as usize;
        if len > MAX_STRING_BYTES {
            return Err(BytecodeError::Limit("qvm string constant exceeds 1 MiB"));
        }
        let bytes = self.read_exact(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| BytecodeError::Utf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string_constant(chunk: &mut Chunk, value: &str) -> u32 {
        chunk
            .add_constant(Value::String(value.to_string()))
            .expect("constant")
    }

    #[test]
    fn bytecode_chunk_roundtrips_deterministically() {
        let mut chunk = Chunk::new();
        let values = [
            Value::Number(7.0),
            Value::Bool(true),
            Value::Null,
            Value::String("q0".into()),
        ];
        for value in values {
            chunk.add_constant(value).expect("constant");
        }
        chunk.emit_op(Opcode::Halt, 3);

        let bytes = chunk.encode().expect("encode");
        let decoded = Chunk::decode(&bytes).expect("decode");
        assert_eq!(decoded, chunk);
        assert_eq!(decoded.encode().expect("re-encode"), bytes);
    }

    #[test]
    fn vm_executes_globals_math_and_host_actions() {
        let mut chunk = Chunk::new();
        let two = chunk.add_constant(Value::Number(2.0)).expect("two");
        let three = chunk.add_constant(Value::Number(3.0)).expect("three");
        let x = string_constant(&mut chunk, "x");
        let round = string_constant(&mut chunk, "round");
        let print = string_constant(&mut chunk, "print");

        chunk.emit_op(Opcode::Constant, 1);
        chunk.emit_u32(two);
        chunk.emit_op(Opcode::Constant, 1);
        chunk.emit_u32(three);
        chunk.emit_op(Opcode::Add, 1);
        chunk.emit_op(Opcode::StoreGlobal, 1);
        chunk.emit_u32(x);
        chunk.emit_op(Opcode::LoadGlobal, 2);
        chunk.emit_u32(x);
        chunk.emit_op(Opcode::CallMath, 2);
        chunk.emit_u32(round);
        chunk.emit_u8(1);
        chunk.emit_op(Opcode::Shell, 2);
        chunk.emit_u32(print);
        chunk.emit_u8(1);
        chunk.emit_op(Opcode::Halt, 2);

        let mut vm = Vm::new();
        let report = vm.execute(&chunk, DEFAULT_INSTRUCTION_BUDGET);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(vm.state.vars["x"], Value::Number(5.0));
        assert_eq!(
            report.actions,
            vec![HostAction::ShellCommand {
                command: "print".into(),
                args: vec![Value::Number(5.0)],
            }]
        );
    }

    #[test]
    fn self_jump_is_stopped_by_instruction_budget_with_source_line() {
        let mut chunk = Chunk::new();
        chunk.emit_op(Opcode::Jump, 42);
        chunk.emit_u32(0);
        let mut vm = Vm::new();
        let report = vm.execute(&chunk, 17);
        assert_eq!(report.instructions_executed, 17);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 42);
        assert!(report.diagnostics[0]
            .message
            .contains("instruction budget exceeded"));
    }

    #[test]
    fn stack_limit_is_enforced_inside_vm() {
        let mut chunk = Chunk::new();
        let value = chunk.add_constant(Value::Number(1.0)).expect("constant");
        for _ in 0..3 {
            chunk.emit_op(Opcode::Constant, 9);
            chunk.emit_u32(value);
        }
        let mut vm = Vm::new();
        vm.set_stack_limit(2);
        let report = vm.execute(&chunk, 100);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 9);
        assert!(report.diagnostics[0]
            .message
            .contains("stack limit exceeded"));
    }

    #[test]
    fn host_state_persists_across_chunks_but_operand_stack_does_not() {
        let mut first = Chunk::new();
        let one = first.add_constant(Value::Number(1.0)).expect("one");
        let x = string_constant(&mut first, "x");
        first.emit_op(Opcode::Constant, 1);
        first.emit_u32(one);
        first.emit_op(Opcode::StoreGlobal, 1);
        first.emit_u32(x);
        first.emit_op(Opcode::Halt, 1);

        let mut second = Chunk::new();
        let x2 = string_constant(&mut second, "x");
        second.emit_op(Opcode::LoadGlobal, 2);
        second.emit_u32(x2);
        second.emit_op(Opcode::GoRun, 2);
        second.emit_op(Opcode::Halt, 2);

        let mut vm = Vm::new();
        assert!(vm.execute(&first, 100).diagnostics.is_empty());
        let report = vm.execute(&second, 100);
        assert_eq!(report.actions, vec![HostAction::GoRun(Value::Number(1.0))]);
    }

    #[test]
    fn semantic_type_error_is_recoverable_and_later_bytecode_runs() {
        let mut chunk = Chunk::new();
        let text = chunk
            .add_constant(Value::String("bad".to_string()))
            .expect("text");
        let bad = string_constant(&mut chunk, "bad");
        let ok_value = chunk.add_constant(Value::Number(7.0)).expect("seven");
        let ok = string_constant(&mut chunk, "ok");
        chunk.emit_op(Opcode::Constant, 4);
        chunk.emit_u32(text);
        chunk.emit_op(Opcode::Neg, 4);
        chunk.emit_op(Opcode::StoreGlobal, 4);
        chunk.emit_u32(bad);
        chunk.emit_op(Opcode::Constant, 5);
        chunk.emit_u32(ok_value);
        chunk.emit_op(Opcode::StoreGlobal, 5);
        chunk.emit_u32(ok);
        chunk.emit_op(Opcode::Halt, 5);

        let mut vm = Vm::new();
        let report = vm.execute(&chunk, 100);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 4);
        assert_eq!(vm.state.vars["bad"], Value::Raw("nan".into()));
        assert_eq!(vm.state.vars["ok"], Value::Number(7.0));
    }

    #[test]
    fn invalid_clamp_reports_diagnostic_without_panicking() {
        let mut chunk = Chunk::new();
        let values = [1.0, 10.0, 2.0]
            .into_iter()
            .map(|value| chunk.add_constant(Value::Number(value)).expect("number"))
            .collect::<Vec<_>>();
        let clamp = string_constant(&mut chunk, "clamp");
        let result = string_constant(&mut chunk, "result");
        for value in values {
            chunk.emit_op(Opcode::Constant, 8);
            chunk.emit_u32(value);
        }
        chunk.emit_op(Opcode::CallMath, 8);
        chunk.emit_u32(clamp);
        chunk.emit_u8(3);
        chunk.emit_op(Opcode::StoreGlobal, 8);
        chunk.emit_u32(result);
        chunk.emit_op(Opcode::Halt, 8);

        let mut vm = Vm::new();
        let report = vm.execute(&chunk, 100);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 8);
        assert_eq!(vm.state.vars["result"], Value::Raw("clamp(?)".into()));
    }

    #[test]
    fn comparison_and_boolean_opcodes_produce_real_bool_values() {
        let mut chunk = Chunk::new();
        let a = chunk.add_constant(Value::Number(2.0)).expect("a");
        let b = chunk.add_constant(Value::Number(3.0)).expect("b");
        let result = string_constant(&mut chunk, "result");
        chunk.emit_op(Opcode::Constant, 1);
        chunk.emit_u32(a);
        chunk.emit_op(Opcode::Constant, 1);
        chunk.emit_u32(b);
        chunk.emit_op(Opcode::Less, 1);
        chunk.emit_op(Opcode::Not, 1);
        chunk.emit_op(Opcode::StoreGlobal, 1);
        chunk.emit_u32(result);
        chunk.emit_op(Opcode::Halt, 1);

        let mut vm = Vm::new();
        let report = vm.execute(&chunk, 100);
        assert!(report.diagnostics.is_empty());
        assert_eq!(vm.state.vars["result"], Value::Bool(false));
    }

    #[test]
    fn inline_function_bytecode_registers_and_calls_with_locals() {
        let mut chunk = Chunk::new();
        let name = string_constant(&mut chunk, "plus_one");
        let x = string_constant(&mut chunk, "x");
        let one = chunk.add_constant(Value::Number(1.0)).expect("one");
        let five = chunk.add_constant(Value::Number(5.0)).expect("five");
        let result = string_constant(&mut chunk, "result");

        chunk.emit_op(Opcode::DefinePublic, 1);
        chunk.emit_u32(name);
        chunk.emit_u8(1);
        chunk.emit_u32(x);
        let end_patch = chunk.current_offset();
        chunk.emit_u32(0);
        chunk.emit_op(Opcode::LoadLocal, 2);
        chunk.emit_u32(x);
        chunk.emit_op(Opcode::Constant, 2);
        chunk.emit_u32(one);
        chunk.emit_op(Opcode::Add, 2);
        chunk.emit_op(Opcode::Return, 2);
        let function_end = chunk.current_offset();
        chunk
            .patch_u32(end_patch, function_end as u32)
            .expect("patch function end");

        chunk.emit_op(Opcode::Constant, 4);
        chunk.emit_u32(five);
        chunk.emit_op(Opcode::CallUser, 4);
        chunk.emit_u32(name);
        chunk.emit_u8(1);
        chunk.emit_op(Opcode::StoreGlobal, 4);
        chunk.emit_u32(result);
        chunk.emit_op(Opcode::Halt, 4);

        let mut vm = Vm::new();
        let report = vm.execute(&chunk, 100);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(vm.state.vars["result"], Value::Number(6.0));
        assert_eq!(vm.state.public_functions["plus_one"].params, vec!["x"]);
    }

    #[test]
    fn recursion_uses_one_shared_instruction_budget() {
        let mut chunk = Chunk::new();
        let name = string_constant(&mut chunk, "forever");
        chunk.emit_op(Opcode::DefinePrivate, 1);
        chunk.emit_u32(name);
        chunk.emit_u8(0);
        let end_patch = chunk.current_offset();
        chunk.emit_u32(0);
        chunk.emit_op(Opcode::CallUser, 2);
        chunk.emit_u32(name);
        chunk.emit_u8(0);
        chunk.emit_op(Opcode::Return, 2);
        let function_end = chunk.current_offset();
        chunk
            .patch_u32(end_patch, function_end as u32)
            .expect("patch function end");
        chunk.emit_op(Opcode::CallUser, 4);
        chunk.emit_u32(name);
        chunk.emit_u8(0);
        chunk.emit_op(Opcode::Pop, 4);
        chunk.emit_op(Opcode::Halt, 4);

        let mut vm = Vm::new();
        vm.set_call_depth_limit(10_000);
        let report = vm.execute(&chunk, 40);
        assert!(report.instructions_executed <= 40);
        let budget_diagnostics = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("instruction budget exceeded"))
            .count();
        assert_eq!(budget_diagnostics, 1);
    }
}
