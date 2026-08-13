//! q0lang lexer + egui layouter for the script editor.
//!
//! Reference syntax:
//!   X = Y
//!   title = "hello"
//!   x = sin(pi / 2) + 3 * -2
//!   gorun! 12
//!   gostop! intro
//!   listen mouse.click onClick
//!   xlisten mouse.click onClick
//!   pb func openDoor actor speed
//!   pr func cacheFrame frame
//!   do! { ... }
//!
//! The tokenizer is tiny and intentionally forgiving — it never panics on
//! malformed input, it just falls back to `Identifier` for whatever it
//! can't classify. That matters because the editor live-relexes every
//! keystroke; a broken intermediate state must still render.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

use crate::settings::Theme;

/// Lexical token classes.  `Whitespace` and `Newline` are kept so the
/// layouter can faithfully reconstruct the source byte-for-byte; the
/// editor's TextEdit holds the original `String`, but the layouter has
/// to emit a `LayoutJob` whose visible glyphs match exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Builtin,
    /// Module/library identifier following `import` (qos.lib.OmegaInjector).
    DottedPath,
    String,
    Number,
    Comment,
    Operator,
    /// `Q0Signal UP!` and similar — uppercase identifiers ending in `!`.
    Signal,
    Identifier,
    Whitespace,
    Newline,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

/// Top-level keywords. Case-sensitive.
const KEYWORDS: &[&str] = &[
    "import", "listen", "xlisten", "pb", "pr", "func", "if", "else", "return", "true", "false",
    "null", "for", "while", "in", "as", "self",
];

/// Built-in libraries / module names known from the q0lang notes. Highlighted
/// distinctly so authoring "import qos.lib.X" feels right.
const BUILTINS: &[&str] = &[
    "q0",
    "mouse",
    "timeline",
    "core",
    "q0.mouse",
    "q0.timeline",
    "q0.core",
    "q0.math",
    "q0.rig",
    "rig",
    "q0rig",
    "q0rig.position",
    "q0rig.value",
    "q0rig.reset",
    "q0shell",
    "q0shell.print",
    "q0shell.read",
    "q0shell.write",
    "q0shell.append",
    "q0shell.mkdir",
    "q0shell.list",
    "q0shell.move",
    "q0shell.delete",
    "mouse.down",
    "mouse.up",
    "mouse.move",
    "mouse.click",
    "mouse.double",
    "mouse.wheel",
    "mouse.over",
    "mouse.out",
    "frame",
    "label",
    "pi",
    "tau",
    "sin",
    "cos",
    "tan",
    "sqrt",
    "abs",
    "min",
    "max",
    "clamp",
    "lerp",
    "floor",
    "ceil",
    "round",
];

pub fn tokenize(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(src.len() / 4 + 4);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Newline first — keep separate so the layouter can pass through line breaks.
        if b == b'\n' {
            out.push(Token {
                kind: TokenKind::Newline,
                start: i,
                end: i + 1,
            });
            i += 1;
            continue;
        }
        if b == b'\r' {
            // Treat \r\n as one Newline pair and standalone \r as one too.
            let end = if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i + 2
            } else {
                i + 1
            };
            out.push(Token {
                kind: TokenKind::Newline,
                start: i,
                end,
            });
            i = end;
            continue;
        }
        if b == b' ' || b == b'\t' {
            let s = i;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::Whitespace,
                start: s,
                end: i,
            });
            continue;
        }
        // Comment: ! ... ! on a single line. Closing ! is optional at EOL —
        // treat unterminated as a comment to end of line so the editor
        // colours partial typing nicely.
        if b == b'!' {
            let s = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                if bytes[i] == b'!' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::Comment,
                start: s,
                end: i,
            });
            continue;
        }
        // String: "..." — backslash escapes are not part of q0lang per the
        // examples, but we tolerate them by skipping the next byte.
        if b == b'"' {
            let s = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::String,
                start: s,
                end: i,
            });
            continue;
        }
        // Single-quoted parameter literal: 'key=...'  (q0lang style)
        if b == b'\'' {
            let s = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                if bytes[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::String,
                start: s,
                end: i,
            });
            continue;
        }
        // Number: digit-led, accept one dot.
        if b.is_ascii_digit() {
            let s = i;
            let mut seen_dot = false;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_digit() {
                    i += 1;
                } else if c == b'.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Token {
                kind: TokenKind::Number,
                start: s,
                end: i,
            });
            continue;
        }
        // Identifier / keyword / signal / dotted path.
        if is_ident_start(b) {
            let s = i;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            // Suffix `!` makes it a Signal (Q0Signal, Start!, OTStart!, UP!).
            let is_signal = i < bytes.len() && bytes[i] == b'!';
            if is_signal {
                i += 1;
                out.push(Token {
                    kind: TokenKind::Signal,
                    start: s,
                    end: i,
                });
                continue;
            }
            // Dotted path (qos.lib.X) — only if the next byte is `.` followed by another identifier.
            if i < bytes.len()
                && bytes[i] == b'.'
                && i + 1 < bytes.len()
                && is_ident_start(bytes[i + 1])
            {
                let mut j = i;
                while j < bytes.len() {
                    if bytes[j] == b'.' && j + 1 < bytes.len() && is_ident_start(bytes[j + 1]) {
                        j += 1;
                        while j < bytes.len() && is_ident_continue(bytes[j]) {
                            j += 1;
                        }
                    } else {
                        break;
                    }
                }
                out.push(Token {
                    kind: TokenKind::DottedPath,
                    start: s,
                    end: j,
                });
                i = j;
                continue;
            }
            let word = &src[s..i];
            let kind = if KEYWORDS.contains(&word) {
                TokenKind::Keyword
            } else if BUILTINS.contains(&word) {
                TokenKind::Builtin
            } else {
                TokenKind::Identifier
            };
            out.push(Token {
                kind,
                start: s,
                end: i,
            });
            continue;
        }
        // Operator / punctuation — single byte at a time.
        if matches!(
            b,
            b'(' | b')'
                | b'{'
                | b'}'
                | b'['
                | b']'
                | b'='
                | b','
                | b';'
                | b':'
                | b'+'
                | b'-'
                | b'*'
                | b'/'
                | b'<'
                | b'>'
                | b'?'
                | b'.'
                | b'@'
                | b'%'
                | b'&'
                | b'|'
                | b'^'
                | b'#'
        ) {
            out.push(Token {
                kind: TokenKind::Operator,
                start: i,
                end: i + 1,
            });
            i += 1;
            continue;
        }
        // Unknown byte — emit one Other and keep going so we never loop.
        let s = i;
        // Walk forward bytewise but try to stop at the next "interesting"
        // boundary so we don't shrink tokens to size-1 over multi-byte UTF-8
        // letters; this also means QSGCW.q0lang's non-ASCII / odd ASCII art
        // gets rendered as one token rather than 50.
        i += 1;
        while i < bytes.len()
            && !is_ident_start(bytes[i])
            && !bytes[i].is_ascii_digit()
            && bytes[i] != b'\n'
            && bytes[i] != b'\r'
            && bytes[i] != b' '
            && bytes[i] != b'\t'
            && bytes[i] != b'!'
            && bytes[i] != b'"'
            && bytes[i] != b'\''
            && !matches!(
                bytes[i],
                b'(' | b')'
                    | b'{'
                    | b'}'
                    | b'['
                    | b']'
                    | b'='
                    | b','
                    | b';'
                    | b':'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'/'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'.'
                    | b'@'
                    | b'%'
                    | b'&'
                    | b'|'
                    | b'^'
                    | b'#'
            )
        {
            i += 1;
        }
        out.push(Token {
            kind: TokenKind::Other,
            start: s,
            end: i,
        });
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Map a token to the colour pulled from the active theme.
fn token_color(kind: TokenKind, theme: &Theme) -> Color32 {
    match kind {
        TokenKind::Keyword => theme.syntax_keyword.to_color32(),
        TokenKind::Builtin => theme.syntax_builtin.to_color32(),
        TokenKind::DottedPath => theme.syntax_builtin.to_color32(),
        TokenKind::String => theme.syntax_string.to_color32(),
        TokenKind::Number => theme.syntax_number.to_color32(),
        TokenKind::Comment => theme.syntax_comment.to_color32(),
        TokenKind::Operator => theme.syntax_operator.to_color32(),
        TokenKind::Signal => theme.syntax_signal.to_color32(),
        TokenKind::Identifier => theme.syntax_identifier.to_color32(),
        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Other => {
            theme.syntax_identifier.to_color32()
        }
    }
}

/// Build a `LayoutJob` for a single line of source. The script editor
/// uses egui's per-line layouter callback (one call per visible row), so
/// this runs hot — keep it allocation-light. We retokenise the line each
/// time rather than caching, because lines are short and the cost is
/// dwarfed by glyph layout anyway.
pub fn layout_line(src: &str, theme: &Theme, font: FontId, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    if src.is_empty() {
        // egui still expects an empty job to carry the right font so the
        // caret renders at the right height; insert a zero-width section.
        job.append(
            "",
            0.0,
            TextFormat {
                font_id: font,
                color: theme.syntax_identifier.to_color32(),
                ..Default::default()
            },
        );
        return job;
    }
    let tokens = tokenize(src);
    for t in &tokens {
        // Newline tokens shouldn't appear because the layouter is called per
        // visual line, but if egui ever passes a multi-line slice we still
        // want correct behaviour.
        let slice = &src[t.start..t.end];
        let color = token_color(t.kind, theme);
        let format = TextFormat {
            font_id: font.clone(),
            color,
            italics: matches!(t.kind, TokenKind::Comment),
            ..Default::default()
        };
        job.append(slice, 0.0, format);
    }
    job
}
