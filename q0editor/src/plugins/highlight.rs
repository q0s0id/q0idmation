//! Colours for q0ex text in the studio. Forgiving: half-typed code still
//! gets its colours.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

use crate::settings::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Plain,
    Keyword,
    Command,
    Builtin,
    Text,
    Number,
    Comment,
    Operator,
    Fence,
}

const KEYWORDS: [&str; 30] = [
    "if", "else", "while", "repeat", "for", "in", "break", "continue", "return", "try", "oops",
    "fn", "when", "keep", "and", "or", "not", "true", "false", "null", "plugin", "settings",
    "from", "to", "step", "of", "slider", "toggle", "choice", "heading",
];

/// Splits `text` into coloured runs.
pub(crate) fn runs(text: &str) -> Vec<(std::ops::Range<usize>, Kind)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut fenced = false;
    let mut line_start = true;
    while at < bytes.len() {
        // Page and blob sections are prose, not code.
        if line_start {
            let end = text[at..]
                .find('\n')
                .map_or(text.len(), |offset| at + offset);
            let line = text[at..end].trim();
            let opens = !fenced
                && line.ends_with("~~~")
                && (line.starts_with("page") || line.starts_with("blob "));
            if fenced || opens {
                if line == "~~~" || opens {
                    out.push((at..end, Kind::Fence));
                    fenced = opens;
                } else {
                    out.push((at..end, Kind::Plain));
                }
                at = end;
                // The line break after it is lexed as usual.
                line_start = false;
                continue;
            }
        }
        let c = bytes[at];
        let start = at;
        line_start = false;
        let kind = match c {
            b'\n' => {
                at += 1;
                line_start = true;
                Kind::Plain
            }
            b'#' => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                Kind::Comment
            }
            b'"' | b'\'' | b'`' => {
                at += 1;
                while at < bytes.len() && bytes[at] != c && bytes[at] != b'\n' {
                    if bytes[at] == b'\\' {
                        at += 1;
                    }
                    at += 1;
                }
                at = (at + 1).min(bytes.len());
                Kind::Text
            }
            c if c.is_ascii_digit() => {
                while at < bytes.len()
                    && (bytes[at].is_ascii_alphanumeric()
                        || (bytes[at] == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit)))
                {
                    at += 1;
                }
                Kind::Number
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                    at += 1;
                }
                let bang = bytes.get(at) == Some(&b'!') && bytes.get(at + 1) != Some(&b'=');
                if bang {
                    at += 1;
                }
                let word = &text[start..at];
                if word == "do!" || word == "later!" || word == "every!" {
                    Kind::Keyword
                } else if bang {
                    Kind::Command
                } else if KEYWORDS.contains(&word) {
                    Kind::Keyword
                } else if q0ex::stdlib::has(word) || super::api::is_namespace(word) {
                    Kind::Builtin
                } else {
                    Kind::Plain
                }
            }
            b' ' | b'\t' | b'\r' => {
                at += 1;
                Kind::Plain
            }
            _ => {
                at += 1;
                // Keep a multi-byte character whole.
                while at < bytes.len() && !text.is_char_boundary(at) {
                    at += 1;
                }
                if c.is_ascii_punctuation() {
                    Kind::Operator
                } else {
                    Kind::Plain
                }
            }
        };
        out.push((start..at, kind));
    }
    out
}

pub(crate) fn color(kind: Kind, theme: &Theme) -> Color32 {
    match kind {
        Kind::Keyword => theme.syntax_keyword.to_color32(),
        Kind::Command => theme.syntax_signal.to_color32(),
        Kind::Builtin => theme.syntax_builtin.to_color32(),
        Kind::Text => theme.syntax_string.to_color32(),
        Kind::Number => theme.syntax_number.to_color32(),
        Kind::Comment => theme.syntax_comment.to_color32(),
        Kind::Operator => theme.syntax_operator.to_color32(),
        Kind::Fence => theme.accent.to_color32(),
        Kind::Plain => theme.syntax_identifier.to_color32(),
    }
}

/// A coloured layout of q0ex `text`, with `error_line` (1-based) marked.
pub(crate) fn layout(
    text: &str,
    theme: &Theme,
    font: FontId,
    wrap: f32,
    error_line: Option<u32>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut line = 1u32;
    let error_back = Color32::from_rgba_unmultiplied(235, 87, 87, 40);
    for (range, kind) in runs(text) {
        let piece = &text[range];
        // Split runs at line ends so the error line gets its own background.
        for part in piece.split_inclusive('\n') {
            let format = TextFormat {
                font_id: font.clone(),
                color: color(kind, theme),
                italics: kind == Kind::Comment,
                background: if Some(line) == error_line {
                    error_back
                } else {
                    Color32::TRANSPARENT
                },
                ..Default::default()
            };
            job.append(part, 0.0, format);
            if part.ends_with('\n') {
                line += 1;
            }
        }
    }
    if text.is_empty() {
        job.append(
            "",
            0.0,
            TextFormat {
                font_id: font,
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, Kind)> {
        runs(text)
            .into_iter()
            .filter(|(_, kind)| *kind != Kind::Plain)
            .map(|(range, kind)| (text[range].to_string(), kind))
            .collect()
    }

    #[test]
    fn commands_keywords_and_text_get_their_colours() {
        let found = kinds("when menu \"Go\" {\n  say! 500ms # hi\n}");
        assert!(found.contains(&("when".into(), Kind::Keyword)));
        assert!(found.contains(&("\"Go\"".into(), Kind::Text)));
        assert!(found.contains(&("say!".into(), Kind::Command)));
        assert!(found.contains(&("500ms".into(), Kind::Number)));
        assert!(found.contains(&("# hi".into(), Kind::Comment)));
    }

    #[test]
    fn page_sections_are_not_code() {
        let found = kinds("page ~~~\nif this were code\n~~~\nif x {");
        assert_eq!(found[0].1, Kind::Fence);
        assert!(!found.iter().any(|(text, _)| text == "this"));
        assert!(found.iter().filter(|(text, _)| text == "if").count() == 1);
    }

    #[test]
    fn half_typed_code_still_colours() {
        let found = kinds("x = \"never closed\ny = `also");
        assert_eq!(
            found.iter().filter(|(_, kind)| *kind == Kind::Text).count(),
            2
        );
    }
}
