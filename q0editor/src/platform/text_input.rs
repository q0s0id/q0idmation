//! Text entry on platforms where typing does not arrive as key presses.

use egui::{Event, Key, Modifiers};

/// An on-screen keyboard driven by the editor's text fields.
pub trait SoftKeyboard: Send {
    /// Runs once per frame, before `input` is handled. Shows or hides the
    /// keyboard to match `text_field_focused`, appends whatever was typed
    /// since the last frame, and takes the area the keyboard covers out of
    /// the screen rectangle.
    fn update(&mut self, ctx: &egui::Context, text_field_focused: bool, input: &mut egui::RawInput);
}

/// One step an input method reports while the user types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeAction {
    /// Text still being composed. Replaces the previous composing text.
    Compose(String),
    /// Finished text. Replaces the composing text, if there is any.
    Commit(String),
    /// Composition is over and the composing text stays as it is.
    FinishComposing,
    /// Remove this many characters before and after the cursor.
    Delete { before: usize, after: usize },
}

/// Turns [`ImeAction`]s into the events egui's text fields understand.
#[derive(Default)]
pub struct ImeTranslator {
    composing: Option<String>,
}

impl ImeTranslator {
    pub fn translate(&mut self, action: ImeAction, events: &mut Vec<Event>) {
        match action {
            ImeAction::Compose(text) => {
                if self.composing.is_none() {
                    events.push(Event::CompositionStart);
                }
                events.push(Event::CompositionUpdate(text.clone()));
                self.composing = Some(text);
            }
            ImeAction::Commit(text) if text.is_empty() => {
                if self.composing.take().is_some() {
                    // egui keeps the composing text when a composition ends on
                    // nothing, so an abandoned one has to be cleared first.
                    events.push(Event::CompositionUpdate(String::new()));
                    events.push(Event::CompositionEnd(String::new()));
                }
            }
            ImeAction::Commit(text) => {
                // Input methods send Enter as a line break, while text fields
                // expect the key.
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        push_key(Key::Enter, events);
                    }
                    if line.is_empty() {
                        // A bare line break confirms what is being composed.
                        self.finish_composing(events);
                    } else if self.composing.take().is_some() {
                        events.push(Event::CompositionEnd(line.to_owned()));
                    } else {
                        events.push(Event::Text(line.to_owned()));
                    }
                }
            }
            ImeAction::FinishComposing => self.finish_composing(events),
            ImeAction::Delete { before, after } => {
                for _ in 0..before {
                    push_key(Key::Backspace, events);
                }
                for _ in 0..after {
                    push_key(Key::Delete, events);
                }
            }
        }
    }

    fn finish_composing(&mut self, events: &mut Vec<Event>) {
        if let Some(text) = self.composing.take() {
            events.push(Event::CompositionEnd(text));
        }
    }
}

fn push_key(key: Key, events: &mut Vec<Event>) {
    for pressed in [true, false] {
        events.push(Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(actions: impl IntoIterator<Item = ImeAction>) -> Vec<Event> {
        let mut translator = ImeTranslator::default();
        let mut events = Vec::new();
        for action in actions {
            translator.translate(action, &mut events);
        }
        events
    }

    fn key_tap(key: Key) -> [Event; 2] {
        [true, false].map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
    }

    #[test]
    fn plain_commit_is_typed_text() {
        assert_eq!(
            translate([ImeAction::Commit("привет".into())]),
            vec![Event::Text("привет".into())]
        );
    }

    #[test]
    fn composition_is_started_once_and_replaced_by_its_commit() {
        assert_eq!(
            translate([
                ImeAction::Compose("n".into()),
                ImeAction::Compose("ni".into()),
                ImeAction::Commit("你".into()),
                ImeAction::Commit("!".into()),
            ]),
            vec![
                Event::CompositionStart,
                Event::CompositionUpdate("n".into()),
                Event::CompositionUpdate("ni".into()),
                Event::CompositionEnd("你".into()),
                Event::Text("!".into()),
            ]
        );
    }

    #[test]
    fn finishing_keeps_the_composing_text_and_is_idle_otherwise() {
        assert_eq!(
            translate([
                ImeAction::FinishComposing,
                ImeAction::Compose("ab".into()),
                ImeAction::FinishComposing,
                ImeAction::FinishComposing,
            ]),
            vec![
                Event::CompositionStart,
                Event::CompositionUpdate("ab".into()),
                Event::CompositionEnd("ab".into()),
            ]
        );
    }

    #[test]
    fn abandoned_composition_is_cleared() {
        assert_eq!(
            translate([
                ImeAction::Compose("ab".into()),
                ImeAction::Commit(String::new()),
            ]),
            vec![
                Event::CompositionStart,
                Event::CompositionUpdate("ab".into()),
                Event::CompositionUpdate(String::new()),
                Event::CompositionEnd(String::new()),
            ]
        );
    }

    #[test]
    fn line_break_confirms_the_composing_text_before_enter() {
        let mut expected = vec![
            Event::CompositionStart,
            Event::CompositionUpdate("Demo".into()),
            Event::CompositionEnd("Demo".into()),
        ];
        expected.extend(key_tap(Key::Enter));
        assert_eq!(
            translate([
                ImeAction::Compose("Demo".into()),
                ImeAction::Commit("\n".into()),
            ]),
            expected
        );
    }

    #[test]
    fn line_breaks_become_enter_and_deletes_become_keys() {
        let mut expected = vec![Event::Text("a".into())];
        expected.extend(key_tap(Key::Enter));
        expected.push(Event::Text("b".into()));
        expected.extend(key_tap(Key::Backspace));
        expected.extend(key_tap(Key::Backspace));
        expected.extend(key_tap(Key::Delete));
        assert_eq!(
            translate([
                ImeAction::Commit("a\nb".into()),
                ImeAction::Delete {
                    before: 2,
                    after: 1
                },
            ]),
            expected
        );
    }
}
