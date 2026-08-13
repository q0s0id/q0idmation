//! q0lang v0 parser shared by editor and player.
//!
//! This is intentionally small and line-oriented: enough structure for the
//! editor to validate the language shape and for the future runtime/compiler
//! to grow from stable AST names instead of raw strings.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Assignment {
        name: String,
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
    },
    DoBlock {
        body: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Ident(String),
    Number(String),
    String(String),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

/// Maximum number of recursive Pratt-parser calls allowed for one expression.
/// This bounds parentheses, unary operators, nested calls, and right-associative
/// operators parsed from untrusted scripts embedded in movie files.
pub const MAX_EXPRESSION_NESTING_DEPTH: usize = 64;

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
        symbols: &["do!", "listen", "xlisten", "pb", "pr", "func"],
    },
];

pub fn parse(src: &str) -> Program {
    let mut p = Parser {
        lines: src.lines().enumerate().peekable(),
        statements: Vec::new(),
        diagnostics: Vec::new(),
    };
    p.parse_all();
    Program {
        statements: p.statements,
        diagnostics: p.diagnostics,
    }
}

struct Parser<'a> {
    lines: std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'a>>>,
    statements: Vec<Statement>,
    diagnostics: Vec<Diagnostic>,
}

impl Parser<'_> {
    fn parse_all(&mut self) {
        while let Some((idx, raw)) = self.lines.next() {
            let line_no = idx + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if line == "}" {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "unexpected closing brace".to_string(),
                });
                continue;
            }
            if line.starts_with("do") {
                self.parse_do(line_no, line);
                continue;
            }
            self.parse_line(line_no, line);
        }
    }

    fn parse_line(&mut self, line_no: usize, line: &str) {
        let parts = split_words(line);
        let Some(first) = parts.first().copied() else {
            return;
        };
        match first {
            "import" => self.parse_import(line_no, &parts),
            "gorun!" => self.parse_timeline(line_no, TimelineSignal::GoRun, line, &parts),
            "gostop!" => self.parse_timeline(line_no, TimelineSignal::GoStop, line, &parts),
            "gorun" | "gostop" => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: format!("execution signal `{first}` must end with !"),
                });
                self.statements.push(Statement::Unknown {
                    text: line.to_string(),
                });
            }
            "listen" => self.parse_listen(line_no, &parts, false),
            "xlisten" => self.parse_listen(line_no, &parts, true),
            "pb" => self.parse_function(line_no, &parts, Visibility::Public),
            "pr" => self.parse_function(line_no, &parts, Visibility::Private),
            _ if first.starts_with("q0shell.") && first.ends_with('!') => {
                self.parse_shell_command(line_no, line, first)
            }
            _ if first.starts_with("q0rig.") && first.ends_with('!') => {
                self.parse_rig_command(line_no, line, first)
            }
            _ if line.contains('=') => self.parse_assignment(line_no, line),
            _ if first.ends_with('!') => self.statements.push(Statement::Unknown {
                text: line.to_string(),
            }),
            _ => {
                self.diagnostics.push(Diagnostic {
                    line: line_no,
                    message: "unknown q0lang statement".to_string(),
                });
                self.statements.push(Statement::Unknown {
                    text: line.to_string(),
                });
            }
        }
    }

    fn parse_import(&mut self, line_no: usize, parts: &[&str]) {
        match parts {
            ["import", lib] => {
                if !BUILTIN_LIBRARIES.iter().any(|b| b.name == *lib) {
                    self.diagnostics.push(Diagnostic {
                        line: line_no,
                        message: format!("unknown library `{lib}`"),
                    });
                }
                self.statements.push(Statement::Import {
                    library: (*lib).to_string(),
                });
            }
            _ => self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "import expects one library name".to_string(),
            }),
        }
    }

    fn parse_timeline(&mut self, line_no: usize, kind: TimelineSignal, line: &str, parts: &[&str]) {
        if parts.len() < 2 {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "timeline signal expects a target".to_string(),
            });
            return;
        }
        let target_src = line[parts[0].len()..].trim();
        self.statements.push(Statement::TimelineSignal {
            kind,
            target: parse_value(target_src, line_no, &mut self.diagnostics),
        });
    }

    fn parse_listen(&mut self, line_no: usize, parts: &[&str], remove: bool) {
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
            return;
        }
        let event = parts[1].to_string();
        if !is_mouse_event(&event) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: format!("event `{event}` is not in q0.mouse built-ins yet"),
            });
        }
        if remove {
            self.statements.push(Statement::Unlisten {
                event,
                handler: parts[2].to_string(),
            });
        } else {
            self.statements.push(Statement::Listen {
                event,
                handler: parts[2].to_string(),
            });
        }
    }

    fn parse_shell_command(&mut self, line_no: usize, line: &str, first: &str) {
        let command = first
            .trim_start_matches("q0shell.")
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
        self.statements
            .push(Statement::ShellCommand { command, args });
    }

    fn parse_rig_command(&mut self, line_no: usize, line: &str, first: &str) {
        let command = first
            .trim_start_matches("q0rig.")
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
        self.statements
            .push(Statement::RigCommand { command, args });
    }

    fn parse_function(&mut self, line_no: usize, parts: &[&str], visibility: Visibility) {
        if parts.len() < 3 || parts[1] != "func" {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "function expects: pb func name [params...] or pr func name [params...]"
                    .to_string(),
            });
            return;
        }
        self.statements.push(Statement::Function {
            visibility,
            name: parts[2].to_string(),
            params: parts[3..].iter().map(|s| (*s).to_string()).collect(),
        });
    }

    fn parse_assignment(&mut self, line_no: usize, line: &str) {
        let Some((name, value)) = line.split_once('=') else {
            return;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || !is_ident(name) {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "left side of assignment must be an identifier".to_string(),
            });
            return;
        }
        if value.is_empty() {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "assignment needs a value".to_string(),
            });
            return;
        }
        self.statements.push(Statement::Assignment {
            name: name.to_string(),
            value: parse_value(value, line_no, &mut self.diagnostics),
        });
    }

    fn parse_do(&mut self, line_no: usize, line: &str) {
        if !line.starts_with("do!") {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "execution block `do` must be written as do!".to_string(),
            });
            if line.contains('{') {
                for (_idx, raw) in self.lines.by_ref() {
                    if strip_comment(raw).trim() == "}" {
                        break;
                    }
                }
            }
            return;
        }
        if !line.contains('{') {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "do! expects an opening {".to_string(),
            });
            return;
        }
        let mut body = String::new();
        let mut closed = false;
        for (idx, raw) in self.lines.by_ref() {
            let trimmed = strip_comment(raw).trim();
            if trimmed == "}" {
                closed = true;
                break;
            }
            body.push_str(raw);
            body.push('\n');
            if trimmed.contains('}') {
                self.diagnostics.push(Diagnostic {
                    line: idx + 1,
                    message: "closing brace must be alone on its line".to_string(),
                });
            }
        }
        if !closed {
            self.diagnostics.push(Diagnostic {
                line: line_no,
                message: "do! block is missing closing }".to_string(),
            });
        }
        self.statements.push(Statement::DoBlock { body });
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
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
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in src.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
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

fn is_ident(src: &str) -> bool {
    let mut chars = src.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
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
        .find(|b| b.name == "q0.mouse")
        .map(|b| b.symbols.contains(&src))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq)]
enum ExprToken {
    Ident(String),
    Number(String),
    String(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
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
                    message: "expression nesting is too deep (maximum 64)".to_string(),
                });
            }
            self.depth_exceeded = true;
            self.pos = self.tokens.len().saturating_sub(1);
            return Value::Raw(self.src.to_string());
        }

        let mut lhs = match self.next() {
            ExprToken::Number(raw) => Value::Number(raw),
            ExprToken::String(text) => Value::String(text),
            ExprToken::Ident(name) => {
                if matches!(self.peek(), ExprToken::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), ExprToken::RParen) {
                        loop {
                            let argument = self.parse_bp(0, depth + 1);
                            if self.depth_exceeded {
                                return Value::Raw(self.src.to_string());
                            }
                            args.push(argument);
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
                let rhs = self.parse_bp(9, depth + 1);
                if self.depth_exceeded {
                    return Value::Raw(self.src.to_string());
                }
                Value::Unary {
                    op: UnaryOp::Neg,
                    value: Box::new(rhs),
                }
            }
            ExprToken::LParen => {
                let inner = self.parse_bp(0, depth + 1);
                if self.depth_exceeded {
                    return Value::Raw(self.src.to_string());
                }
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

        while let Some((l_bp, r_bp, op)) = infix_binding_power(self.peek()) {
            if l_bp < min_bp {
                break;
            }
            self.next();
            let rhs = self.parse_bp(r_bp, depth + 1);
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
                b' ' | b'\t' => i += 1,
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
                b'"' => {
                    let (text, end) = self.lex_string(i + 1, b'"');
                    tokens.push(ExprToken::String(text));
                    i = end;
                }
                b'\'' => {
                    let (text, end) = self.lex_string(i + 1, b'\'');
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
                b if is_ident_start(b) => {
                    let start = i;
                    i += 1;
                    while i < bytes.len() && is_ident_continue(bytes[i]) {
                        i += 1;
                    }
                    tokens.push(ExprToken::Ident(self.src[start..i].to_string()));
                }
                _ => {
                    self.diagnostics.push(Diagnostic {
                        line: self.line,
                        message: format!(
                            "unknown expression byte `{}`",
                            self.src[i..].chars().next().unwrap_or('?')
                        ),
                    });
                    i += 1;
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
            match bytes[i] {
                b'\\' if i + 1 < bytes.len() => {
                    let escaped = match bytes[i + 1] {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'"' => '"',
                        b'\'' => '\'',
                        b'\\' => '\\',
                        other => other as char,
                    };
                    out.push(escaped);
                    i += 2;
                }
                b if b == quote => {
                    closed = true;
                    i += 1;
                    break;
                }
                b => {
                    out.push(b as char);
                    i += 1;
                }
            }
        }
        if !closed {
            self.diagnostics.push(Diagnostic {
                line: self.line,
                message: "string literal is missing closing quote".to_string(),
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
        ExprToken::Plus => Some((1, 2, BinaryOp::Add)),
        ExprToken::Minus => Some((1, 2, BinaryOp::Sub)),
        ExprToken::Star => Some((3, 4, BinaryOp::Mul)),
        ExprToken::Slash => Some((3, 4, BinaryOp::Div)),
        ExprToken::Percent => Some((3, 4, BinaryOp::Rem)),
        ExprToken::Caret => Some((8, 7, BinaryOp::Pow)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_q0lang_base_shape() {
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
            program.statements[2],
            Statement::RigCommand { ref command, ref args }
                if command == "value" && args.len() == 2
        ));
        assert!(matches!(
            program.statements[3],
            Statement::RigCommand { ref command, ref args }
                if command == "reset" && args.len() == 1
        ));
        assert!(matches!(
            program.statements[4],
            Statement::RigCommand { ref command, ref args }
                if command == "pose" && args.len() == 2
        ));
        assert!(matches!(
            program.statements[5],
            Statement::RigCommand { ref command, ref args }
                if command == "pose_reset" && args.len() == 1
        ));
    }
}
