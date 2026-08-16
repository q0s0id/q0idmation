//! q0lang parser shared by editor, compiler and player.
//!
//! Blocks are parsed into a real recursive AST. Runtime never reparses block
//! source text: source parsing ends here, before qvm bytecode compilation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub statement_lines: Vec<usize>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub statement_lines: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Assignment {
        name: String,
        value: Value,
    },
    Expression {
        value: Value,
    },
    TimelineSignal {
        kind: TimelineSignal,
        target: Value,
    },
    ShellCommand {
        command: String,
        args: Vec<Value>,
    },
    RigCommand {
        command: String,
        args: Vec<Value>,
    },
    SceneCommand {
        command: String,
        args: Vec<Value>,
    },
    Listen {
        event: String,
        handler: String,
    },
    Unlisten {
        event: String,
        handler: String,
    },
    Function {
        visibility: Visibility,
        name: String,
        params: Vec<String>,
        body: Option<Block>,
    },
    DoBlock {
        body: Block,
    },
    If {
        condition: Value,
        then_body: Block,
        else_body: Option<Block>,
    },
    While {
        condition: Value,
        body: Block,
    },
    Return {
        value: Option<Value>,
    },
    Import {
        library: String,
    },
    Unknown {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineSignal {
    GoRun,
    GoStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Ident(String),
    Number(String),
    String(String),
    Bool(bool),
    Null,
    Unary {
        op: UnaryOp,
        value: Box<Value>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Value>,
        right: Box<Value>,
    },
    Call {
        name: String,
        args: Vec<Value>,
    },
    Raw(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

pub const MAX_EXPRESSION_NESTING_DEPTH: usize = 64;
pub const MAX_BLOCK_NESTING_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinLibrary {
    pub name: &'static str,
    pub symbols: &'static [&'static str],
}

pub const BUILTIN_LIBRARIES: &[BuiltinLibrary] = &[
    BuiltinLibrary {
        name: "q0.mouse",
        symbols: &[
            "mouse.down",
            "mouse.up",
            "mouse.move",
            "mouse.click",
            "mouse.double",
            "mouse.wheel",
            "mouse.over",
            "mouse.out",
        ],
    },
    BuiltinLibrary {
        name: "q0.timeline",
        symbols: &["gorun!", "gostop!", "frame", "label"],
    },
    BuiltinLibrary {
        name: "q0.input",
        symbols: &[
            "input.left",
            "input.right",
            "input.jump",
            "input.jump_pressed",
        ],
    },
    BuiltinLibrary {
        name: "q0.scene",
        symbols: &[
            "stage.width",
            "stage.height",
            "player.x",
            "player.y",
            "player.left",
            "player.right",
            "player.top",
            "player.bottom",
            "player.width",
            "player.height",
            "q0scene.switch!",
        ],
    },
    BuiltinLibrary {
        name: "q0.time",
        symbols: &["time.dt"],
    },
    BuiltinLibrary {
        name: "q0.math",
        symbols: &[
            "pi", "tau", "sin", "cos", "tan", "sqrt", "abs", "min", "max", "clamp", "lerp",
            "floor", "ceil", "round",
        ],
    },
    BuiltinLibrary {
        name: "q0shell",
        symbols: &[
            "q0shell.print!",
            "q0shell.read!",
            "q0shell.write!",
            "q0shell.append!",
            "q0shell.mkdir!",
            "q0shell.list!",
            "q0shell.move!",
            "q0shell.delete!",
        ],
    },
    BuiltinLibrary {
        name: "q0.rig",
        symbols: &[
            "q0rig.position!",
            "q0rig.value!",
            "q0rig.reset!",
            "q0rig.pose!",
            "q0rig.pose_reset!",
        ],
    },
    BuiltinLibrary {
        name: "q0.core",
        symbols: &[
            "do!", "listen", "xlisten", "pb", "pr", "func", "if", "else", "while", "return",
            "true", "false", "null", "and", "or", "not",
        ],
    },
];

pub fn parse(src: &str) -> Program {
    Parser::new(src).parse_program()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostCommandKind {
    Shell,
    Rig,
    Scene,
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            lines: src.lines().collect(),
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse_program(mut self) -> Program {
        let mut statements = Vec::new();
        let mut statement_lines = Vec::new();
        while self.pos < self.lines.len() {
            let line_no = self.pos + 1;
            let line = strip_comment(self.lines[self.pos]).trim().to_string();
            if line.is_empty() {
                self.pos += 1;
                continue;
            }
            if line.starts_with('}') {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "unexpected closing brace".to_string(),
                });
                self.pos += 1;
                continue;
            }
            if line.starts_with("else") {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "unexpected else without matching if".to_string(),
                });
                self.pos += 1;
                continue;
            }
            if let Some(statement) = self.parse_statement(0) {
                statements.push(statement);
                statement_lines.push(line_no);
            }
        }
        Program {
            statements,
            statement_lines,
            diagnostics: self.diagnostics,
        }
    }

    fn parse_statement(&mut self, depth: usize) -> Option<Statement> {
        if depth >= MAX_BLOCK_NESTING_DEPTH {
            let line_no = self.pos + 1;
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: format!(
                    "q0lang block nesting is too deep (maximum {MAX_BLOCK_NESTING_DEPTH})"
                ),
            });
            self.pos += 1;
            return None;
        }

        let line_no = self.pos + 1;
        let line = strip_comment(self.lines[self.pos]).trim().to_string();
        self.pos += 1;
        if line.is_empty() {
            return None;
        }
        let parts = split_words(&line);
        let first = parts.first().copied()?;

        match first {
            "import" => self.parse_import(line_no, &parts),
            "gorun!" => self.parse_timeline(line_no, TimelineSignal::GoRun, &line, &parts),
            "gostop!" => self.parse_timeline(line_no, TimelineSignal::GoStop, &line, &parts),
            "gorun" | "gostop" => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: format!("execution signal `{first}` must end with !"),
                });
                Some(Statement::Unknown { text: line })
            }
            "listen" => self.parse_listen(line_no, &parts, false),
            "xlisten" => self.parse_listen(line_no, &parts, true),
            "pb" => self.parse_function(line_no, &line, Visibility::Public, depth),
            "pr" => self.parse_function(line_no, &line, Visibility::Private, depth),
            "do!" => self.parse_do(line_no, &line, depth),
            "do" => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "execution block `do` must be written as do!".to_string(),
                });
                if line.contains('{') {
                    let _ = self.parse_block(line_no, "do", depth + 1);
                }
                None
            }
            "if" => self.parse_if(line_no, &line, depth),
            "while" => self.parse_while(line_no, &line, depth),
            "return" => self.parse_return(line_no, &line),
            "else" => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "unexpected else without matching if".to_string(),
                });
                None
            }
            _ if first.starts_with("q0shell.") && first.ends_with('!') => {
                self.parse_host_command(line_no, &line, first, HostCommandKind::Shell)
            }
            _ if first.starts_with("q0rig.") && first.ends_with('!') => {
                self.parse_host_command(line_no, &line, first, HostCommandKind::Rig)
            }
            _ if first.starts_with("q0scene.") && first.ends_with('!') => {
                self.parse_host_command(line_no, &line, first, HostCommandKind::Scene)
            }
            _ if find_assignment_operator(&line).is_some() => self.parse_assignment(line_no, &line),
            _ if first.ends_with('!') => Some(Statement::Unknown { text: line }),
            _ => {
                let before = self.diagnostics.len();
                let value = parse_value(&line, line_no, &mut self.diagnostics);
                if self.diagnostics.len() == before && matches!(value, Value::Call { .. }) {
                    Some(Statement::Expression { value })
                } else {
                    if self.diagnostics.len() == before {
                        self.diagnostics.push(Diagnostic {
                            line: line_no,
                            message: "unknown q0lang statement".to_string(),
                        });
                    }
                    Some(Statement::Unknown { text: line })
                }
            }
        }
    }

    fn parse_import(&mut self, line_no: usize, parts: &[&str]) -> Option<Statement> {
        match parts {
            ["import", library] => {
                if !BUILTIN_LIBRARIES
                    .iter()
                    .any(|builtin| builtin.name == *library)
                {
                    self.diagnostics.push(Diagnostic {
                        line: line_no,
                        message: format!("unknown library `{library}`"),
                    });
                }
                Some(Statement::Import {
                    library: (*library).to_string(),
                })
            }
            _ => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "import expects one library name".to_string(),
                });
                None
            }
        }
    }

    fn parse_timeline(
        &mut self,
        line_no: usize,
        kind: TimelineSignal,
        line: &str,
        parts: &[&str],
    ) -> Option<Statement> {
        if parts.len() < 2 {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "timeline signal expects a target".to_string(),
            });
            return None;
        }
        let target_src = line[parts[0].len()..].trim();
        let target = parse_value(target_src, line_no, &mut self.diagnostics);
        Some(Statement::TimelineSignal { kind, target })
    }

    fn parse_listen(&mut self, line_no: usize, parts: &[&str], remove: bool) -> Option<Statement> {
        if parts.len() != 3 {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: if remove {
                    "xlisten expects: xlisten event handler"
                } else {
                    "listen expects: listen event handler"
                }
                .to_string(),
            });
            return None;
        }
        let event = parts[1].to_string();
        if !is_mouse_event(&event) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: format!("event `{event}` is not in q0.mouse built-ins yet"),
            });
        }
        Some(if remove {
            Statement::Unlisten {
                event,
                handler: parts[2].to_string(),
            }
        } else {
            Statement::Listen {
                event,
                handler: parts[2].to_string(),
            }
        })
    }

    fn parse_host_command(
        &mut self,
        line_no: usize,
        line: &str,
        first: &str,
        kind: HostCommandKind,
    ) -> Option<Statement> {
        let prefix = match kind {
            HostCommandKind::Shell => "q0shell.",
            HostCommandKind::Rig => "q0rig.",
            HostCommandKind::Scene => "q0scene.",
        };
        let command = first
            .trim_start_matches(prefix)
            .trim_end_matches('!')
            .to_string();
        let raw_args = line[first.len()..].trim();
        let args = if raw_args.is_empty() {
            Vec::new()
        } else {
            split_top_level_commas(raw_args)
                .into_iter()
                .map(|arg| parse_value(arg.trim(), line_no, &mut self.diagnostics))
                .collect()
        };
        Some(match kind {
            HostCommandKind::Shell => Statement::ShellCommand { command, args },
            HostCommandKind::Rig => Statement::RigCommand { command, args },
            HostCommandKind::Scene => Statement::SceneCommand { command, args },
        })
    }

    fn parse_function(
        &mut self,
        line_no: usize,
        line: &str,
        visibility: Visibility,
        depth: usize,
    ) -> Option<Statement> {
        let has_body = line.ends_with('{');
        let header = if has_body {
            line[..line.len() - 1].trim_end()
        } else {
            line
        };
        let parts = split_words(header);
        if parts.len() < 3 || parts[1] != "func" {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message:
                    "function expects: pb func name [params...] { ... } or signature-only form"
                        .to_string(),
            });
            return None;
        }
        let name = parts[2];
        if !is_plain_ident(name) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "function name must be an identifier".to_string(),
            });
            return None;
        }
        let params = parts[3..]
            .iter()
            .map(|param| (*param).to_string())
            .collect::<Vec<_>>();
        if let Some(bad) = params.iter().find(|param| !is_plain_ident(param)) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: format!("function parameter `{bad}` must be an identifier"),
            });
        }
        let body = if has_body {
            Some(self.parse_block(line_no, "function", depth + 1).0)
        } else {
            None
        };
        Some(Statement::Function {
            visibility,
            name: name.to_string(),
            params,
            body,
        })
    }

    fn parse_do(&mut self, line_no: usize, line: &str, depth: usize) -> Option<Statement> {
        if !line.ends_with('{') {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "do! expects an opening { at the end of the line".to_string(),
            });
            return None;
        }
        let (body, _, _) = self.parse_block(line_no, "do!", depth + 1);
        Some(Statement::DoBlock { body })
    }

    fn parse_if(&mut self, line_no: usize, line: &str, depth: usize) -> Option<Statement> {
        let Some(condition_src) = block_header_expression(line, "if") else {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "if expects: if condition {".to_string(),
            });
            return None;
        };
        let condition = parse_value(condition_src, line_no, &mut self.diagnostics);
        let (then_body, suffix, close_line) = self.parse_block(line_no, "if", depth + 1);
        let else_body = if suffix.as_deref() == Some("else {") {
            Some(self.parse_block(close_line, "else", depth + 1).0)
        } else if suffix.is_some() {
            self.diagnostics.push(Diagnostic {
                line: close_line,
                message: "only `} else {` may follow an if closing brace".to_string(),
            });
            None
        } else if self.consume_separate_else_header() {
            let else_line = self.pos;
            Some(self.parse_block(else_line, "else", depth + 1).0)
        } else {
            None
        };
        Some(Statement::If {
            condition,
            then_body,
            else_body,
        })
    }

    fn parse_while(&mut self, line_no: usize, line: &str, depth: usize) -> Option<Statement> {
        let Some(condition_src) = block_header_expression(line, "while") else {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "while expects: while condition {".to_string(),
            });
            return None;
        };
        let condition = parse_value(condition_src, line_no, &mut self.diagnostics);
        let (body, suffix, close_line) = self.parse_block(line_no, "while", depth + 1);
        if let Some(suffix) = suffix {
            self.diagnostics.push(Diagnostic {
                line: close_line,
                message: format!("unexpected text after while closing brace: `{suffix}`"),
            });
        }
        Some(Statement::While { condition, body })
    }

    fn parse_return(&mut self, line_no: usize, line: &str) -> Option<Statement> {
        let rest = line["return".len()..].trim();
        let value = if rest.is_empty() {
            None
        } else {
            Some(parse_value(rest, line_no, &mut self.diagnostics))
        };
        Some(Statement::Return { value })
    }

    fn parse_assignment(&mut self, line_no: usize, line: &str) -> Option<Statement> {
        let index = find_assignment_operator(line)?;
        let name = line[..index].trim();
        let value = line[index + 1..].trim();
        if !is_member_ident(name) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "left side of assignment must be an identifier or member path".to_string(),
            });
            return None;
        }
        if value.is_empty() {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "assignment needs a value".to_string(),
            });
            return None;
        }
        Some(Statement::Assignment {
            name: name.to_string(),
            value: parse_value(value, line_no, &mut self.diagnostics),
        })
    }

    fn parse_block(
        &mut self,
        opening_line: usize,
        kind: &str,
        depth: usize,
    ) -> (Block, Option<String>, usize) {
        let mut block = Block::default();
        if depth >= MAX_BLOCK_NESTING_DEPTH {
            self.diagnostics.push(Diagnostic {
                line: opening_line,
                message: format!(
                    "q0lang block nesting is too deep (maximum {MAX_BLOCK_NESTING_DEPTH})"
                ),
            });
            return (block, None, opening_line);
        }
        while self.pos < self.lines.len() {
            let line_no = self.pos + 1;
            let line = strip_comment(self.lines[self.pos]).trim().to_string();
            if line.is_empty() {
                self.pos += 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix('}') {
                self.pos += 1;
                let suffix = rest.trim();
                return (
                    block,
                    (!suffix.is_empty()).then(|| suffix.to_string()),
                    line_no,
                );
            }
            if let Some(statement) = self.parse_statement(depth) {
                block.statements.push(statement);
                block.statement_lines.push(line_no);
            }
        }
        self.diagnostics.push(Diagnostic {
            line: opening_line,
            message: format!("{kind} block is missing closing }}"),
        });
        (block, None, self.lines.len().max(opening_line))
    }

    fn consume_separate_else_header(&mut self) -> bool {
        let mut scan = self.pos;
        while scan < self.lines.len() && strip_comment(self.lines[scan]).trim().is_empty() {
            scan += 1;
        }
        if scan < self.lines.len() && strip_comment(self.lines[scan]).trim() == "else {" {
            self.pos = scan + 1;
            true
        } else {
            false
        }
    }
}

fn block_header_expression<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?.trim_start();
    let condition = rest.strip_suffix('{')?.trim_end();
    (!condition.is_empty()).then_some(condition)
}

fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == active {
                quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            quote = Some(b);
        } else if b == b'#' {
            return &line[..i];
        }
        i += 1;
    }
    line
}

fn split_words(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

fn parse_value(src: &str, line: usize, diagnostics: &mut Vec<Diagnostic>) -> Value {
    ExprParser::new(src, line, diagnostics).parse()
}

fn split_top_level_commas(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in src.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&src[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&src[start..]);
    out
}

fn find_assignment_operator(src: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0_i32;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == active {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' => quote = Some(b),
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'=' if depth == 0 => {
                let prev = i.checked_sub(1).and_then(|index| bytes.get(index)).copied();
                let next = bytes.get(i + 1).copied();
                if prev != Some(b'=')
                    && prev != Some(b'!')
                    && prev != Some(b'<')
                    && prev != Some(b'>')
                    && next != Some(b'=')
                {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_plain_ident(src: &str) -> bool {
    let mut chars = src.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_member_ident(src: &str) -> bool {
    !src.is_empty() && src.split('.').all(is_plain_ident)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn is_mouse_event(src: &str) -> bool {
    BUILTIN_LIBRARIES
        .iter()
        .find(|library| library.name == "q0.mouse")
        .map(|library| library.symbols.contains(&src))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq)]
enum ExprToken {
    Ident(String),
    Number(String),
    String(String),
    True,
    False,
    Null,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Not,
    LParen,
    RParen,
    Comma,
    End,
}

struct ExprParser<'a, 'd> {
    src: &'a str,
    line: usize,
    diagnostics: &'d mut Vec<Diagnostic>,
    tokens: Vec<ExprToken>,
    pos: usize,
    depth_exceeded: bool,
}

impl<'a, 'd> ExprParser<'a, 'd> {
    fn new(src: &'a str, line: usize, diagnostics: &'d mut Vec<Diagnostic>) -> Self {
        let mut parser = Self {
            src,
            line,
            diagnostics,
            tokens: Vec::new(),
            pos: 0,
            depth_exceeded: false,
        };
        parser.tokens = parser.lex();
        parser
    }

    fn parse(&mut self) -> Value {
        let value = self.parse_bp(0, 0);
        if self.depth_exceeded {
            return Value::Raw(self.src.to_string());
        }
        if !matches!(self.peek(), ExprToken::End) {
            self.diagnostics.push(Diagnostic {
                line: self.line,
                message: "extra junk after expression".to_string(),
            });
            return Value::Raw(self.src.to_string());
        }
        value
    }

    fn parse_bp(&mut self, min_bp: u8, depth: usize) -> Value {
        if depth >= MAX_EXPRESSION_NESTING_DEPTH {
            if !self.depth_exceeded {
                self.diagnostics.push(Diagnostic {
                    line: self.line,
                    message: format!(
                        "expression nesting is too deep (maximum {MAX_EXPRESSION_NESTING_DEPTH})"
                    ),
                });
            }
            self.depth_exceeded = true;
            self.pos = self.tokens.len().saturating_sub(1);
            return Value::Raw(self.src.to_string());
        }

        let mut lhs = match self.next() {
            ExprToken::Number(raw) => Value::Number(raw),
            ExprToken::String(text) => Value::String(text),
            ExprToken::True => Value::Bool(true),
            ExprToken::False => Value::Bool(false),
            ExprToken::Null => Value::Null,
            ExprToken::Ident(name) => {
                if matches!(self.peek(), ExprToken::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), ExprToken::RParen) {
                        loop {
                            args.push(self.parse_bp(0, depth + 1));
                            if self.depth_exceeded {
                                return Value::Raw(self.src.to_string());
                            }
                            if matches!(self.peek(), ExprToken::Comma) {
                                self.next();
                                continue;
                            }
                            break;
                        }
                    }
                    if matches!(self.peek(), ExprToken::RParen) {
                        self.next();
                    } else {
                        self.diagnostics.push(Diagnostic {
                            line: self.line,
                            message: "call is missing closing )".to_string(),
                        });
                    }
                    Value::Call { name, args }
                } else {
                    Value::Ident(name)
                }
            }
            ExprToken::Minus => {
                let value = self.parse_bp(17, depth + 1);
                Value::Unary {
                    op: UnaryOp::Neg,
                    value: Box::new(value),
                }
            }
            ExprToken::Not => {
                let value = self.parse_bp(17, depth + 1);
                Value::Unary {
                    op: UnaryOp::Not,
                    value: Box::new(value),
                }
            }
            ExprToken::LParen => {
                let inner = self.parse_bp(0, depth + 1);
                if matches!(self.peek(), ExprToken::RParen) {
                    self.next();
                } else {
                    self.diagnostics.push(Diagnostic {
                        line: self.line,
                        message: "expression is missing closing )".to_string(),
                    });
                }
                inner
            }
            ExprToken::End => {
                self.diagnostics.push(Diagnostic {
                    line: self.line,
                    message: "expression expected".to_string(),
                });
                Value::Raw(self.src.to_string())
            }
            other => {
                self.diagnostics.push(Diagnostic {
                    line: self.line,
                    message: format!("unexpected expression token `{other:?}`"),
                });
                Value::Raw(self.src.to_string())
            }
        };

        while let Some((left_bp, right_bp, op)) = infix_binding_power(self.peek()) {
            if left_bp < min_bp {
                break;
            }
            self.next();
            let rhs = self.parse_bp(right_bp, depth + 1);
            if self.depth_exceeded {
                return Value::Raw(self.src.to_string());
            }
            lhs = Value::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        lhs
    }

    fn lex(&mut self) -> Vec<ExprToken> {
        let mut tokens = Vec::new();
        let bytes = self.src.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b' ' | b'\t' | b'\r' | b'\n' => i += 1,
                b'+' => {
                    tokens.push(ExprToken::Plus);
                    i += 1;
                }
                b'-' => {
                    tokens.push(ExprToken::Minus);
                    i += 1;
                }
                b'*' => {
                    tokens.push(ExprToken::Star);
                    i += 1;
                }
                b'/' => {
                    tokens.push(ExprToken::Slash);
                    i += 1;
                }
                b'%' => {
                    tokens.push(ExprToken::Percent);
                    i += 1;
                }
                b'^' => {
                    tokens.push(ExprToken::Caret);
                    i += 1;
                }
                b'=' if bytes.get(i + 1) == Some(&b'=') => {
                    tokens.push(ExprToken::EqualEqual);
                    i += 2;
                }
                b'!' if bytes.get(i + 1) == Some(&b'=') => {
                    tokens.push(ExprToken::BangEqual);
                    i += 2;
                }
                b'<' if bytes.get(i + 1) == Some(&b'=') => {
                    tokens.push(ExprToken::LessEqual);
                    i += 2;
                }
                b'>' if bytes.get(i + 1) == Some(&b'=') => {
                    tokens.push(ExprToken::GreaterEqual);
                    i += 2;
                }
                b'<' => {
                    tokens.push(ExprToken::Less);
                    i += 1;
                }
                b'>' => {
                    tokens.push(ExprToken::Greater);
                    i += 1;
                }
                b'(' => {
                    tokens.push(ExprToken::LParen);
                    i += 1;
                }
                b')' => {
                    tokens.push(ExprToken::RParen);
                    i += 1;
                }
                b',' => {
                    tokens.push(ExprToken::Comma);
                    i += 1;
                }
                b'"' | b'\'' => {
                    let quote = bytes[i];
                    let (text, end) = self.lex_string(i + 1, quote);
                    tokens.push(ExprToken::String(text));
                    i = end;
                }
                b'.' if i + 3 < bytes.len()
                    && bytes[i + 1] == b'.'
                    && bytes[i + 2] == b'.'
                    && bytes[i + 3] == b'"' =>
                {
                    self.diagnostics.push(Diagnostic {
                        line: self.line,
                        message: "legacy ...\"text\" string syntax is dead; use \"text\""
                            .to_string(),
                    });
                    let (text, end) = self.lex_string(i + 4, b'"');
                    tokens.push(ExprToken::String(text));
                    i = end;
                }
                b'0'..=b'9' => {
                    let start = i;
                    let mut seen_dot = false;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'0'..=b'9' => i += 1,
                            b'.' if !seen_dot => {
                                seen_dot = true;
                                i += 1;
                            }
                            _ => break,
                        }
                    }
                    tokens.push(ExprToken::Number(self.src[start..i].to_string()));
                }
                byte if is_ident_start(byte) => {
                    let start = i;
                    i += 1;
                    while i < bytes.len() && is_ident_continue(bytes[i]) {
                        i += 1;
                    }
                    let word = &self.src[start..i];
                    tokens.push(match word {
                        "true" => ExprToken::True,
                        "false" => ExprToken::False,
                        "null" => ExprToken::Null,
                        "and" => ExprToken::And,
                        "or" => ExprToken::Or,
                        "not" => ExprToken::Not,
                        _ => ExprToken::Ident(word.to_string()),
                    });
                }
                _ => {
                    let ch = self.src[i..].chars().next().unwrap_or('?');
                    self.diagnostics.push(Diagnostic {
                        line: self.line,
                        message: format!("unknown expression character `{ch}`"),
                    });
                    i += ch.len_utf8();
                }
            }
        }
        tokens.push(ExprToken::End);
        tokens
    }

    fn lex_string(&mut self, mut i: usize, quote: u8) -> (String, usize) {
        let bytes = self.src.as_bytes();
        let mut out = String::new();
        let mut closed = false;
        while i < bytes.len() {
            if bytes[i] == quote {
                i += 1;
                closed = true;
                break;
            }
            if bytes[i] == b'\\' {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                let escaped = match bytes[i] {
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'\\' => '\\',
                    b'"' => '"',
                    b'\'' => '\'',
                    other => other as char,
                };
                out.push(escaped);
                i += 1;
                continue;
            }
            let ch = self.src[i..].chars().next().unwrap_or('?');
            out.push(ch);
            i += ch.len_utf8();
        }
        if !closed {
            self.diagnostics.push(Diagnostic {
                line: self.line,
                message: "string is missing closing quote".to_string(),
            });
        }
        (out, i)
    }

    fn peek(&self) -> &ExprToken {
        self.tokens.get(self.pos).unwrap_or(&ExprToken::End)
    }

    fn next(&mut self) -> ExprToken {
        let token = self.tokens.get(self.pos).cloned().unwrap_or(ExprToken::End);
        self.pos += 1;
        token
    }
}

fn infix_binding_power(token: &ExprToken) -> Option<(u8, u8, BinaryOp)> {
    match token {
        ExprToken::Or => Some((1, 2, BinaryOp::Or)),
        ExprToken::And => Some((3, 4, BinaryOp::And)),
        ExprToken::EqualEqual => Some((5, 6, BinaryOp::Equal)),
        ExprToken::BangEqual => Some((5, 6, BinaryOp::NotEqual)),
        ExprToken::Less => Some((7, 8, BinaryOp::Less)),
        ExprToken::LessEqual => Some((7, 8, BinaryOp::LessEqual)),
        ExprToken::Greater => Some((7, 8, BinaryOp::Greater)),
        ExprToken::GreaterEqual => Some((7, 8, BinaryOp::GreaterEqual)),
        ExprToken::Plus => Some((9, 10, BinaryOp::Add)),
        ExprToken::Minus => Some((9, 10, BinaryOp::Sub)),
        ExprToken::Star => Some((11, 12, BinaryOp::Mul)),
        ExprToken::Slash => Some((11, 12, BinaryOp::Div)),
        ExprToken::Percent => Some((11, 12, BinaryOp::Rem)),
        ExprToken::Caret => Some((15, 14, BinaryOp::Pow)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_shape() {
        let program = parse(
            r#"import q0.mouse
X = "hello"
gorun! 12
gostop! label
listen mouse.click onClick
xlisten mouse.click onClick
pb func openDoor actor speed
pr func cacheFrame frame
do! {
  Y = 1
}
"#,
        );
        assert_eq!(program.diagnostics, Vec::new());
        assert_eq!(program.statements.len(), 9);
        assert!(matches!(
            program.statements[1],
            Statement::Assignment {
                value: Value::String(_),
                ..
            }
        ));
        assert!(matches!(
            program.statements[8],
            Statement::DoBlock { ref body } if body.statements.len() == 1
        ));
    }

    #[test]
    fn reports_missing_signal_bang_and_legacy_string() {
        let program = parse(
            r#"X = ..."old"
gorun 2
do {
}
"#,
        );
        assert_eq!(program.diagnostics.len(), 3);
        assert!(program.diagnostics[0]
            .message
            .contains("legacy ...\"text\" string syntax is dead"));
        assert!(program.diagnostics[1].message.contains("must end with !"));
        assert!(program.diagnostics[2].message.contains("do!"));
    }

    #[test]
    fn parses_math_expressions_and_calls() {
        let program = parse(
            r#"import q0.math
x = sin(pi / 2) + 3 * -2
gorun! 1 + 1
"#,
        );
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            program.statements[1],
            Statement::Assignment {
                value: Value::Binary { .. },
                ..
            }
        ));
        assert!(matches!(
            program.statements[2],
            Statement::TimelineSignal {
                target: Value::Binary { .. },
                ..
            }
        ));
    }

    #[test]
    fn parses_q0shell_commands_with_expression_args() {
        let program = parse(
            r#"q0shell.write! 'out.txt', 'frame=' + round(1.5)
q0shell.move! 'out.txt', 'done.txt'
"#,
        );
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            program.statements[0],
            Statement::ShellCommand {
                ref command,
                ref args
            } if command == "write" && args.len() == 2
        ));
    }

    #[test]
    fn accepts_expression_just_below_nesting_budget() {
        let nesting = MAX_EXPRESSION_NESTING_DEPTH - 1;
        let source = format!("x = {}1{}", "(".repeat(nesting), ")".repeat(nesting));
        let program = parse(&source);
        assert_eq!(program.diagnostics, Vec::new());
    }

    #[test]
    fn reports_expression_nesting_budget_instead_of_recursing_unbounded() {
        let nesting = MAX_EXPRESSION_NESTING_DEPTH + 1024;
        let source = format!("x = {}1{}", "(".repeat(nesting), ")".repeat(nesting));
        let program = parse(&source);
        let depth_diagnostics = program
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .message
                    .contains("expression nesting is too deep")
            })
            .count();
        assert_eq!(depth_diagnostics, 1);
        assert!(matches!(
            program.statements.first(),
            Some(Statement::Assignment {
                value: Value::Raw(_),
                ..
            })
        ));
    }

    #[test]
    fn parses_q0scene_switch_command() {
        let program = parse("import q0.scene\nq0scene.switch! \"room_2\"\n");
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            program.statements[1],
            Statement::SceneCommand { ref command, ref args }
                if command == "switch" && args == &[Value::String("room_2".into())]
        ));
    }

    #[test]
    fn parses_q0rig_runtime_control_commands() {
        let program = parse(
            r#"import q0.rig
q0rig.position! "look", 12 + 3, 20
q0rig.value! "arm fk ik", 0.75
q0rig.reset! "look"
q0rig.pose! "wave", 0.6
q0rig.pose_reset! "wave"
"#,
        );
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            program.statements[1],
            Statement::RigCommand { ref command, ref args }
                if command == "position" && args.len() == 3
        ));
        assert!(matches!(
            program.statements[5],
            Statement::RigCommand { ref command, ref args }
                if command == "pose_reset" && args.len() == 1
        ));
    }

    #[test]
    fn parses_real_nested_blocks_bool_logic_and_function_body() {
        let program = parse(
            r#"pb func choose x limit {
  if x >= limit and not false {
    return x
  } else {
    while x < limit {
      x = x + 1
    }
    return x
  }
}
result = choose(1, 3)
"#,
        );
        assert_eq!(program.diagnostics, Vec::new(), "{:?}", program.diagnostics);
        assert!(matches!(
            program.statements[0],
            Statement::Function {
                body: Some(ref body),
                ..
            } if matches!(body.statements.first(), Some(Statement::If { .. }))
        ));
        assert!(matches!(
            program.statements[1],
            Statement::Assignment {
                value: Value::Call { .. },
                ..
            }
        ));
    }

    #[test]
    fn hash_inside_string_is_not_a_comment() {
        let program = parse("x = \"#still text\" # comment\n");
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            &program.statements[0],
            Statement::Assignment {
                value: Value::String(value),
                ..
            } if value == "#still text"
        ));
    }

    #[test]
    fn separate_else_line_is_supported() {
        let program = parse("if true {\n  x = 1\n}\nelse {\n  x = 2\n}\n");
        assert_eq!(program.diagnostics, Vec::new());
        assert!(matches!(
            &program.statements[0],
            Statement::If {
                else_body: Some(body),
                ..
            } if body.statements.len() == 1
        ));
    }
}
