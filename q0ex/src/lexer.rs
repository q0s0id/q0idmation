//! Text to tokens. Fenced `page` and `blob` sections never reach here;
//! [`crate::parser`] cuts them out first.

use crate::ast::Unit;
use crate::parser::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64, Option<Unit>),
    Str(String),
    /// A backtick string: its text and `{code}` pieces, in order.
    Template(Vec<RawPart>),
    /// A name, with a trailing `!` when it is a command (`say!`).
    Ident(String),
    Sym(&'static str),
    /// `# ...` on a line of its own or after code.
    Comment(String),
    /// `#@ x y`: where the block editor puts the next item.
    Pos(f32, f32),
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawPart {
    Text(String),
    Code(String, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Byte range in the source.
    pub start: usize,
    pub end: usize,
}

/// Longest first, so `<->` wins over `<-` and `<`.
const SYMBOLS: [&str; 34] = [
    "<->", "..<", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=", "|>", "..", "&&", "||", "(", ")",
    "[", "]", "{", "}", ",", ":", ".", "=", "<", ">", "+", "-", "*", "/", "%", "^", "!", "?",
];

pub fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

pub fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub fn tokenize(src: &str, first_line: u32) -> Result<Vec<Token>, ParseError> {
    let mut lexer = Lexer {
        src,
        chars: src.char_indices().collect(),
        at: 0,
        line: first_line,
        tokens: Vec::new(),
    };
    lexer.run()?;
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    at: usize,
    line: u32,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).map(|(_, c)| *c)
    }

    fn offset(&self) -> usize {
        self.chars
            .get(self.at)
            .map_or(self.src.len(), |(offset, _)| *offset)
    }

    fn push(&mut self, tok: Tok, start: usize, line: u32) {
        let end = self.offset();
        self.tokens.push(Token {
            tok,
            line,
            start,
            end,
        });
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            message: message.into(),
        }
    }

    fn run(&mut self) -> Result<(), ParseError> {
        while let Some(c) = self.peek(0) {
            let start = self.offset();
            let line = self.line;
            match c {
                '\n' => {
                    self.at += 1;
                    self.push(Tok::Newline, start, line);
                    self.line += 1;
                }
                ' ' | '\t' | '\r' => self.at += 1,
                '#' => self.comment(start)?,
                '"' | '\'' => {
                    let text = self.string(c)?;
                    self.push(Tok::Str(text), start, line);
                }
                '`' => {
                    let parts = self.template()?;
                    self.push(Tok::Template(parts), start, line);
                }
                c if c.is_ascii_digit() => self.number(start)?,
                c if is_ident_start(c) => {
                    let mut name = String::new();
                    while let Some(c) = self.peek(0).filter(|c| is_ident_continue(*c)) {
                        name.push(c);
                        self.at += 1;
                    }
                    // `say!` is a command; `x != y` is not.
                    if self.peek(0) == Some('!') && self.peek(1) != Some('=') {
                        name.push('!');
                        self.at += 1;
                    }
                    self.push(Tok::Ident(name), start, line);
                }
                _ => {
                    let rest = &self.src[start..];
                    let Some(symbol) = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol))
                    else {
                        return Err(self.error(format!("`{c}` means nothing here")));
                    };
                    self.at += symbol.chars().count();
                    self.push(Tok::Sym(symbol), start, line);
                }
            }
        }
        let end = self.src.len();
        self.tokens.push(Token {
            tok: Tok::Eof,
            line: self.line,
            start: end,
            end,
        });
        Ok(())
    }

    fn comment(&mut self, start: usize) -> Result<(), ParseError> {
        let line = self.line;
        let mut text = String::new();
        self.at += 1;
        while let Some(c) = self.peek(0).filter(|c| *c != '\n') {
            text.push(c);
            self.at += 1;
        }
        if let Some(rest) = text.strip_prefix('@') {
            let numbers = rest
                .split_whitespace()
                .map(str::parse::<f32>)
                .collect::<Result<Vec<_>, _>>();
            if let Ok(numbers) = numbers {
                if let [x, y] = numbers[..] {
                    self.push(Tok::Pos(x, y), start, line);
                    return Ok(());
                }
            }
        }
        let text = text
            .strip_prefix(' ')
            .unwrap_or(&text)
            .trim_end()
            .to_string();
        self.push(Tok::Comment(text), start, line);
        Ok(())
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        let Some(c) = self.peek(0) else {
            return Err(self.error("the file ends inside a string"));
        };
        self.at += 1;
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            other => other,
        })
    }

    fn string(&mut self, quote: char) -> Result<String, ParseError> {
        self.at += 1;
        let mut text = String::new();
        loop {
            match self.peek(0) {
                None | Some('\n') => return Err(self.error("this string never ends")),
                Some('\\') => {
                    self.at += 1;
                    text.push(self.escape()?);
                }
                Some(c) if c == quote => {
                    self.at += 1;
                    return Ok(text);
                }
                Some(c) => {
                    text.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn template(&mut self) -> Result<Vec<RawPart>, ParseError> {
        self.at += 1;
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek(0) {
                None | Some('\n') => return Err(self.error("this `string` never ends")),
                Some('\\') => {
                    self.at += 1;
                    text.push(self.escape()?);
                }
                Some('`') => {
                    self.at += 1;
                    if !text.is_empty() {
                        parts.push(RawPart::Text(text));
                    }
                    return Ok(parts);
                }
                Some('{') => {
                    self.at += 1;
                    if !text.is_empty() {
                        parts.push(RawPart::Text(std::mem::take(&mut text)));
                    }
                    let mut depth = 1;
                    let mut code = String::new();
                    let mut quote: Option<char> = None;
                    loop {
                        let Some(c) = self.peek(0).filter(|c| *c != '\n') else {
                            return Err(self.error("a `{` in this `string` is never closed"));
                        };
                        self.at += 1;
                        match (quote, c) {
                            (Some(q), c) if c == q => quote = None,
                            (Some(_), '\\') => {
                                code.push(c);
                                if let Some(next) = self.peek(0) {
                                    code.push(next);
                                    self.at += 1;
                                }
                                continue;
                            }
                            (Some(_), _) => {}
                            (None, '"' | '\'') => quote = Some(c),
                            (None, '{') => depth += 1,
                            (None, '}') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        code.push(c);
                    }
                    parts.push(RawPart::Code(code, self.line));
                }
                Some(c) => {
                    text.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn number(&mut self, start: usize) -> Result<(), ParseError> {
        let line = self.line;
        let mut text = String::new();
        while let Some(c) = self.peek(0).filter(char::is_ascii_digit) {
            text.push(c);
            self.at += 1;
        }
        // `1..5` is a range, not one point five.
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            text.push('.');
            self.at += 1;
            while let Some(c) = self.peek(0).filter(char::is_ascii_digit) {
                text.push(c);
                self.at += 1;
            }
        }
        let value: f64 = text
            .parse()
            .map_err(|_| self.error(format!("`{text}` is not a number")))?;
        let mut unit_text = String::new();
        while let Some(c) = self.peek(0).filter(|c| c.is_ascii_alphabetic()) {
            unit_text.push(c);
            self.at += 1;
        }
        let unit = if unit_text.is_empty() {
            None
        } else {
            Some(Unit::parse(&unit_text).ok_or_else(|| {
                self.error(format!(
                    "`{text}{unit_text}`: numbers take the units ms, s, deg and px"
                ))
            })?)
        };
        self.push(Tok::Num(value, unit), start, line);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src, 1)
            .unwrap()
            .into_iter()
            .map(|token| token.tok)
            .collect()
    }

    #[test]
    fn commands_keep_their_bang_and_not_equal_stays_an_operator() {
        assert_eq!(
            toks("say! x != y"),
            vec![
                Tok::Ident("say!".into()),
                Tok::Ident("x".into()),
                Tok::Sym("!="),
                Tok::Ident("y".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn numbers_take_units_and_ranges_stay_ranges() {
        assert_eq!(
            toks("500ms 1..5 0.25"),
            vec![
                Tok::Num(500.0, Some(Unit::Ms)),
                Tok::Num(1.0, None),
                Tok::Sym(".."),
                Tok::Num(5.0, None),
                Tok::Num(0.25, None),
                Tok::Eof
            ]
        );
        assert!(tokenize("3apples", 1).is_err());
    }

    #[test]
    fn templates_split_into_text_and_code() {
        assert_eq!(
            toks("`frame {n + 1}!`"),
            vec![
                Tok::Template(vec![
                    RawPart::Text("frame ".into()),
                    RawPart::Code("n + 1".into(), 1),
                    RawPart::Text("!".into())
                ]),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn swaps_pipes_and_positions() {
        assert_eq!(
            toks("#@ 10 20\na <-> b |> f"),
            vec![
                Tok::Pos(10.0, 20.0),
                Tok::Newline,
                Tok::Ident("a".into()),
                Tok::Sym("<->"),
                Tok::Ident("b".into()),
                Tok::Sym("|>"),
                Tok::Ident("f".into()),
                Tok::Eof
            ]
        );
    }
}
