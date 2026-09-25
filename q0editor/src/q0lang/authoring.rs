//! Shared editing tools and context-free diagnostics for every q0lang surface.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::{TextEditOutput, TextEditState};
use egui::{Context, Id, Key, Modifiers, TextEdit};

use super::parser::{Diagnostic, Program};

#[derive(Clone)]
pub(super) struct Analysis {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Default)]
struct AnalysisCache(Vec<(String, Arc<Analysis>)>);

impl AnalysisCache {
    fn get(&mut self, source: &str) -> Arc<Analysis> {
        if let Some(index) = self.0.iter().position(|(cached, _)| cached == source) {
            let entry = self.0.remove(index);
            let result = Arc::clone(&entry.1);
            self.0.push(entry);
            return result;
        }
        let program = super::parser::parse(source);
        let diagnostics = q0s_format::q0lang::compiler::compile(&program).diagnostics;
        let result = Arc::new(Analysis {
            program,
            diagnostics,
        });
        // Small bounded LRU: sharing by exact source is safe because this analysis
        // does not execute code or depend on frame, scene, variables or host state.
        if self.0.len() >= 8 {
            self.0.remove(0);
        }
        self.0.push((source.to_owned(), Arc::clone(&result)));
        result
    }
}

pub(super) fn analyze(ctx: &Context, source: &str) -> Arc<Analysis> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<AnalysisCache>(Id::new("q0lang_analysis_cache"))
            .get(source)
    })
}

#[derive(Clone, Default)]
struct SearchState {
    open: bool,
    replace: bool,
    query: String,
    replacement: String,
    line: String,
    show_line: bool,
    message: String,
    source: String,
    matched_query: String,
    matches: Vec<Range<usize>>,
}

impl SearchState {
    fn refresh(&mut self, text: &str) {
        if self.source != text || self.matched_query != self.query {
            self.matches = match_ranges(text, &self.query);
            self.source.clear();
            self.source.push_str(text);
            self.matched_query.clone_from(&self.query);
        }
    }
}

/// Literal, case-sensitive matches in egui character coordinates, never bytes.
fn match_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    let needle_chars = needle.chars().count();
    for (byte, _) in text.match_indices(needle) {
        let start = previous_char + text[previous_byte..byte].chars().count();
        ranges.push(start..start + needle_chars);
        previous_byte = byte + needle.len();
        previous_char = start + needle_chars;
    }
    ranges
}

fn next_match(
    matches: &[Range<usize>],
    selection: Range<usize>,
    forward: bool,
) -> Option<Range<usize>> {
    if forward {
        matches
            .iter()
            .find(|range| range.start >= selection.end)
            .or(matches.first())
            .cloned()
    } else {
        matches
            .iter()
            .rev()
            .find(|range| range.end <= selection.start)
            .or(matches.last())
            .cloned()
    }
}

fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn selected_range(ctx: &Context, id: Id) -> Range<usize> {
    TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| {
            range.primary.index.min(range.secondary.index)
                ..range.primary.index.max(range.secondary.index)
        })
        .unwrap_or(0..0)
}

pub(super) fn select_range(ctx: &Context, id: Id, range: Range<usize>) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(range.start),
        CCursor::new(range.end),
    )));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
    ctx.data_mut(|data| data.insert_temp(id.with("scroll_selection"), range.start));
    ctx.request_repaint();
}

pub(super) fn scroll_to_selection(ui: &egui::Ui, output: &TextEditOutput) {
    if let Some(index) = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<usize>(output.response.id.with("scroll_selection")))
    {
        let rect = output
            .galley
            .pos_from_ccursor(CCursor::new(index))
            .translate(output.galley_pos.to_vec2());
        ui.scroll_to_rect(rect.expand(8.0), Some(egui::Align::Center));
    }
}

fn line_range(text: &str, line: usize) -> Option<Range<usize>> {
    if line == 0 {
        return None;
    }
    let mut start = 0;
    for (index, content) in text.split('\n').enumerate() {
        let length = content.chars().count();
        if index + 1 == line {
            return Some(start..start + length - usize::from(content.ends_with('\r')));
        }
        start += length + 1;
    }
    None
}

pub(super) fn select_line(ctx: &Context, id: Id, text: &str, line: usize) -> bool {
    if let Some(range) = line_range(text, line) {
        select_range(ctx, id, range);
        true
    } else {
        false
    }
}

fn replace_selection(
    text: &mut String,
    needle: &str,
    replacement: &str,
    selection: Range<usize>,
) -> Option<Range<usize>> {
    if needle.is_empty() || selection.is_empty() {
        return None;
    }
    let bytes = char_to_byte(text, selection.start)..char_to_byte(text, selection.end);
    if &text[bytes.clone()] != needle {
        return None;
    }
    text.replace_range(bytes, replacement);
    Some(selection.start..selection.start + replacement.chars().count())
}

fn replace_all(text: &mut String, needle: &str, replacement: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let count = text.matches(needle).count();
    if count > 0 {
        *text = text.replace(needle, replacement);
    }
    count
}

fn navigate(ctx: &Context, id: Id, state: &mut SearchState, text: &str, forward: bool) {
    state.refresh(text);
    if let Some(range) = next_match(&state.matches, selected_range(ctx, id), forward) {
        select_range(ctx, id, range);
        state.message.clear();
    } else {
        state.message = if state.query.is_empty() {
            "type text to find"
        } else {
            "no matches"
        }
        .to_owned();
    }
}

/// Returns true only if replacement changes the buffer. State is scoped to the
/// stable editor id, so opening two script windows never shares a search cursor.
pub(super) fn tools_ui(ui: &mut egui::Ui, id: Id, text: &mut String) -> bool {
    let ctx = ui.ctx().clone();
    let state_id = id.with("authoring_tools");
    let find_id = id.with("find_query");
    let replace_id = id.with("replace_query");
    let line_id = id.with("line_query");
    let mut state = ctx
        .data_mut(|data| data.remove_temp::<SearchState>(state_id))
        .unwrap_or_default();
    let active = ctx.memory(|memory| {
        [id, find_id, replace_id, line_id]
            .iter()
            .any(|id| memory.has_focus(*id))
    });
    let (mut find, mut replace, mut line, next, prev, escape) = if active {
        ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::COMMAND, Key::F),
                input.consume_key(Modifiers::COMMAND, Key::H),
                input.consume_key(Modifiers::COMMAND, Key::G),
                input.consume_key(Modifiers::NONE, Key::F3),
                input.consume_key(Modifiers::SHIFT, Key::F3),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        })
    } else {
        (false, false, false, false, false, false)
    };
    let mut changed = false;
    ui.push_id(id.with("tools"), |ui| {
        ui.horizontal_wrapped(|ui| {
            find |= ui
                .small_button("find")
                .on_hover_text("ctrl+f; f3 next; shift+f3 previous")
                .clicked();
            replace |= ui.small_button("replace").on_hover_text("ctrl+h").clicked();
            line |= ui
                .small_button("go to line")
                .on_hover_text("ctrl+g")
                .clicked();
        });
        if find || replace {
            state.open = true;
            state.replace = replace;
            state.show_line = false;
            state.message.clear();
            let selected = selected_range(&ctx, id);
            let seed = &text[char_to_byte(text, selected.start)..char_to_byte(text, selected.end)];
            if !seed.is_empty() && !seed.contains('\n') && seed.chars().count() <= 256 {
                state.query = seed.to_owned();
            }
        }
        if line {
            state.show_line = true;
            state.line.clear();
            state.message.clear();
        }
        if escape {
            state.open = false;
            state.show_line = false;
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        if next || prev {
            navigate(&ctx, id, &mut state, text, !prev);
        }
        if state.open {
            ui.horizontal_wrapped(|ui| {
                ui.label("find");
                let response = ui.add(
                    TextEdit::singleline(&mut state.query)
                        .id(find_id)
                        .desired_width(190.0)
                        .hint_text("literal text, case sensitive"),
                );
                if find || replace {
                    response.request_focus();
                }
                if response.changed() {
                    state.message.clear();
                }
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                state.refresh(text);
                let selected = selected_range(&ctx, id);
                let current = state
                    .matches
                    .iter()
                    .position(|range| *range == selected)
                    .map(|index| index + 1);
                ui.label(
                    current
                        .map(|index| format!("{index} / {}", state.matches.len()))
                        .unwrap_or_else(|| format!("{} matches", state.matches.len())),
                );
                if ui
                    .add_enabled(!state.matches.is_empty(), egui::Button::new("prev").small())
                    .clicked()
                {
                    navigate(&ctx, id, &mut state, text, false);
                }
                if ui
                    .add_enabled(!state.matches.is_empty(), egui::Button::new("next").small())
                    .clicked()
                    || enter
                {
                    navigate(&ctx, id, &mut state, text, true);
                }
                ui.checkbox(&mut state.replace, "replace");
                if ui.small_button("close search").clicked() {
                    state.open = false;
                    ctx.memory_mut(|memory| memory.request_focus(id));
                }
            });
            if state.replace {
                ui.horizontal_wrapped(|ui| {
                    ui.label("with");
                    ui.add(
                        TextEdit::singleline(&mut state.replacement)
                            .id(replace_id)
                            .desired_width(190.0)
                            .hint_text("replacement text"),
                    );
                    let has_matches = !state.matches.is_empty();
                    if ui
                        .add_enabled(has_matches, egui::Button::new("replace one").small())
                        .clicked()
                    {
                        if let Some(inserted) = replace_selection(
                            text,
                            &state.query,
                            &state.replacement,
                            selected_range(&ctx, id),
                        ) {
                            changed = state.query != state.replacement;
                            select_range(&ctx, id, inserted.end..inserted.end);
                            navigate(&ctx, id, &mut state, text, true);
                            state.message = "replaced 1 match".to_owned();
                        } else {
                            navigate(&ctx, id, &mut state, text, true);
                            state.message = "match selected; replace one to change it".to_owned();
                        }
                    }
                    if ui
                        .add_enabled(has_matches, egui::Button::new("replace all").small())
                        .clicked()
                    {
                        let count = replace_all(text, &state.query, &state.replacement);
                        changed = count > 0 && state.query != state.replacement;
                        state.refresh(text);
                        let cursor = selected_range(&ctx, id).start.min(text.chars().count());
                        select_range(&ctx, id, cursor..cursor);
                        state.message = format!("replaced {count} matches");
                    }
                });
            }
        }
        if state.show_line {
            ui.horizontal(|ui| {
                ui.label("line");
                let response = ui.add(
                    TextEdit::singleline(&mut state.line)
                        .id(line_id)
                        .desired_width(90.0)
                        .hint_text("1"),
                );
                if line {
                    response.request_focus();
                }
                if ui.small_button("go").clicked()
                    || (response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                {
                    if state
                        .line
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .is_some_and(|line| select_line(&ctx, id, text, line))
                    {
                        state.show_line = false;
                        state.message.clear();
                    } else {
                        state.message =
                            format!("enter a line from 1 to {}", text.split('\n').count());
                    }
                }
                if ui.small_button("close").clicked() {
                    state.show_line = false;
                }
            });
        }
        if (state.open || state.show_line) && !state.message.is_empty() {
            ui.small(&state.message);
        }
    });
    ctx.data_mut(|data| data.insert_temp(state_id, state));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_matches_use_character_coordinates_and_wrap_both_ways() {
        let matches = match_ranges("я 🌙 я 🌙", "🌙");
        assert_eq!(matches, vec![2..3, 6..7]);
        assert_eq!(next_match(&matches, 2..3, true), Some(6..7));
        assert_eq!(next_match(&matches, 6..7, true), Some(2..3));
        assert_eq!(next_match(&matches, 6..7, false), Some(2..3));
        assert_eq!(next_match(&matches, 2..3, false), Some(6..7));
    }

    #[test]
    fn literal_numeric_search_is_not_a_line_command() {
        assert_eq!(
            match_ranges("frame = 12\nx = 12", "12"),
            vec![8..10, 15..17]
        );
        assert!(match_ranges("abc", "").is_empty());
        assert!(match_ranges("abc", "ABC").is_empty());
        assert_eq!(match_ranges("aaaaa", "aa"), vec![0..2, 2..4]);
    }

    #[test]
    fn replacement_requires_the_exact_selection_and_preserves_unicode() {
        let mut text = "я 🌙 я 🌙".to_owned();
        assert_eq!(replace_selection(&mut text, "🌙", "звезда", 0..1), None);
        assert_eq!(text, "я 🌙 я 🌙");
        assert_eq!(
            replace_selection(&mut text, "🌙", "звезда", 2..3),
            Some(2..8)
        );
        assert_eq!(text, "я звезда я 🌙");
        assert_eq!(replace_all(&mut text, "я", "🐈"), 2);
        assert_eq!(text, "🐈 звезда 🐈 🌙");
        assert_eq!(replace_all(&mut text, "", "!"), 0);
        assert_eq!(replace_all(&mut text, "🐈", ""), 2);
        assert_eq!(text, " звезда  🌙");
    }

    #[test]
    fn line_navigation_handles_empty_trailing_crlf_and_unicode_lines() {
        assert_eq!(line_range("", 1), Some(0..0));
        assert_eq!(line_range("я\r\n🌙\n", 1), Some(0..1));
        assert_eq!(line_range("я\r\n🌙\n", 2), Some(3..4));
        assert_eq!(line_range("я\r\n🌙\n", 3), Some(5..5));
        assert_eq!(line_range("я\n", 0), None);
        assert_eq!(line_range("я\n", 3), None);
    }

    #[test]
    fn analysis_reuses_exact_source_and_invalidates_on_edits() {
        let mut cache = AnalysisCache::default();
        let first = cache.get("x = 1");
        assert!(Arc::ptr_eq(&first, &cache.get("x = 1")));
        let broken = cache.get("if {");
        assert!(!broken.diagnostics.is_empty());
        assert!(cache.get("x = 1").diagnostics.is_empty());
        assert!(!Arc::ptr_eq(&first, &cache.get("x = 2")));
        for n in 0..20 {
            cache.get(&format!("x = {n}"));
        }
        assert_eq!(cache.0.len(), 8);
    }
}
