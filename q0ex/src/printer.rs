//! A [`Program`] back to text, the way the block editor writes it.
//! Parsing what this prints gives the same program again.

use crate::ast::*;

pub fn print(program: &Program) -> String {
    let mut out = Printer::default();
    let mut previous: Option<&ItemKind> = None;
    for item in &program.items {
        // Comments stay glued to what follows; other items get some air.
        let comment = matches!(
            &item.kind,
            ItemKind::Stmt(Stmt {
                kind: StmtKind::Comment(_),
                ..
            })
        );
        let after_comment = matches!(
            previous,
            Some(ItemKind::Stmt(Stmt {
                kind: StmtKind::Comment(_),
                ..
            }))
        );
        let simple = matches!(&item.kind, ItemKind::Stmt(_) | ItemKind::Keep { .. });
        let previous_simple = matches!(previous, Some(ItemKind::Stmt(_) | ItemKind::Keep { .. }));
        if previous.is_some() && !after_comment && !(simple && previous_simple && !comment) {
            out.text.push('\n');
        }
        if let Some((x, y)) = item.pos {
            out.line(&format!("#@ {} {}", number(x as f64), number(y as f64)));
        }
        out.item(&item.kind);
        previous = Some(&item.kind);
    }
    out.text
}

pub fn print_expr(expr: &Expr) -> String {
    let mut out = Printer::default();
    out.expr(expr, 0)
}

pub fn print_stmt(stmt: &Stmt) -> String {
    let mut out = Printer::default();
    out.stmt(stmt);
    out.text.trim_end().to_string()
}

/// A number as q0ex writes it: no `.0` on whole numbers.
pub fn number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else if value.is_nan() {
        "(0 / 0)".into()
    } else if value.is_infinite() {
        if value > 0.0 { "(1 / 0)" } else { "(-1 / 0)" }.into()
    } else {
        let text = format!("{value}");
        // Rust writes tiny numbers with an exponent q0ex cannot read.
        if text.contains('e') {
            format!("{value:.12}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        } else {
            text
        }
    }
}

pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Default)]
struct Printer {
    text: String,
    indent: usize,
}

impl Printer {
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.text.push_str("  ");
        }
        self.text.push_str(text);
        self.text.push('\n');
    }

    fn item(&mut self, kind: &ItemKind) {
        match kind {
            ItemKind::Plugin { name, fields } => {
                self.line(&format!("plugin {} {{", quote(name)));
                self.indent += 1;
                for (key, value) in fields {
                    let value = self.expr(value, 0);
                    self.line(&format!("{key} = {value}"));
                }
                self.indent -= 1;
                self.line("}");
            }
            ItemKind::Settings(decls) => {
                self.line("settings {");
                self.indent += 1;
                for decl in decls {
                    let text = self.setting(decl);
                    self.line(&text);
                }
                self.indent -= 1;
                self.line("}");
            }
            ItemKind::Page(text) => {
                self.line("page ~~~");
                for line in text.split('\n') {
                    self.text.push_str(line);
                    self.text.push('\n');
                }
                self.line("~~~");
            }
            ItemKind::Blob { name, kind, data } => {
                self.line(&format!("blob {name} {kind} ~~~"));
                for chunk in data.as_bytes().chunks(76) {
                    self.text
                        .push_str(std::str::from_utf8(chunk).unwrap_or_default());
                    self.text.push('\n');
                }
                self.line("~~~");
            }
            ItemKind::When(when) => {
                let mut head = format!("when {}", when.event);
                if let Some(arg) = &when.arg {
                    head.push(' ');
                    head.push_str(&quote(arg));
                }
                if !when.params.is_empty() {
                    head.push(' ');
                    head.push_str(&when.params.join(", "));
                }
                self.block_stmt(&head, &when.body);
            }
            ItemKind::Fn(def) => self.fn_def(def),
            ItemKind::Keep { name, value } => {
                let value = self.expr(value, 0);
                self.line(&format!("keep {name} = {value}"));
            }
            ItemKind::Stmt(stmt) => self.stmt(stmt),
        }
    }

    fn setting(&mut self, decl: &SettingDecl) -> String {
        let mut text = decl.kind.word().to_string();
        if decl.kind.has_key() {
            text.push(' ');
            text.push_str(&decl.key);
        }
        text.push(' ');
        text.push_str(&quote(&decl.label));
        if let Some(default) = &decl.default {
            text.push_str(" = ");
            text.push_str(&self.expr(default, 0));
        }
        if let (Some(min), Some(max)) = (decl.min, decl.max) {
            text.push_str(&format!(" from {} to {}", number(min), number(max)));
        }
        if let Some(step) = decl.step {
            text.push_str(&format!(" step {}", number(step)));
        }
        if !decl.options.is_empty() {
            let options = decl
                .options
                .iter()
                .map(|option| self.expr(option, 0))
                .collect::<Vec<_>>();
            text.push_str(" of ");
            text.push_str(&options.join(", "));
        }
        if let Some(help) = &decl.help {
            text.push_str(" ? ");
            text.push_str(&quote(help));
        }
        text
    }

    fn fn_def(&mut self, def: &FnDef) {
        let head = format!("fn {}({})", def.name, def.params.join(", "));
        self.block_stmt(&head, &def.body);
    }

    /// `head {`, the body, `}`.
    fn block_stmt(&mut self, head: &str, body: &[Stmt]) {
        self.line(&format!("{head} {{"));
        self.body(body);
        self.line("}");
    }

    fn body(&mut self, body: &[Stmt]) {
        self.indent += 1;
        for stmt in body {
            self.stmt(stmt);
        }
        self.indent -= 1;
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                let text = self.expr(expr, 0);
                self.line(&text);
            }
            StmtKind::Assign { target, op, value } => {
                let target = self.expr(target, 0);
                let value = self.expr(value, 0);
                self.line(&format!("{target} {} {value}", op.text()));
            }
            StmtKind::Swap(a, b) => {
                let a = self.expr(a, 0);
                let b = self.expr(b, 0);
                self.line(&format!("{a} <-> {b}"));
            }
            StmtKind::If {
                branches,
                otherwise,
            } => {
                for (index, (cond, body)) in branches.iter().enumerate() {
                    let cond = self.expr(cond, 0);
                    if index == 0 {
                        self.line(&format!("if {cond} {{"));
                    } else {
                        self.line(&format!("}} else if {cond} {{"));
                    }
                    self.body(body);
                }
                if let Some(body) = otherwise {
                    self.line("} else {");
                    self.body(body);
                }
                self.line("}");
            }
            StmtKind::While { cond, body } => {
                let head = format!("while {}", self.expr(cond, 0));
                self.block_stmt(&head, body);
            }
            StmtKind::Repeat { count, body } => {
                let head = format!("repeat {}", self.expr(count, 0));
                self.block_stmt(&head, body);
            }
            StmtKind::For {
                var,
                second,
                iter,
                body,
            } => {
                let names = match second {
                    Some(second) => format!("{var}, {second}"),
                    None => var.clone(),
                };
                let head = format!("for {names} in {}", self.expr(iter, 0));
                self.block_stmt(&head, body);
            }
            StmtKind::Break => self.line("break"),
            StmtKind::Continue => self.line("continue"),
            StmtKind::Return(value) => match value {
                Some(value) => {
                    let value = self.expr(value, 0);
                    self.line(&format!("return {value}"));
                }
                None => self.line("return"),
            },
            StmtKind::Try {
                body,
                name,
                handler,
            } => {
                self.line("try {");
                self.body(body);
                match name {
                    Some(name) => self.line(&format!("}} oops {name} {{")),
                    None => self.line("} oops {"),
                }
                self.body(handler);
                self.line("}");
            }
            StmtKind::Do(body) => self.block_stmt("do!", body),
            StmtKind::Later { delay, body } => {
                let head = format!("later! {}", self.expr(delay, 0));
                self.block_stmt(&head, body);
            }
            StmtKind::Every { interval, body } => {
                let head = format!("every! {}", self.expr(interval, 0));
                self.block_stmt(&head, body);
            }
            StmtKind::Fn(def) => self.fn_def(def),
            StmtKind::Comment(text) => {
                if text.is_empty() {
                    self.line("#");
                } else {
                    self.line(&format!("# {text}"));
                }
            }
        }
    }

    /// `expr` as text, in parentheses when its binding is weaker than
    /// `outer`.
    fn expr(&mut self, expr: &Expr, outer: u8) -> String {
        match &expr.kind {
            ExprKind::Null => "null".into(),
            ExprKind::Bool(value) => value.to_string(),
            ExprKind::Num(value, unit) => {
                let mut text = number(*value);
                if let Some(unit) = unit {
                    text.push_str(unit.text());
                }
                if *value < 0.0 && outer > 8 {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Str(text) => quote(text),
            ExprKind::Template(parts) => {
                let mut text = String::from("`");
                for part in parts {
                    match part {
                        TemplatePart::Text(piece) => {
                            for c in piece.chars() {
                                match c {
                                    '`' | '{' | '\\' => {
                                        text.push('\\');
                                        text.push(c);
                                    }
                                    '\n' => text.push_str("\\n"),
                                    c => text.push(c),
                                }
                            }
                        }
                        TemplatePart::Expr(inner) => {
                            text.push('{');
                            text.push_str(&self.expr(inner, 0));
                            text.push('}');
                        }
                    }
                }
                text.push('`');
                text
            }
            ExprKind::Var(name) => name.clone(),
            ExprKind::List(items) => {
                let items = items
                    .iter()
                    .map(|item| self.expr(item, 0))
                    .collect::<Vec<_>>();
                format!("[{}]", items.join(", "))
            }
            ExprKind::Map(entries) => {
                if entries.is_empty() {
                    return "{}".into();
                }
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        let key = if is_plain_name(key) {
                            key.clone()
                        } else {
                            quote(key)
                        };
                        format!("{key}: {}", self.expr(value, 0))
                    })
                    .collect::<Vec<_>>();
                format!("{{{}}}", entries.join(", "))
            }
            ExprKind::Field(object, name) => format!("{}.{name}", self.expr(object, 10)),
            ExprKind::Index(object, index) => {
                format!("{}[{}]", self.expr(object, 10), self.expr(index, 0))
            }
            ExprKind::Call { callee, args, bang } => {
                let callee_text = self.expr(callee, 10);
                let args = args.iter().map(|arg| self.expr(arg, 0)).collect::<Vec<_>>();
                // Commands keep their bare look only where a statement starts.
                // `say! (1 + 2) * 3` would read as a call with parentheses.
                let bare = args.first().is_none_or(|first| !first.starts_with('('));
                if *bang && bare && outer == 0 && crate::parser::bang_name(callee).is_some() {
                    if args.is_empty() {
                        callee_text
                    } else {
                        format!("{callee_text} {}", args.join(", "))
                    }
                } else {
                    format!("{callee_text}({})", args.join(", "))
                }
            }
            ExprKind::Unary(op, operand) => {
                let inner = self.expr(operand, 9);
                let text = match op {
                    UnOp::Neg => format!("-{inner}"),
                    UnOp::Not => format!("not {inner}"),
                };
                if outer > 9 {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Binary(op, left, right) => {
                let precedence = op.precedence();
                let (left_min, right_min) = if *op == BinOp::Pow {
                    (precedence + 2, precedence)
                } else {
                    (precedence, precedence + 1)
                };
                let text = format!(
                    "{} {} {}",
                    self.expr(left, left_min),
                    op.text(),
                    self.expr(right, right_min)
                );
                if precedence < outer {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Range {
                from,
                to,
                inclusive,
            } => {
                let text = format!(
                    "{}{}{}",
                    self.expr(from, 1),
                    if *inclusive { ".." } else { "..<" },
                    self.expr(to, 1)
                );
                if outer > 0 {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Pipe(value, target) => {
                let text = format!("{} |> {}", self.expr(value, 0), self.expr(target, 1));
                if outer > 0 {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Lambda { params, body } => {
                let mut inner = Printer {
                    text: String::new(),
                    indent: self.indent,
                };
                inner.body(body);
                let mut closing = String::new();
                for _ in 0..self.indent {
                    closing.push_str("  ");
                }
                format!("fn({}) {{\n{}{closing}}}", params.join(", "), inner.text)
            }
        }
    }
}

pub fn is_plain_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(crate::lexer::is_ident_start)
        && chars.all(crate::lexer::is_ident_continue)
        && !crate::parser::KEYWORDS.contains(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// Printing and parsing again changes nothing.
    fn round_trip(src: &str) {
        let first = parse(src).unwrap_or_else(|error| panic!("{error}\n{src}"));
        let printed = print(&first);
        let second = parse(&printed).unwrap_or_else(|error| panic!("{error}\n{printed}"));
        let again = print(&second);
        assert_eq!(printed, again, "printing is not stable:\n{printed}");
        let strip = |program: &Program| {
            program
                .items
                .iter()
                .map(|item| format!("{:?}", strip_lines(&item.kind)))
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(&first), strip(&second), "{printed}");
    }

    /// Line numbers move when printing; compare everything else.
    fn strip_lines(kind: &ItemKind) -> String {
        let text = format!("{kind:?}");
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(index) = rest.find("line: ") {
            out.push_str(&rest[..index]);
            rest = &rest[index + 6..];
            rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn programs_survive_printing() {
        round_trip(
            r#"plugin "Test" {
  id = "a.b"
}
settings {
  slider size "Size" = 4 from -1 to 50 step 0.5 ? "help"
  choice mode "Mode" = "a" of "a", "b"
  heading "Look"
}
keep n = 0
# about the menu
when menu "Go" {
  n += 1
  say! `n is {n + 1}`, 2
  x = -(2 ^ 2) + (1 + 2) * 3
  y = [1, 2, {a: 1, "b c": 2}][0]
  if a and not b {
    break
  } else if c {
    continue
  } else {
    return x
  }
  for k, v in m {
    a <-> b
  }
  try {
    oops! "no"
  } oops err {
    print! err
  }
  later! 500ms {
    every! 1s {
      deselect!
    }
  }
  f = fn(a) {
    return a |> twice()
  }
  r = 1..<10
}
fn twice(x) {
  return x * 2
}
"#,
        );
    }

    #[test]
    fn numbers_print_plainly() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(-2.5), "-2.5");
        assert_eq!(number(1e-9), "0.000000001");
    }
}
