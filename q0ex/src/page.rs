//! The markup of a plugin's page: Markdown's basics plus colours, sizes and
//! alignment.
//!
//! ```text
//! # Heading            ## Smaller           ### Smallest
//! **bold** *italic* __underline__ ~~strike~~ `code`
//! {red}coloured{/}  {#ff8800}any colour{/}  {big}bigger{/}  {small}  {huge}
//! [a link](https://q0id.com)
//! - a bullet           1. a numbered line   > a callout
//! ---                  (a divider)
//! ![a caption](thumbnail)   a picture from the plugin's blobs
//! ->centred line<-     ->pushed to the right
//! ```

use crate::stdlib::parse_color;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub color: Option<[u8; 4]>,
    /// Relative size: 1 is normal.
    pub size: f32,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PageBlock {
    Heading(u8, Vec<Span>, Align),
    Paragraph(Vec<Span>, Align),
    Bullet(Vec<Span>, u8),
    Numbered(u32, Vec<Span>),
    Quote(Vec<Span>),
    Code(String),
    Rule,
    Image { blob: String, caption: String },
}

pub fn parse_page(text: &str) -> Vec<PageBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut align = Align::Left;
    let mut lines = text.lines();
    let flush = |paragraph: &mut Vec<String>, align: Align, blocks: &mut Vec<PageBlock>| {
        if !paragraph.is_empty() {
            blocks.push(PageBlock::Paragraph(spans(&paragraph.join(" ")), align));
            paragraph.clear();
        }
    };
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            flush(&mut paragraph, align, &mut blocks);
            continue;
        }
        if trimmed.starts_with("```") {
            flush(&mut paragraph, align, &mut blocks);
            let mut code = Vec::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
                code.push(inner);
            }
            blocks.push(PageBlock::Code(code.join("\n")));
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            flush(&mut paragraph, align, &mut blocks);
            blocks.push(PageBlock::Rule);
            continue;
        }
        let (content, line_align) = match trimmed.strip_prefix("->") {
            Some(rest) => match rest.strip_suffix("<-") {
                Some(inner) => (inner.trim(), Align::Center),
                None => (rest.trim(), Align::Right),
            },
            None => (trimmed, Align::Left),
        };
        let level = content.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&level) && content[level..].starts_with(' ') {
            flush(&mut paragraph, align, &mut blocks);
            blocks.push(PageBlock::Heading(
                level as u8,
                spans(content[level..].trim()),
                line_align,
            ));
            continue;
        }
        if let Some(rest) = content.strip_prefix("![") {
            if let Some((caption, rest)) = rest.split_once("](") {
                if let Some(blob) = rest.strip_suffix(')') {
                    flush(&mut paragraph, align, &mut blocks);
                    blocks.push(PageBlock::Image {
                        blob: blob.trim().to_string(),
                        caption: caption.to_string(),
                    });
                    continue;
                }
            }
        }
        if let Some(rest) = content
            .strip_prefix("> ")
            .or(content.strip_prefix(">").filter(|r| r.is_empty()))
        {
            flush(&mut paragraph, align, &mut blocks);
            blocks.push(PageBlock::Quote(spans(rest)));
            continue;
        }
        let indent = (line.len() - trimmed.len()) / 2;
        if let Some(rest) = content.strip_prefix("- ").or(content.strip_prefix("* ")) {
            flush(&mut paragraph, align, &mut blocks);
            blocks.push(PageBlock::Bullet(spans(rest), indent.min(4) as u8));
            continue;
        }
        if let Some((number, rest)) = content.split_once(". ") {
            if let Ok(number) = number.parse::<u32>() {
                flush(&mut paragraph, align, &mut blocks);
                blocks.push(PageBlock::Numbered(number, spans(rest)));
                continue;
            }
        }
        if line_align != align {
            flush(&mut paragraph, align, &mut blocks);
            align = line_align;
        }
        paragraph.push(content.to_string());
    }
    flush(&mut paragraph, align, &mut blocks);
    blocks
}

#[derive(Clone)]
struct Style {
    color: Option<[u8; 4]>,
    size: f32,
}

/// Inline markup to styled pieces.
pub fn spans(text: &str) -> Vec<Span> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out: Vec<Span> = Vec::new();
    let mut current = String::new();
    let (mut bold, mut italic, mut underline, mut strike) = (false, false, false, false);
    let mut stack = vec![Style {
        color: None,
        size: 1.0,
    }];
    let mut at = 0;
    let push = |out: &mut Vec<Span>,
                current: &mut String,
                flags: (bool, bool, bool, bool),
                style: &Style,
                code: bool,
                link: Option<String>| {
        if current.is_empty() {
            return;
        }
        out.push(Span {
            text: std::mem::take(current),
            bold: flags.0,
            italic: flags.1,
            underline: flags.2,
            strike: flags.3,
            code,
            color: style.color,
            size: style.size,
            link,
        });
    };
    let starts = |at: usize, pattern: &str| {
        pattern
            .chars()
            .enumerate()
            .all(|(offset, c)| chars.get(at + offset) == Some(&c))
    };
    while at < chars.len() {
        let flags = (bold, italic, underline, strike);
        let style = stack.last().cloned().expect("the base style stays");
        let c = chars[at];
        if c == '\\' && at + 1 < chars.len() {
            current.push(chars[at + 1]);
            at += 2;
            continue;
        }
        let toggles: [(&str, u8); 4] = [("**", 0), ("__", 2), ("~~", 3), ("*", 1)];
        if let Some((pattern, which)) = toggles.iter().find(|(pattern, _)| starts(at, pattern)) {
            // `*` alone needs something after it to start italics.
            push(&mut out, &mut current, flags, &style, false, None);
            match which {
                0 => bold = !bold,
                1 => italic = !italic,
                2 => underline = !underline,
                _ => strike = !strike,
            }
            at += pattern.len();
            continue;
        }
        if c == '`' {
            if let Some(end) = (at + 1..chars.len()).find(|index| chars[*index] == '`') {
                push(&mut out, &mut current, flags, &style, false, None);
                current = chars[at + 1..end].iter().collect();
                push(&mut out, &mut current, flags, &style, true, None);
                at = end + 1;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = (at + 1..chars.len()).find(|index| chars[*index] == ']') {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..chars.len()).find(|index| chars[*index] == ')') {
                        push(&mut out, &mut current, flags, &style, false, None);
                        current = chars[at + 1..close].iter().collect();
                        let url: String = chars[close + 2..end].iter().collect();
                        push(&mut out, &mut current, flags, &style, false, Some(url));
                        at = end + 1;
                        continue;
                    }
                }
            }
        }
        if c == '{' {
            if let Some(end) = (at + 1..chars.len())
                .take(24)
                .find(|index| chars[*index] == '}')
            {
                let word: String = chars[at + 1..end].iter().collect();
                let word = word.trim();
                let change = if word == "/" {
                    Some(None)
                } else {
                    let size = match word {
                        "small" => Some(0.8),
                        "big" => Some(1.35),
                        "huge" => Some(1.9),
                        _ => None,
                    };
                    match (size, parse_color(word)) {
                        (Some(size), _) => Some(Some(Style {
                            size,
                            ..style.clone()
                        })),
                        (None, Some(color)) => Some(Some(Style {
                            color: Some(color),
                            ..style.clone()
                        })),
                        _ => None,
                    }
                };
                if let Some(change) = change {
                    push(&mut out, &mut current, flags, &style, false, None);
                    match change {
                        Some(next) => stack.push(next),
                        None => {
                            if stack.len() > 1 {
                                stack.pop();
                            }
                        }
                    }
                    at = end + 1;
                    continue;
                }
            }
        }
        current.push(c);
        at += 1;
    }
    let style = stack.last().cloned().expect("the base style stays");
    push(
        &mut out,
        &mut current,
        (bold, italic, underline, strike),
        &style,
        false,
        None,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_styles_nest_and_close() {
        let pieces = spans("a **b** {red}c {big}d{/}{/} e `f*` [g](h)");
        let texts = pieces
            .iter()
            .map(|span| span.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, ["a ", "b", " ", "c ", "d", " e ", "f*", " ", "g"]);
        assert!(pieces[1].bold);
        assert_eq!(pieces[3].color, Some([235, 64, 52, 255]));
        assert_eq!(pieces[4].size, 1.35);
        assert_eq!(pieces[4].color, Some([235, 64, 52, 255]));
        assert_eq!(pieces[5].color, None);
        assert!(pieces[6].code);
        assert_eq!(pieces[8].link.as_deref(), Some("h"));
    }

    #[test]
    fn blocks_are_recognised() {
        let page = parse_page(
            "# Title\nsome text\nmore text\n\n- one\n  - two\n1. first\n> careful\n---\n![look](shot)\n->middle<-\n```\ncode here\n```",
        );
        assert!(matches!(&page[0], PageBlock::Heading(1, ..)));
        assert!(
            matches!(&page[1], PageBlock::Paragraph(spans, Align::Left) if spans[0].text == "some text more text")
        );
        assert!(matches!(&page[2], PageBlock::Bullet(_, 0)));
        assert!(matches!(&page[3], PageBlock::Bullet(_, 1)));
        assert!(matches!(&page[4], PageBlock::Numbered(1, _)));
        assert!(matches!(&page[5], PageBlock::Quote(_)));
        assert!(matches!(&page[6], PageBlock::Rule));
        assert!(matches!(&page[7], PageBlock::Image { blob, .. } if blob == "shot"));
        assert!(matches!(&page[8], PageBlock::Paragraph(_, Align::Center)));
        assert!(matches!(&page[9], PageBlock::Code(code) if code == "code here"));
    }
}
