//! Writes a parsed q0lang program back out as text, the way a person would
//! write it: two-space indents, one statement a line, and parentheses only
//! where the meaning needs them. `parse(print(program))` gives the same
//! statements back.

use super::parser::{BinaryOp, Block, Statement, TimelineSignal, UnaryOp, Value, Visibility};

/// The text of `statements`, and the line each statement starts on, in the
/// order a walk visits them: a statement, then what is inside it.
pub fn print_program(statements: &[Statement]) -> (String, Vec<usize>) {
    let mut printer = Printer::default();
    for (index, statement) in statements.iter().enumerate() {
        // Functions get room around them, like paragraphs.
        let function = matches!(statement, Statement::Function { body: Some(_), .. });
        let after_function = index > 0
            && matches!(
                statements[index - 1],
                Statement::Function { body: Some(_), .. }
            );
        if index > 0 && (function || after_function) {
            printer.lines.push(String::new());
        }
        printer.statement(statement, 0);
    }
    let mut text = printer.lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    (text, printer.starts)
}

pub fn print_statements(statements: &[Statement]) -> String {
    print_program(statements).0
}

#[derive(Default)]
struct Printer {
    lines: Vec<String>,
    starts: Vec<usize>,
}

impl Printer {
    fn line(&mut self, depth: usize, text: String) {
        self.lines.push(format!("{}{}", "  ".repeat(depth), text));
    }

    fn block(&mut self, block: &Block, depth: usize) {
        for statement in &block.statements {
            self.statement(statement, depth);
        }
    }

    fn statement(&mut self, statement: &Statement, depth: usize) {
        self.starts.push(self.lines.len() + 1);
        match statement {
            Statement::Assignment { name, value } => {
                self.line(depth, format!("{name} = {}", print_value(value)))
            }
            Statement::Expression { value } => self.line(depth, print_value(value)),
            Statement::TimelineSignal { kind, target } => {
                let word = match kind {
                    TimelineSignal::GoRun => "gorun!",
                    TimelineSignal::GoStop => "gostop!",
                };
                self.line(depth, format!("{word} {}", print_value(target)));
            }
            Statement::ShellCommand { command, args } => {
                self.line(depth, command_text("q0shell", command, args))
            }
            Statement::RigCommand { command, args } => {
                self.line(depth, command_text("q0rig", command, args))
            }
            Statement::SceneCommand { command, args } => {
                self.line(depth, command_text("q0scene", command, args))
            }
            Statement::PhysicsCommand { command, args } => {
                self.line(depth, command_text("q0physics", command, args))
            }
            Statement::ParticlesCommand { command, args } => {
                self.line(depth, command_text("q0particles", command, args))
            }
            Statement::LightCommand { command, args } => {
                self.line(depth, command_text("q0light", command, args))
            }
            Statement::LayerCommand { command, args } => {
                self.line(depth, command_text("q0layer", command, args))
            }
            Statement::WindowCommand { command, args } => {
                self.line(depth, command_text("q0window", command, args))
            }
            Statement::Listen { event, handler } => {
                self.line(depth, format!("listen {event} {handler}"))
            }
            Statement::Unlisten { event, handler } => {
                self.line(depth, format!("xlisten {event} {handler}"))
            }
            Statement::Function {
                visibility,
                name,
                params,
                body,
            } => {
                let mut header = format!(
                    "{} func {name}",
                    match visibility {
                        Visibility::Public => "pb",
                        Visibility::Private => "pr",
                    }
                );
                for param in params {
                    header.push(' ');
                    header.push_str(param);
                }
                match body {
                    Some(body) => {
                        self.line(depth, format!("{header} {{"));
                        self.block(body, depth + 1);
                        self.line(depth, "}".into());
                    }
                    None => self.line(depth, header),
                }
            }
            Statement::DoBlock { body } => {
                self.line(depth, "do! {".into());
                self.block(body, depth + 1);
                self.line(depth, "}".into());
            }
            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                self.line(depth, format!("if {} {{", print_value(condition)));
                self.block(then_body, depth + 1);
                if let Some(else_body) = else_body {
                    self.line(depth, "} else {".into());
                    self.block(else_body, depth + 1);
                }
                self.line(depth, "}".into());
            }
            Statement::While { condition, body } => {
                self.line(depth, format!("while {} {{", print_value(condition)));
                self.block(body, depth + 1);
                self.line(depth, "}".into());
            }
            Statement::ForRange {
                name,
                start,
                stop,
                step,
                body,
            } => {
                let zero = matches!(start, Value::Number(n) if n == "0");
                let one = matches!(step, Value::Number(n) if n == "1");
                let range = match (zero, one) {
                    (true, true) => print_value(stop),
                    (_, true) => format!("{}, {}", print_value(start), print_value(stop)),
                    _ => format!(
                        "{}, {}, {}",
                        print_value(start),
                        print_value(stop),
                        print_value(step)
                    ),
                };
                self.line(depth, format!("for {name} in range({range}) {{"));
                self.block(body, depth + 1);
                self.line(depth, "}".into());
            }
            Statement::Break => self.line(depth, "break".into()),
            Statement::Continue => self.line(depth, "continue".into()),
            Statement::Return { value } => self.line(
                depth,
                match value {
                    Some(value) => format!("return {}", print_value(value)),
                    None => "return".into(),
                },
            ),
            Statement::Import { library } => self.line(depth, format!("import {library}")),
            Statement::Unknown { text } => self.line(depth, text.trim().to_string()),
        }
    }
}

fn command_text(library: &str, command: &str, args: &[Value]) -> String {
    let mut text = format!("{library}.{command}!");
    if !args.is_empty() {
        text.push(' ');
        text.push_str(&args.iter().map(print_value).collect::<Vec<_>>().join(", "));
    }
    text
}

/// How tightly an operator holds its left and right values; the parser's
/// own numbers.
fn binding(op: BinaryOp) -> (u8, u8) {
    match op {
        BinaryOp::Or => (1, 2),
        BinaryOp::And => (3, 4),
        BinaryOp::Equal | BinaryOp::NotEqual => (5, 6),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => (7, 8),
        BinaryOp::Add | BinaryOp::Sub => (9, 10),
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => (11, 12),
        BinaryOp::Pow => (15, 14),
    }
}

pub fn operator_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Pow => "^",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

pub fn print_value(value: &Value) -> String {
    match value {
        Value::Ident(name) => name.clone(),
        Value::Number(raw) => raw.clone(),
        Value::String(text) => quote(text),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".into(),
        Value::Raw(raw) => raw.clone(),
        Value::Unary { op, value } => {
            let inner = match value.as_ref() {
                Value::Binary { .. } => format!("({})", print_value(value)),
                _ => print_value(value),
            };
            match op {
                UnaryOp::Neg => format!("-{inner}"),
                UnaryOp::Not => format!("not {inner}"),
            }
        }
        Value::Binary { op, left, right } => {
            let (left_power, right_power) = binding(*op);
            let left_text = match left.as_ref() {
                Value::Binary { op: inner, .. } if binding(*inner).1 <= left_power => {
                    format!("({})", print_value(left))
                }
                _ => print_value(left),
            };
            let right_text = match right.as_ref() {
                Value::Binary { op: inner, .. } if binding(*inner).0 < right_power => {
                    format!("({})", print_value(right))
                }
                _ => print_value(right),
            };
            format!("{left_text} {} {right_text}", operator_text(*op))
        }
        Value::Call { name, args } => format!(
            "{name}({})",
            args.iter().map(print_value).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// A string literal that reads back as `text`.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse;
    use super::*;

    /// Statements as they read, without the lines they were read from.
    fn lineless(statements: &[Statement]) -> Vec<Statement> {
        fn block(block: &Block) -> Block {
            Block {
                statements: lineless(&block.statements),
                statement_lines: Vec::new(),
            }
        }
        statements
            .iter()
            .map(|statement| match statement {
                Statement::Function {
                    visibility,
                    name,
                    params,
                    body,
                } => Statement::Function {
                    visibility: *visibility,
                    name: name.clone(),
                    params: params.clone(),
                    body: body.as_ref().map(block),
                },
                Statement::DoBlock { body } => Statement::DoBlock { body: block(body) },
                Statement::If {
                    condition,
                    then_body,
                    else_body,
                } => Statement::If {
                    condition: condition.clone(),
                    then_body: block(then_body),
                    else_body: else_body.as_ref().map(block),
                },
                Statement::While { condition, body } => Statement::While {
                    condition: condition.clone(),
                    body: block(body),
                },
                Statement::ForRange {
                    name,
                    start,
                    stop,
                    step,
                    body,
                } => Statement::ForRange {
                    name: name.clone(),
                    start: start.clone(),
                    stop: stop.clone(),
                    step: step.clone(),
                    body: block(body),
                },
                other => other.clone(),
            })
            .collect()
    }

    fn same_meaning(source: &str) {
        let first = parse(source);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let (printed, starts) = print_program(&first.statements);
        let second = parse(&printed);
        assert!(
            second.diagnostics.is_empty(),
            "{printed}\n{:?}",
            second.diagnostics
        );
        assert_eq!(
            lineless(&first.statements),
            lineless(&second.statements),
            "{printed}"
        );
        assert_eq!(print_program(&second.statements).0, printed);
        assert_eq!(
            starts.first().copied(),
            (!first.statements.is_empty()).then_some(1)
        );
    }

    #[test]
    fn programs_read_back_as_they_were() {
        same_meaning(
            r#"import q0.math
import q0.layer
x = (1 + 2) * 3 - (4 - 5) - -6
y = 2 ^ 3 ^ 2
z = (2 ^ 3) ^ 2
w = -(2 ^ 2)
q = not (a and b) or c == (d != e)
s = "say \"hi\"\n\\ok"
pb func hits ox oy {
  if q0layer.touches("bones", "soul") and not dead {
    return true
  } else {
    if ox < 0 {
      return false
    }
  }
  for i in range(1, 10, 2) {
    total = total + i
  }
  for j in range(3) {
    continue
  }
  while false {
    break
  }
  return null
}
pr func quiet
listen mouse.click hits
xlisten mouse.click hits
do! {
  gostop! 12
  gorun! "intro"
}
q0layer.solid! "walls", "soul"
q0window.center!
q0particles.burst! "boom", 20 + 4
"#,
        );
    }

    #[test]
    fn statements_say_where_they_start() {
        let program = parse("a = 1\nif a {\n  b = 2\n} else {\n  c = 3\n}\nd = 4\n");
        let (text, starts) = print_program(&program.statements);
        assert_eq!(text.lines().count(), 7);
        assert_eq!(starts, vec![1, 2, 3, 5, 7]);
    }
}
