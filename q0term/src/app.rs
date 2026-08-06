use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange};
use egui::{Key, RichText, ScrollArea, TextEdit, Ui};
use q0shell::{ShellError, ShellHost};

use crate::theme::{self, DIM, PROMPT_HOST, PROMPT_PATH, PROMPT_USER, RED, RED_BRIGHT, RED_ERROR};

const BUILTINS: &[&str] = &["clear", "help", "pwd"];
const Q0SHELL_CMDS: &[&str] = &[
    "q0shell.append!",
    "q0shell.delete!",
    "q0shell.list!",
    "q0shell.mkdir!",
    "q0shell.move!",
    "q0shell.print!",
    "q0shell.read!",
    "q0shell.write!",
];
const SNIPPETS: &[&str] = &["import q0.math", "import q0shell"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Out,
    Err,
    Dim,
}

struct LogLine {
    kind: LineKind,
    text: String,
}

pub struct TermApp {
    host: ShellHost,
    cwd_label: String,
    log: Vec<LogLine>,
    input: String,
    history: Vec<String>,
    history_pos: Option<usize>,
    follow_tail: bool,
    focus_input: bool,
    pending_cursor: Option<usize>,
}

impl TermApp {
    pub fn new() -> Self {
        let root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let host = ShellHost::new(root).expect("q0term host root");
        let cwd_label = format_cwd(host.root());
        let mut app = Self {
            host,
            cwd_label,
            log: Vec::new(),
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            follow_tail: true,
            focus_input: true,
            pending_cursor: None,
        };
        app.push_dim("Q0S q0lang shell. Type 'help'. Press Tab to complete.");
        app
    }

    fn prompt(&self) -> String {
        format!("q0@q0term:{}$ ", self.cwd_label)
    }

    fn set_cursor_end(&mut self) {
        self.pending_cursor = Some(self.input.chars().count());
    }

    fn bump_scroll(&mut self) {
        self.follow_tail = true;
    }

    fn push_out(&mut self, text: impl Into<String>) {
        self.log.push(LogLine {
            kind: LineKind::Out,
            text: text.into(),
        });
        self.bump_scroll();
    }

    fn push_err(&mut self, text: impl Into<String>) {
        self.log.push(LogLine {
            kind: LineKind::Err,
            text: text.into(),
        });
        self.bump_scroll();
    }

    fn push_dim(&mut self, text: impl Into<String>) {
        self.log.push(LogLine {
            kind: LineKind::Dim,
            text: text.into(),
        });
        self.bump_scroll();
    }

    fn clear_screen(&mut self) {
        self.log.clear();
        self.bump_scroll();
    }

    fn run_line(&mut self, source: String) {
        let source = source.trim().to_string();
        if source.is_empty() {
            return;
        }

        self.push_out(format!("{}{}", self.prompt(), source));

        if self.history.last().map(|s| s.as_str()) != Some(source.as_str()) {
            self.history.push(source.clone());
        }
        self.history_pos = None;

        match source.as_str() {
            "clear" => {
                self.clear_screen();
                return;
            }
            "help" => {
                self.print_help();
                return;
            }
            "pwd" => {
                self.push_out(self.host.root().display().to_string());
                return;
            }
            _ => {}
        }

        match self.host.run_source(&source) {
            Ok(lines) => {
                for line in lines {
                    self.push_out(line);
                }
            }
            Err(ShellError::Diagnostics { log, .. }) => {
                for line in log {
                    self.push_err(line);
                }
            }
            Err(err) => self.push_err(err.to_string()),
        }
    }

    fn print_help(&mut self) {
        self.push_dim("Builtins: clear, help, pwd");
        self.push_dim(
            "q0lang: q0shell.print!, read!, write!, append!, mkdir!, list!, move!, delete!",
        );
        self.push_dim("Example: q0shell.print! 'hello world'");
        self.push_dim(
            "History: Up/Down. Tab: complete. End: scroll down. Ctrl+L or clear: clear screen.",
        );
    }

    fn handle_history_nav(&mut self, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let len = self.history.len();
        let pos = match (self.history_pos, up) {
            (Some(p), true) => p.saturating_sub(1),
            (Some(p), false) => p + 1,
            (None, true) => len - 1,
            (None, false) => return,
        };
        if pos >= len {
            self.history_pos = None;
            self.input.clear();
        } else {
            self.history_pos = Some(pos);
            self.input = self.history[pos].clone();
        }
        self.set_cursor_end();
    }

    fn tab_complete(&mut self) {
        self.history_pos = None;
        let (token, start) = current_token(&self.input);
        if token.is_empty() {
            self.push_dim(
                [BUILTINS, Q0SHELL_CMDS, SNIPPETS]
                    .into_iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>()
                    .join("  "),
            );
            return;
        }

        let mut matches = self.completion_candidates(token);
        matches.sort();
        matches.dedup();

        if matches.is_empty() {
            return;
        }

        if matches.len() == 1 {
            let m = matches[0].clone();
            self.input.replace_range(start.., &m);
            if should_append_space(&m) && !self.input.ends_with(' ') {
                self.input.push(' ');
            }
            self.set_cursor_end();
            return;
        }

        let common = longest_common_prefix(&matches);
        if common.len() > token.len() || starts_with_ignore_ascii_case(&common, token) {
            self.input.replace_range(start.., &common);
            self.set_cursor_end();
        } else {
            self.push_dim(matches.join("  "));
        }
    }

    fn completion_candidates(&self, token: &str) -> Vec<String> {
        let mut out = Vec::new();

        for item in BUILTINS
            .iter()
            .chain(Q0SHELL_CMDS.iter())
            .chain(SNIPPETS.iter())
            .copied()
        {
            if starts_with_ignore_ascii_case(item, token) {
                out.push(item.to_string());
            }
        }

        for line in &self.history {
            if starts_with_ignore_ascii_case(line, token) {
                out.push(line.clone());
            }
        }

        if looks_like_path_token(token) {
            out.extend(path_completions(self.host.root(), token));
        }

        out
    }
}

impl eframe::App for TermApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::L)) {
            self.clear_screen();
        }
        if ctx.input(|i| i.key_pressed(Key::End)) {
            self.follow_tail = true;
        }
        if ctx.input(|i| i.raw_scroll_delta.y > 0.0) {
            self.follow_tail = false;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG))
            .show(ctx, |ui| {
                self.render_terminal(ui);
            });
    }
}

impl TermApp {
    fn render_terminal(&mut self, ui: &mut Ui) {
        let _output = ScrollArea::vertical()
            .id_source("q0term_scroll")
            .stick_to_bottom(self.follow_tail)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                for line in &self.log {
                    let (color, font) = match line.kind {
                        LineKind::Out => (RED, theme::mono(15.0)),
                        LineKind::Err => (RED_ERROR, theme::mono(15.0)),
                        LineKind::Dim => (DIM, theme::mono(14.0)),
                    };
                    ui.label(RichText::new(&line.text).font(font).color(color));
                }

                self.render_prompt_line(ui);

                if self.follow_tail {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
            });
    }

    fn render_prompt_line(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            ui.label(
                RichText::new("q0@")
                    .font(theme::mono(15.0))
                    .color(PROMPT_USER),
            );
            ui.label(
                RichText::new("q0term")
                    .font(theme::mono(15.0))
                    .color(PROMPT_HOST),
            );
            ui.label(
                RichText::new(":")
                    .font(theme::mono(15.0))
                    .color(PROMPT_USER),
            );
            ui.label(
                RichText::new(&self.cwd_label)
                    .font(theme::mono(15.0))
                    .color(PROMPT_PATH),
            );
            ui.label(
                RichText::new("$ ")
                    .font(theme::mono(15.0))
                    .color(RED_BRIGHT),
            );

            let input_width = ui.available_width().max(80.0);
            let output = TextEdit::singleline(&mut self.input)
                .id(egui::Id::new("q0term_input"))
                .frame(false)
                .font(theme::mono(15.0))
                .text_color(RED)
                .desired_width(input_width)
                .show(ui);

            let response = &output.response;

            if self.focus_input {
                response.request_focus();
                self.focus_input = false;
            } else if !response.has_focus() {
                response.request_focus();
            }

            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let up = ui.input(|i| i.key_pressed(Key::ArrowUp));
            let down = ui.input(|i| i.key_pressed(Key::ArrowDown));
            let tab = ui.input(|i| i.key_pressed(Key::Tab));

            if tab && response.has_focus() {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
                while self.input.ends_with('\t') {
                    self.input.pop();
                }
                self.tab_complete();
            } else if up && response.has_focus() {
                self.handle_history_nav(true);
            } else if down && response.has_focus() {
                self.handle_history_nav(false);
            } else if enter && response.has_focus() {
                let line = std::mem::take(&mut self.input);
                self.bump_scroll();
                self.run_line(line);
            }

            if let Some(pos) = self.pending_cursor.take() {
                let idx = pos.min(self.input.chars().count());
                let mut state = output.state.clone();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
                state.store(ui.ctx(), output.response.id);
                output.response.request_focus();
            }
        });
    }
}

fn current_token(input: &str) -> (&str, usize) {
    if let Some(i) = input.rfind(|c: char| c.is_whitespace()) {
        let start = i + 1;
        (&input[start..], start)
    } else {
        (input, 0)
    }
}

fn starts_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .get(..needle.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(needle))
}

fn longest_common_prefix(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let first: Vec<char> = items[0].chars().collect();
    let mut len = 0usize;
    'outer: for (i, &ch) in first.iter().enumerate() {
        for item in items.iter().skip(1) {
            match item.chars().nth(i) {
                Some(c) if c.eq_ignore_ascii_case(&ch) => {}
                _ => break 'outer,
            }
        }
        len = i + 1;
    }
    first.iter().take(len).collect()
}

fn should_append_space(completion: &str) -> bool {
    completion.ends_with('!') || BUILTINS.contains(&completion) || SNIPPETS.contains(&completion)
}

fn looks_like_path_token(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with("q0shell.")
        && !token.starts_with("import")
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '\\'))
}

fn path_completions(root: &Path, token: &str) -> Vec<String> {
    let token = token.trim_matches('"').trim_matches('\'');
    let path = Path::new(token);
    let (dir, prefix) = if token.is_empty() {
        (root.to_path_buf(), String::new())
    } else if token.ends_with('/') || token.ends_with('\\') {
        (root.join(path), String::new())
    } else if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        (
            root.join(parent),
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(token)
                .to_string(),
        )
    } else {
        (root.to_path_buf(), token.to_string())
    };

    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if prefix.is_empty() || starts_with_ignore_ascii_case(&name, &prefix) {
            let mut item = name;
            if entry.path().is_dir() {
                item.push('/');
            }
            out.push(item);
        }
    }
    out
}

fn format_cwd(path: &Path) -> String {
    let raw = path.display().to_string();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);

    if let Ok(home) = env::var("USERPROFILE").or_else(|_| env::var("HOME")) {
        let home = home.strip_prefix(r"\\?\").unwrap_or(&home);
        if stripped.eq_ignore_ascii_case(home) {
            return "~".to_string();
        }
        if let Some(rest) = stripped.strip_prefix(home) {
            let rest = rest.trim_start_matches(['\\', '/']);
            if !rest.is_empty() {
                return format!("~/{rest}");
            }
        }
    }

    if stripped.len() > 48 {
        format!("...{}", &stripped[stripped.len() - 45..])
    } else {
        stripped.to_string()
    }
}
