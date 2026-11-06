//! Tokens to a [`Program`].
//!
//! The grammar is q0lang's cousin: one statement a line, `{` ends a header
//! and `}` closes on a line of its own (or right after a one-line body),
//! `#` comments, and `name! args` commands. Lists, maps and calls may run
//! over several lines.

use std::rc::Rc;

use crate::ast::*;
use crate::lexer::{tokenize, RawPart, Tok, Token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Words that cannot name a variable.
pub const KEYWORDS: [&str; 22] = [
    "if", "else", "while", "repeat", "for", "in", "break", "continue", "return", "try", "oops",
    "do!", "later!", "every!", "fn", "when", "keep", "and", "or", "not", "true", "false",
];

/// Parses a whole plugin file.
pub fn parse(src: &str) -> Result<Program, ParseError> {
    let (code, fenced) = cut_fences(src)?;
    let tokens = tokenize(&code, 1)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
    };
    let mut items = parser.items()?;
    // Fenced sections go back where they were written.
    for item in fenced {
        let index = items
            .iter()
            .position(|other| other.line > item.line)
            .unwrap_or(items.len());
        items.insert(index, item);
    }
    Ok(Program { items })
}

/// Parses one expression, as the block editor's text fields need.
pub fn parse_expr(src: &str) -> Result<Expr, ParseError> {
    let tokens = tokenize(src, 1)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
    };
    let expr = parser.expr()?;
    parser.skip_newlines();
    if !parser.at_eof() {
        return Err(parser.error("one value was expected"));
    }
    Ok(expr)
}

/// Takes `page ~~~` and `blob name kind ~~~` sections out of the text,
/// leaving their lines blank so line numbers still match.
fn cut_fences(src: &str) -> Result<(String, Vec<Item>), ParseError> {
    let mut code = String::with_capacity(src.len());
    let mut items = Vec::new();
    let mut lines = src.split('\n').enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        let line_no = index as u32 + 1;
        let trimmed = line.trim();
        let header = trimmed
            .strip_suffix("~~~")
            .map(str::trim_end)
            .filter(|head| head.starts_with("page") || head.starts_with("blob "));
        let Some(header) = header else {
            code.push_str(line);
            code.push('\n');
            continue;
        };
        let words = header.split_whitespace().collect::<Vec<_>>();
        let kind = match words[..] {
            ["page"] => None,
            ["blob", name, kind] => Some((name.to_string(), kind.to_string())),
            _ => {
                code.push_str(line);
                code.push('\n');
                continue;
            }
        };
        code.push('\n');
        let mut body = Vec::new();
        let mut closed = false;
        for (_, inner) in lines.by_ref() {
            code.push('\n');
            if inner.trim() == "~~~" {
                closed = true;
                break;
            }
            body.push(inner.trim_end_matches('\r'));
        }
        if !closed {
            return Err(ParseError {
                line: line_no,
                message: "this ~~~ section is never closed with a line of ~~~".into(),
            });
        }
        let kind = match kind {
            None => ItemKind::Page(body.join("\n")),
            Some((name, kind)) => ItemKind::Blob {
                name,
                kind,
                data: body.iter().map(|line| line.trim()).collect(),
            },
        };
        items.push(Item {
            kind,
            line: line_no,
            pos: None,
        });
    }
    if code.ends_with('\n') && !src.ends_with('\n') {
        code.pop();
    }
    Ok((code, items))
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    /// How deeply expressions and blocks nest right now.
    depth: u32,
}

/// Nesting deeper than this is refused rather than risking the stack.
const MAX_NESTING: u32 = 64;

impl Parser {
    fn tok(&self) -> &Tok {
        &self.tokens[self.at.min(self.tokens.len() - 1)].tok
    }

    fn peek_tok(&self, ahead: usize) -> &Tok {
        &self.tokens[(self.at + ahead).min(self.tokens.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.tokens[self.at.min(self.tokens.len() - 1)].line
    }

    fn bump(&mut self) -> Tok {
        let tok = self.tok().clone();
        if self.at < self.tokens.len() - 1 {
            self.at += 1;
        }
        tok
    }

    fn at_eof(&self) -> bool {
        matches!(self.tok(), Tok::Eof)
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line(),
            message: message.into(),
        }
    }

    fn is_sym(&self, symbol: &str) -> bool {
        matches!(self.tok(), Tok::Sym(s) if *s == symbol)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.tok(), Tok::Ident(name) if name == word)
    }

    fn eat_sym(&mut self, symbol: &str) -> bool {
        if self.is_sym(symbol) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.is_word(word) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, symbol: &str, what: &str) -> Result<(), ParseError> {
        if self.eat_sym(symbol) {
            Ok(())
        } else {
            Err(self.error(format!("expected {what}")))
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.tok(), Tok::Newline) {
            self.bump();
        }
    }

    /// Newlines and comments, as allowed inside brackets.
    fn skip_layout(&mut self) {
        while matches!(self.tok(), Tok::Newline | Tok::Comment(_)) {
            self.bump();
        }
    }

    fn name(&mut self, what: &str) -> Result<String, ParseError> {
        match self.tok().clone() {
            Tok::Ident(name) if !KEYWORDS.contains(&name.as_str()) => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.error(format!("expected {what}"))),
        }
    }

    fn string(&mut self, what: &str) -> Result<String, ParseError> {
        match self.tok().clone() {
            Tok::Str(text) => {
                self.bump();
                Ok(text)
            }
            _ => Err(self.error(format!("expected {what} in quotes"))),
        }
    }

    /// The end of a statement: a line break, a comment, or the end.
    /// A trailing comment comes back to be kept.
    fn end_of_line(&mut self) -> Result<Option<Stmt>, ParseError> {
        match self.tok().clone() {
            Tok::Newline => {
                self.bump();
                Ok(None)
            }
            Tok::Eof => Ok(None),
            Tok::Comment(text) => {
                let line = self.line();
                self.bump();
                Ok(Some(Stmt {
                    kind: StmtKind::Comment(text),
                    line,
                }))
            }
            _ => Err(self.error("one statement a line: something else follows here")),
        }
    }

    // ------------------------------------------------------------- items

    fn items(&mut self) -> Result<Vec<Item>, ParseError> {
        let mut items = Vec::new();
        let mut pos = None;
        loop {
            self.skip_newlines();
            let line = self.line();
            let kind = match self.tok().clone() {
                Tok::Eof => break,
                Tok::Pos(x, y) => {
                    self.bump();
                    pos = Some((x, y));
                    continue;
                }
                Tok::Comment(text) => {
                    self.bump();
                    ItemKind::Stmt(Stmt {
                        kind: StmtKind::Comment(text),
                        line,
                    })
                }
                Tok::Ident(word) if word == "plugin" && matches!(self.peek_tok(1), Tok::Str(_)) => {
                    self.plugin()?
                }
                Tok::Ident(word)
                    if word == "settings" && matches!(self.peek_tok(1), Tok::Sym("{")) =>
                {
                    self.settings()?
                }
                Tok::Ident(word) if word == "when" => self.when()?,
                Tok::Ident(word) if word == "fn" && matches!(self.peek_tok(1), Tok::Ident(_)) => {
                    self.bump();
                    ItemKind::Fn(self.fn_def()?)
                }
                Tok::Ident(word) if word == "keep" => {
                    self.bump();
                    let name = self.name("a name after keep")?;
                    self.expect_sym("=", "`=` and a starting value after the name")?;
                    let value = self.expr()?;
                    ItemKind::Keep { name, value }
                }
                _ => {
                    let stmt = self.statement()?;
                    ItemKind::Stmt(stmt)
                }
            };
            items.push(Item {
                kind,
                line,
                pos: pos.take(),
            });
            if let Some(comment) = self.end_of_line()? {
                items.push(Item {
                    line: comment.line,
                    kind: ItemKind::Stmt(comment),
                    pos: None,
                });
            }
        }
        Ok(items)
    }

    fn plugin(&mut self) -> Result<ItemKind, ParseError> {
        self.bump();
        let name = self.string("the plugin's name")?;
        self.expect_sym("{", "`{` after the plugin's name")?;
        let mut fields = Vec::new();
        loop {
            self.skip_layout();
            if self.eat_sym("}") {
                break;
            }
            if self.at_eof() {
                return Err(self.error("the plugin block is never closed with `}`"));
            }
            let key = self.name("a field name like id or version")?;
            self.expect_sym("=", "`=` after the field name")?;
            let value = self.expr()?;
            fields.push((key, value));
        }
        Ok(ItemKind::Plugin { name, fields })
    }

    fn number_literal(&mut self, what: &str) -> Result<f64, ParseError> {
        let negative = self.eat_sym("-");
        match self.bump() {
            Tok::Num(value, unit) => {
                let value = value * unit.map_or(1.0, Unit::scale);
                Ok(if negative { -value } else { value })
            }
            _ => Err(self.error(format!("expected a number for {what}"))),
        }
    }

    fn settings(&mut self) -> Result<ItemKind, ParseError> {
        self.bump();
        self.expect_sym("{", "`{` after settings")?;
        let mut decls = Vec::new();
        loop {
            self.skip_layout();
            if self.eat_sym("}") {
                break;
            }
            if self.at_eof() {
                return Err(self.error("the settings block is never closed with `}`"));
            }
            let line = self.line();
            let word = match self.bump() {
                Tok::Ident(word) => word,
                _ => return Err(self.error("expected a setting kind like slider or toggle")),
            };
            let kind = SettingKind::from_word(&word).ok_or_else(|| {
                self.error(format!(
                    "`{word}` is not a setting kind; use slider, number, toggle, text, color, \
                     choice, button, key, heading or note"
                ))
            })?;
            let key = if kind.has_key() {
                self.name("the setting's name, as code reads it")?
            } else {
                String::new()
            };
            let label = self.string("the setting's label")?;
            let mut decl = SettingDecl {
                kind,
                key,
                label,
                default: None,
                min: None,
                max: None,
                step: None,
                options: Vec::new(),
                help: None,
                line,
            };
            loop {
                if self.eat_sym("=") {
                    decl.default = Some(self.expr()?);
                } else if self.eat_word("from") {
                    decl.min = Some(self.number_literal("from")?);
                    if !self.eat_word("to") {
                        return Err(self.error("`from` needs a `to`: from 0 to 10"));
                    }
                    decl.max = Some(self.number_literal("to")?);
                } else if self.eat_word("step") {
                    decl.step = Some(self.number_literal("step")?);
                } else if self.eat_word("of") {
                    loop {
                        decl.options.push(self.expr()?);
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                } else if self.eat_sym("?") {
                    decl.help = Some(self.string("the help text")?);
                } else {
                    break;
                }
            }
            if kind == SettingKind::Choice && decl.options.is_empty() {
                return Err(self.error("a choice needs its options: of \"a\", \"b\""));
            }
            decls.push(decl);
            if !matches!(self.tok(), Tok::Newline | Tok::Comment(_)) && !self.is_sym("}") {
                return Err(self.error("one setting a line"));
            }
        }
        Ok(ItemKind::Settings(decls))
    }

    fn when(&mut self) -> Result<ItemKind, ParseError> {
        self.bump();
        let event = match self.bump() {
            Tok::Ident(event) => event,
            _ => return Err(self.error("expected an event after when, like menu or tick")),
        };
        let arg = match self.tok().clone() {
            Tok::Str(text) => {
                self.bump();
                Some(text)
            }
            _ => None,
        };
        let mut params = Vec::new();
        while !self.is_sym("{") {
            params.push(self.name("a parameter name or `{`")?);
            if !self.eat_sym(",") {
                break;
            }
        }
        let body = self.block()?;
        Ok(ItemKind::When(When {
            event,
            arg,
            params,
            body,
        }))
    }

    fn params(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect_sym("(", "`(` and the parameters")?;
        let mut params = Vec::new();
        self.skip_layout();
        if !self.eat_sym(")") {
            loop {
                self.skip_layout();
                params.push(self.name("a parameter name")?);
                self.skip_layout();
                if self.eat_sym(")") {
                    break;
                }
                self.expect_sym(",", "`,` or `)`")?;
            }
        }
        Ok(params)
    }

    fn fn_def(&mut self) -> Result<FnDef, ParseError> {
        let name = self.name("the function's name")?;
        let params = self.params()?;
        let body = self.block()?;
        Ok(FnDef { name, params, body })
    }

    // ------------------------------------------------------------ blocks

    /// `{` and its statements up to `}`. A body on the same line as its
    /// `{` holds one statement: `if done { return }`.
    fn block(&mut self) -> Result<Block, ParseError> {
        self.depth += 1;
        let block = self.block_inner();
        self.depth -= 1;
        block
    }

    fn block_inner(&mut self) -> Result<Block, ParseError> {
        if self.depth > MAX_NESTING {
            return Err(self.error("blocks are nested too deeply"));
        }
        self.expect_sym("{", "`{`")?;
        let mut stmts = Vec::new();
        if !matches!(self.tok(), Tok::Newline | Tok::Comment(_)) {
            if !self.eat_sym("}") {
                stmts.push(self.statement()?);
                self.expect_sym("}", "`}` to close a one-line block")?;
            }
            return Ok(Rc::new(stmts));
        }
        if let Some(comment) = self.end_of_line()? {
            stmts.push(comment);
        }
        loop {
            self.skip_newlines();
            if self.eat_sym("}") {
                break;
            }
            if self.at_eof() {
                return Err(self.error("a `{` is never closed with `}`"));
            }
            if let Tok::Comment(text) = self.tok().clone() {
                let line = self.line();
                self.bump();
                stmts.push(Stmt {
                    kind: StmtKind::Comment(text),
                    line,
                });
                continue;
            }
            if matches!(self.tok(), Tok::Pos(..)) {
                self.bump();
                continue;
            }
            stmts.push(self.statement()?);
            if self.is_sym("}") {
                return Err(self.error("`}` goes on a line of its own"));
            }
            if let Some(comment) = self.end_of_line()? {
                stmts.push(comment);
            }
        }
        Ok(Rc::new(stmts))
    }

    fn statement(&mut self) -> Result<Stmt, ParseError> {
        let line = self.line();
        let word = match self.tok() {
            Tok::Ident(word) => word.clone(),
            _ => String::new(),
        };
        let kind = match word.as_str() {
            "if" => {
                self.bump();
                let mut branches = vec![(self.expr()?, self.block()?)];
                let mut otherwise = None;
                while self.eat_word("else") {
                    if self.eat_word("if") {
                        branches.push((self.expr()?, self.block()?));
                    } else {
                        otherwise = Some(self.block()?);
                        break;
                    }
                }
                StmtKind::If {
                    branches,
                    otherwise,
                }
            }
            "while" => {
                self.bump();
                let cond = self.expr()?;
                StmtKind::While {
                    cond,
                    body: self.block()?,
                }
            }
            "repeat" => {
                self.bump();
                let count = self.expr()?;
                StmtKind::Repeat {
                    count,
                    body: self.block()?,
                }
            }
            "for" => {
                self.bump();
                let var = self.name("a name after for")?;
                let second = if self.eat_sym(",") {
                    Some(self.name("a second name")?)
                } else {
                    None
                };
                if !self.eat_word("in") {
                    return Err(self.error("expected `in`: for item in list"));
                }
                let iter = self.expr()?;
                StmtKind::For {
                    var,
                    second,
                    iter,
                    body: self.block()?,
                }
            }
            "break" => {
                self.bump();
                StmtKind::Break
            }
            "continue" => {
                self.bump();
                StmtKind::Continue
            }
            "return" => {
                self.bump();
                if matches!(self.tok(), Tok::Newline | Tok::Eof | Tok::Comment(_))
                    || self.is_sym("}")
                {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expr()?))
                }
            }
            "try" => {
                self.bump();
                let body = self.block()?;
                if !self.eat_word("oops") {
                    return Err(self.error("a try needs its oops: } oops err {"));
                }
                let name = if self.is_sym("{") {
                    None
                } else {
                    Some(self.name("a name for the error, or `{`")?)
                };
                StmtKind::Try {
                    body,
                    name,
                    handler: self.block()?,
                }
            }
            "do!" => {
                self.bump();
                StmtKind::Do(self.block()?)
            }
            "later!" => {
                self.bump();
                let delay = self.expr()?;
                StmtKind::Later {
                    delay,
                    body: self.block()?,
                }
            }
            "every!" => {
                self.bump();
                let interval = self.expr()?;
                StmtKind::Every {
                    interval,
                    body: self.block()?,
                }
            }
            "fn" if matches!(self.peek_tok(1), Tok::Ident(_)) => {
                self.bump();
                StmtKind::Fn(self.fn_def()?)
            }
            "when" | "keep" => {
                return Err(self.error(format!("`{word}` belongs at the top, outside other blocks")))
            }
            "else" => return Err(self.error("`else` goes right after a `}`: } else {")),
            "oops" => return Err(self.error("`oops` goes right after a try's `}`: } oops err {")),
            _ => self.simple_statement()?,
        };
        Ok(Stmt { kind, line })
    }

    /// How many tokens `name.name.cmd!` takes when the statement starts
    /// with a `!` command, before its arguments.
    fn command_path(&self) -> Option<usize> {
        let mut length = 0;
        loop {
            match self.peek_tok(length) {
                Tok::Ident(name) if !KEYWORDS.contains(&name.as_str()) => {
                    if name.ends_with('!') {
                        return (!matches!(self.peek_tok(length + 1), Tok::Sym("(")))
                            .then_some(length + 1);
                    }
                }
                _ => return None,
            }
            if !matches!(self.peek_tok(length + 1), Tok::Sym(".")) {
                return None;
            }
            length += 2;
        }
    }

    fn simple_statement(&mut self) -> Result<StmtKind, ParseError> {
        let line = self.line();
        // `name! a, b`: a command, without parentheses.
        if let Some(length) = self.command_path() {
            let mut callee: Option<Expr> = None;
            for _ in 0..length.div_ceil(2) {
                let Tok::Ident(name) = self.bump() else {
                    unreachable!("command_path checked the names");
                };
                callee = Some(match callee {
                    None => Expr::new(ExprKind::Var(name), line),
                    Some(object) => Expr::new(ExprKind::Field(Box::new(object), name), line),
                });
                self.eat_sym(".");
            }
            let callee = callee.expect("a command has a name");
            let mut args = Vec::new();
            if !matches!(self.tok(), Tok::Newline | Tok::Eof | Tok::Comment(_)) && !self.is_sym("}")
            {
                loop {
                    args.push(self.expr()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                    self.skip_newlines();
                }
            }
            return Ok(StmtKind::Expr(Expr::new(
                ExprKind::Call {
                    callee: Box::new(callee),
                    args,
                    bang: true,
                },
                line,
            )));
        }
        let target = self.expr()?;
        let op = match self.tok() {
            Tok::Sym("=") => Some(AssignOp::Set),
            Tok::Sym("+=") => Some(AssignOp::Add),
            Tok::Sym("-=") => Some(AssignOp::Sub),
            Tok::Sym("*=") => Some(AssignOp::Mul),
            Tok::Sym("/=") => Some(AssignOp::Div),
            _ => None,
        };
        if let Some(op) = op {
            if !matches!(
                target.kind,
                ExprKind::Var(_) | ExprKind::Field(..) | ExprKind::Index(..)
            ) {
                return Err(self.error("only a name, a field or an item can be set"));
            }
            self.bump();
            let value = self.expr()?;
            return Ok(StmtKind::Assign { target, op, value });
        }
        if self.eat_sym("<->") {
            let other = self.expr()?;
            return Ok(StmtKind::Swap(target, other));
        }
        if self.is_sym("==") {
            return Err(self.error("`==` compares; to set a value use `=`"));
        }
        Ok(StmtKind::Expr(target))
    }

    // ------------------------------------------------------- expressions

    pub(crate) fn expr(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        let expr = self.expr_inner();
        self.depth -= 1;
        expr
    }

    fn expr_inner(&mut self) -> Result<Expr, ParseError> {
        if self.depth > MAX_NESTING {
            return Err(self.error("this is nested too deeply"));
        }
        let mut left = self.range()?;
        while self.is_sym("|>") {
            let line = self.line();
            self.bump();
            self.skip_newlines();
            let right = self.range()?;
            left = Expr::new(ExprKind::Pipe(Box::new(left), Box::new(right)), line);
        }
        Ok(left)
    }

    fn range(&mut self) -> Result<Expr, ParseError> {
        let left = self.binary(0)?;
        let inclusive = if self.is_sym("..") {
            true
        } else if self.is_sym("..<") {
            false
        } else {
            return Ok(left);
        };
        let line = self.line();
        self.bump();
        let right = self.binary(0)?;
        Ok(Expr::new(
            ExprKind::Range {
                from: Box::new(left),
                to: Box::new(right),
                inclusive,
            },
            line,
        ))
    }

    fn binop(&self) -> Option<BinOp> {
        Some(match self.tok() {
            Tok::Sym("+") => BinOp::Add,
            Tok::Sym("-") => BinOp::Sub,
            Tok::Sym("*") => BinOp::Mul,
            Tok::Sym("/") => BinOp::Div,
            Tok::Sym("%") => BinOp::Rem,
            Tok::Sym("==") => BinOp::Eq,
            Tok::Sym("!=") => BinOp::Ne,
            Tok::Sym("<") => BinOp::Lt,
            Tok::Sym("<=") => BinOp::Le,
            Tok::Sym(">") => BinOp::Gt,
            Tok::Sym(">=") => BinOp::Ge,
            Tok::Sym("&&") => BinOp::And,
            Tok::Sym("||") => BinOp::Or,
            Tok::Ident(word) if word == "and" => BinOp::And,
            Tok::Ident(word) if word == "or" => BinOp::Or,
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        while let Some(op) = self.binop() {
            let precedence = op.precedence();
            if precedence < min {
                break;
            }
            let line = self.line();
            self.bump();
            self.skip_newlines();
            let right = self.binary(precedence + 1)?;
            left = Expr::new(ExprKind::Binary(op, Box::new(left), Box::new(right)), line);
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        let line = self.line();
        if self.depth > MAX_NESTING {
            return Err(self.error("this is nested too deeply"));
        }
        if self.eat_sym("-") {
            self.depth += 1;
            let operand = self.unary();
            self.depth -= 1;
            let operand = operand?;
            // `-3` is a number, which reads better in blocks.
            if let ExprKind::Num(value, unit) = operand.kind {
                return Ok(Expr::new(ExprKind::Num(-value, unit), line));
            }
            return Ok(Expr::new(
                ExprKind::Unary(UnOp::Neg, Box::new(operand)),
                line,
            ));
        }
        if self.eat_sym("!") || self.eat_word("not") {
            self.depth += 1;
            let operand = self.unary();
            self.depth -= 1;
            let operand = operand?;
            return Ok(Expr::new(
                ExprKind::Unary(UnOp::Not, Box::new(operand)),
                line,
            ));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr, ParseError> {
        let base = self.postfix()?;
        if self.is_sym("^") {
            let line = self.line();
            self.bump();
            self.depth += 1;
            let exponent = self.unary();
            self.depth -= 1;
            let exponent = exponent?;
            return Ok(Expr::new(
                ExprKind::Binary(BinOp::Pow, Box::new(base), Box::new(exponent)),
                line,
            ));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            let line = self.line();
            if self.eat_sym(".") {
                let field = match self.bump() {
                    Tok::Ident(name) => name,
                    _ => return Err(self.error("expected a name after `.`")),
                };
                expr = Expr::new(ExprKind::Field(Box::new(expr), field), line);
            } else if self.eat_sym("[") {
                self.skip_layout();
                let index = self.expr()?;
                self.skip_layout();
                self.expect_sym("]", "`]`")?;
                expr = Expr::new(ExprKind::Index(Box::new(expr), Box::new(index)), line);
            } else if self.is_sym("(") {
                self.bump();
                let args = self.list_items(")")?;
                expr = Expr::new(
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                        bang: false,
                    },
                    line,
                );
            } else {
                return Ok(expr);
            }
        }
    }

    /// Comma-separated values up to `close`; line breaks are free.
    fn list_items(&mut self, close: &str) -> Result<Vec<Expr>, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_layout();
            if self.eat_sym(close) {
                return Ok(items);
            }
            items.push(self.expr()?);
            self.skip_layout();
            if self.eat_sym(close) {
                return Ok(items);
            }
            self.expect_sym(",", &format!("`,` or `{close}`"))?;
        }
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let line = self.line();
        let kind = match self.bump() {
            Tok::Num(value, unit) => ExprKind::Num(value, unit),
            Tok::Str(text) => ExprKind::Str(text),
            Tok::Template(parts) => {
                let mut out = Vec::new();
                for part in parts {
                    match part {
                        RawPart::Text(text) => out.push(TemplatePart::Text(text)),
                        RawPart::Code(code, code_line) => {
                            let tokens = tokenize(&code, code_line)?;
                            let mut inner = Parser {
                                tokens,
                                at: 0,
                                depth: 0,
                            };
                            let expr = inner.expr()?;
                            if !inner.at_eof() {
                                return Err(inner.error("one value goes inside `{}`"));
                            }
                            out.push(TemplatePart::Expr(expr));
                        }
                    }
                }
                ExprKind::Template(out)
            }
            Tok::Ident(word) => match word.as_str() {
                "true" => ExprKind::Bool(true),
                "false" => ExprKind::Bool(false),
                "null" => ExprKind::Null,
                "fn" => {
                    let params = self.params()?;
                    let body = self.block()?;
                    ExprKind::Lambda { params, body }
                }
                word if KEYWORDS.contains(&word) => {
                    return Err(ParseError {
                        line,
                        message: format!("`{word}` cannot be used as a value here"),
                    })
                }
                _ => ExprKind::Var(word),
            },
            Tok::Sym("(") => {
                self.skip_layout();
                let inner = self.expr()?;
                self.skip_layout();
                self.expect_sym(")", "`)`")?;
                return Ok(inner);
            }
            Tok::Sym("[") => ExprKind::List(self.list_items("]")?),
            Tok::Sym("{") => {
                let mut entries = Vec::new();
                loop {
                    self.skip_layout();
                    if self.eat_sym("}") {
                        break;
                    }
                    let key = match self.bump() {
                        Tok::Ident(name) => name,
                        Tok::Str(text) => text,
                        _ => return Err(self.error("expected a key like name: or \"name\":")),
                    };
                    self.expect_sym(":", "`:` after the key")?;
                    self.skip_layout();
                    entries.push((key, self.expr()?));
                    self.skip_layout();
                    if self.eat_sym("}") {
                        break;
                    }
                    self.expect_sym(",", "`,` or `}`")?;
                }
                ExprKind::Map(entries)
            }
            Tok::Newline | Tok::Eof => {
                return Err(ParseError {
                    line,
                    message: "a value is missing at the end of this line".into(),
                })
            }
            Tok::Comment(_) => {
                return Err(ParseError {
                    line,
                    message: "a value is missing before the comment".into(),
                })
            }
            Tok::Sym(symbol) => {
                return Err(ParseError {
                    line,
                    message: format!("expected a value, not `{symbol}`"),
                })
            }
            Tok::Pos(..) => {
                return Err(ParseError {
                    line,
                    message: "expected a value".into(),
                })
            }
        };
        Ok(Expr::new(kind, line))
    }
}

/// The name of a `!` command `expr` refers to: `say!`, `editor.goto!`.
pub fn bang_name(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Var(name) | ExprKind::Field(_, name) if name.ends_with('!') => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmts(src: &str) -> Vec<StmtKind> {
        parse(src)
            .unwrap()
            .items
            .into_iter()
            .filter_map(|item| match item.kind {
                ItemKind::Stmt(stmt) => Some(stmt.kind),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn commands_take_arguments_without_parentheses() {
        let parsed = stmts("say! \"hi\", 2\ndeselect!");
        let ExprKind::Call { args, bang, .. } = (match &parsed[0] {
            StmtKind::Expr(expr) => &expr.kind,
            other => panic!("{other:?}"),
        }) else {
            panic!()
        };
        assert!(*bang);
        assert_eq!(args.len(), 2);
        assert!(
            matches!(&parsed[1], StmtKind::Expr(Expr { kind: ExprKind::Call { args, .. }, .. }) if args.is_empty())
        );
    }

    #[test]
    fn a_whole_plugin_parses_with_its_sections() {
        let src = r##"# a test
plugin "Redder" {
  id = "q0s.redder"
  version = "1.0"
}

page ~~~
# Redder
makes things **red**. {red}really{/}
~~~

settings {
  slider size "Brush size" = 4 from 1 to 50 step 1 ? "how big"
  choice mode "Mode" = "fill" of "fill", "stroke"
  heading "Looks"
  toggle glow "Glow" = true
}

keep clicks = 0

#@ 40 80
when menu "Make it red" {
  clicks += 1
  for obj in project.selection() {
    obj.tint = "#ff0000"
  }
  later! 500ms { say! `done {clicks}` }
}

fn twice(x) {
  return x * 2
}

blob thumb png ~~~
iVBORw0KGgo=
~~~
"##;
        let program = parse(src).unwrap();
        let kinds = program
            .items
            .iter()
            .map(|item| match &item.kind {
                ItemKind::Plugin { .. } => "plugin",
                ItemKind::Settings(_) => "settings",
                ItemKind::Page(_) => "page",
                ItemKind::Blob { .. } => "blob",
                ItemKind::When(_) => "when",
                ItemKind::Fn(_) => "fn",
                ItemKind::Keep { .. } => "keep",
                ItemKind::Stmt(_) => "stmt",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["stmt", "plugin", "page", "settings", "keep", "when", "fn", "blob"]
        );
        let when = program
            .items
            .iter()
            .find(|item| matches!(item.kind, ItemKind::When(_)))
            .unwrap();
        assert_eq!(when.pos, Some((40.0, 80.0)));
        let ItemKind::Settings(decls) = &program.items[3].kind else {
            panic!()
        };
        assert_eq!(decls.len(), 4);
        assert_eq!(decls[0].max, Some(50.0));
        assert_eq!(decls[0].help.as_deref(), Some("how big"));
        assert_eq!(decls[1].options.len(), 2);
        let ItemKind::Page(page) = &program.items[2].kind else {
            panic!()
        };
        assert!(page.contains("{red}really{/}"));
    }

    #[test]
    fn errors_name_their_line() {
        let error = parse("x = 1\nif x {\n  y = \n}").unwrap_err();
        assert_eq!(error.line, 3);
        let error = parse("when tick {\n  keep a = 1\n}").unwrap_err();
        assert!(error.message.contains("top"), "{error}");
    }

    #[test]
    fn one_line_blocks_lambdas_and_else_if() {
        let parsed = stmts(
            "if a { return }\nelse_ok = 1\nif a {\n  b = 1\n} else if c {\n  b = 2\n} else {\n  b = 3\n}\nf = fn(x) {\n  return x\n}\nlist = [\n  1,\n  2\n]",
        );
        assert_eq!(parsed.len(), 5);
        assert!(
            matches!(&parsed[2], StmtKind::If { branches, otherwise: Some(_) } if branches.len() == 2)
        );
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let deep = format!("x = {}1{}", "(".repeat(500), ")".repeat(500));
        assert!(parse(&deep).unwrap_err().message.contains("deeply"));
        assert!(parse(&format!("x = {}1", "-".repeat(500))).is_err());
    }

    #[test]
    fn power_binds_tighter_than_minus() {
        let expr = parse_expr("-x ^ 2").unwrap();
        assert!(matches!(expr.kind, ExprKind::Unary(UnOp::Neg, _)));
        let expr = parse_expr("1 + 2 * 3 == 7 and true").unwrap();
        assert!(matches!(expr.kind, ExprKind::Binary(BinOp::And, ..)));
    }
}
