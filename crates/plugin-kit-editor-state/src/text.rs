//! A field and its character caret, from the CA-72's and CA-74's preset fields (R10,
//! R18), with selection for their typed knob values (R-KNOB-ENTRY). Positions count
//! Unicode scalar values, not bytes or grapheme clusters. Measuring text is the caller's job.

use std::ops::Range;

/// An editing command after the caller has translated its window's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEdit<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    SelectAll,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    text: String,
    caret: usize,
    anchor: Option<usize>,
}

impl TextField {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            caret: text.chars().count(),
            text,
            anchor: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| anchor.min(self.caret)..anchor.max(self.caret))
    }

    /// Place the caret at a character, clipped to the field, ending any selection.
    pub fn set_caret(&mut self, character: usize) {
        self.caret = character.min(self.text.chars().count());
        self.anchor = None;
    }

    /// Select between two character positions, clipped to the field.
    pub fn select(&mut self, anchor: usize, caret: usize) {
        self.set_caret(caret);
        self.anchor = Some(anchor.min(self.text.chars().count()));
    }

    fn byte(&self, character: usize) -> usize {
        self.text
            .char_indices()
            .nth(character)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn remove(&mut self, range: Range<usize>) {
        let bytes = self.byte(range.start)..self.byte(range.end);
        self.text.replace_range(bytes, "");
        self.caret = range.start;
        self.anchor = None;
    }

    /// Insert printable characters at the caret, replacing a selection. Control
    /// characters are ignored, as in the source preset fields. The return value says
    /// that editing input was accepted, even when it replaces text with identical text.
    /// There is no length limit or prefix truncation; validation belongs to the caller.
    pub fn insert(&mut self, text: &str) -> bool {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        if let Some(range) = self.selection() {
            self.remove(range);
        }
        let at = self.byte(self.caret);
        self.text.insert_str(at, &clean);
        self.caret += clean.chars().count();
        self.anchor = None;
        true
    }

    pub fn backspace(&mut self) -> bool {
        let range = self
            .selection()
            .or_else(|| self.caret.checked_sub(1).map(|start| start..self.caret));
        if let Some(range) = range {
            self.remove(range);
            return true;
        }
        false
    }

    pub fn delete(&mut self) -> bool {
        let range = self.selection().or_else(|| {
            (self.caret < self.text.chars().count()).then_some(self.caret..self.caret + 1)
        });
        if let Some(range) = range {
            self.remove(range);
            return true;
        }
        false
    }

    /// Apply a command. Returns true for accepted editing input; caret movement and
    /// selection alone return false, so opening a rounded value need not change it.
    pub fn edit(&mut self, edit: TextEdit<'_>) -> bool {
        match edit {
            TextEdit::Insert(text) => return self.insert(text),
            TextEdit::Backspace => return self.backspace(),
            TextEdit::Delete => return self.delete(),
            TextEdit::Left => {
                let at = self
                    .selection()
                    .map_or(self.caret.saturating_sub(1), |s| s.start);
                self.set_caret(at);
            }
            TextEdit::Right => {
                let at = self
                    .selection()
                    .map_or(self.caret.saturating_add(1), |s| s.end);
                self.set_caret(at);
            }
            TextEdit::Home => self.set_caret(0),
            TextEdit::End => self.set_caret(self.text.chars().count()),
            TextEdit::SelectAll => self.select(0, self.text.chars().count()),
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_edits_at_its_character_caret() {
        let mut f = TextField::new("Bàss");
        f.edit(TextEdit::Left);
        f.backspace();
        assert_eq!((f.text(), f.caret()), ("Bàs", 2));
        f.insert("ü\u{7}");
        assert_eq!((f.text(), f.caret()), ("Bàüs", 3));
        f.edit(TextEdit::Home);
        f.delete();
        f.edit(TextEdit::End);
        f.insert("!");
        assert_eq!(f.text(), "àüs!");
    }

    #[test]
    fn selection_replaces_and_deletes_whole_characters() {
        let mut f = TextField::new("Aé🦀中Z");
        f.select(4, 1);
        assert_eq!(f.selection(), Some(1..4));
        f.insert("Ω🐈");
        assert_eq!((f.text(), f.caret()), ("AΩ🐈Z", 3));
        f.select(1, 3);
        assert!(f.backspace());
        assert_eq!((f.text(), f.caret()), ("AZ", 1));
        f.edit(TextEdit::SelectAll);
        assert!(f.delete());
        assert_eq!((f.text(), f.caret()), ("", 0));
        assert!(!f.backspace() && !f.delete());
    }

    #[test]
    fn caret_movement_collapses_selection_and_never_uses_bytes() {
        let mut f = TextField::new("e\u{301}🦀");
        for want in [2, 1, 0, 0] {
            assert!(!f.edit(TextEdit::Left));
            assert_eq!(f.caret(), want);
        }
        for want in [1, 2, 3, 3] {
            f.edit(TextEdit::Right);
            assert_eq!(f.caret(), want);
        }
        f.edit(TextEdit::SelectAll);
        f.edit(TextEdit::Left);
        assert_eq!((f.caret(), f.selection()), (0, None));
        f.select(0, 3);
        f.edit(TextEdit::Right);
        assert_eq!((f.caret(), f.selection()), (3, None));
        f.set_caret(usize::MAX);
        f.backspace();
        assert_eq!(f.text(), "e\u{301}");
        f.backspace();
        assert_eq!(f.text(), "e");
    }

    #[test]
    fn ignored_controls_leave_the_selection_and_long_input_is_whole() {
        let mut f = TextField::new("selected");
        f.edit(TextEdit::SelectAll);
        assert!(!f.insert("\n\r\t\0"));
        assert_eq!(f.selection(), Some(0..8));
        let long = "é".repeat(256);
        assert!(f.insert(&long));
        assert_eq!((f.text(), f.caret()), (long.as_str(), 256));
    }
}
