//! Typed values without a parameter, unit or drawn field. From the CA-72's R52 and
//! R-KNOB-ENTRY and the CA-74's `editor::entry`. The adapter decides which controls
//! take values, translates keys and applies one returned value through its host.

use crate::text::{TextEdit, TextField};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Editing<T> {
    target: T,
    field: TextField,
    edited: bool,
}

/// One active field and the preceding click. The target is the caller's stable
/// control identity. An unedited displayed value never replaces its precise value.
#[derive(Clone, Debug)]
pub struct ValueEntry<T> {
    active: Option<Editing<T>>,
    last_click: Option<(T, Instant)>,
}

impl<T> Default for ValueEntry<T> {
    fn default() -> Self {
        Self {
            active: None,
            last_click: None,
        }
    }
}

impl<T> ValueEntry<T> {
    /// Begin directly, for example from a modifier click. `selected` selects the
    /// initial value for replacement; a readout may instead begin with empty text.
    pub fn begin(&mut self, target: T, text: impl Into<String>, selected: bool) {
        let mut field = TextField::new(text);
        if selected {
            field.edit(TextEdit::SelectAll);
        }
        self.active = Some(Editing {
            target,
            field,
            edited: false,
        });
        self.last_click = None;
    }

    pub fn field(&self) -> Option<&TextField> {
        self.active.as_ref().map(|e| &e.field)
    }

    pub fn target(&self) -> Option<&T> {
        self.active.as_ref().map(|e| &e.target)
    }

    pub fn edited(&self) -> bool {
        self.active.as_ref().is_some_and(|e| e.edited)
    }

    /// Returns false when no value is being entered. A caller captures all window
    /// keys while `field()` is present, including keys it does not translate here.
    pub fn edit(&mut self, edit: TextEdit<'_>) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        active.edited |= active.field.edit(edit);
        true
    }

    /// Escape, an outside press, focus loss or window closure sets nothing.
    pub fn cancel(&mut self) {
        self.active = None;
        self.last_click = None;
    }

    /// Enter ends the field. Only edited, successfully parsed text returns a value,
    /// passed through the caller's clamp. The parser must validate the whole text
    /// and reject non-finite numbers where they are not meaningful. No gesture or
    /// parameter change occurs here; the caller applies the result once.
    pub fn commit<V>(
        &mut self,
        parse: impl FnOnce(&str) -> Option<V>,
        clamp: impl FnOnce(V) -> V,
    ) -> Option<(T, V)> {
        self.last_click = None;
        let active = self.active.take()?;
        if !active.edited {
            return None;
        }
        let value = clamp(parse(active.field.text())?);
        Some((active.target, value))
    }
}

impl<T: PartialEq> ValueEntry<T> {
    /// Two clicks on the same target less than `interval` apart begin entry.
    /// Timestamps and the interval are supplied by the caller; a backwards timestamp
    /// is not a double click. A successful pair is consumed before the next click.
    pub fn click(
        &mut self,
        target: T,
        now: Instant,
        interval: Duration,
        text: impl Into<String>,
        selected: bool,
    ) -> bool {
        let double = self.last_click.as_ref().is_some_and(|(previous, then)| {
            *previous == target
                && now
                    .checked_duration_since(*then)
                    .is_some_and(|d| d < interval)
        });
        if double {
            self.begin(target, text, selected);
        } else {
            self.last_click = Some((target, now));
        }
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Option<f64> {
        text.parse::<f64>().ok().filter(|n| n.is_finite())
    }

    #[test]
    fn a_double_click_begins_only_for_the_same_control_within_the_interval() {
        let mut e = ValueEntry::default();
        let t = Instant::now();
        let interval = Duration::from_millis(350);
        assert!(!e.click("a", t, interval, "1", true));
        assert!(e.field().is_none());
        assert!(!e.click("b", t, interval, "2", true));
        assert!(!e.click("b", t + interval, interval, "2", true));
        assert!(!e.click("b", t, interval, "2", true));
        assert!(e.click("b", t + interval / 2, interval, "2", true));
        assert_eq!(e.field().expect("begun").selection(), Some(0..1));
        e.cancel();
        assert!(!e.click("b", t + interval / 2, interval, "2", true));
    }

    #[test]
    fn enter_parses_and_clamps_an_edited_value_once() {
        let mut e = ValueEntry::default();
        for (text, want) in [("0.45", 0.45), ("-99", 0.0), ("99", 1.0)] {
            e.begin("knob", "0.333", true);
            e.edit(TextEdit::Insert(text));
            assert_eq!(e.commit(parse, |n| n.clamp(0.0, 1.0)), Some(("knob", want)));
            assert_eq!(e.commit(parse, |n| n), None);
            assert!(e.field().is_none());
        }
        e.begin("voices", "", false);
        e.edit(TextEdit::Insert("12"));
        assert_eq!(
            e.commit(parse, |n| n.round().clamp(2.0, 10.0)),
            Some(("voices", 10.0))
        );
    }

    #[test]
    fn only_edited_valid_text_confirmed_with_enter_sets_anything() {
        let mut e = ValueEntry::default();
        e.begin(7, "0.333", true);
        e.edit(TextEdit::Left);
        assert_eq!(e.commit::<f64>(|_| panic!("unchanged"), |v| v), None);
        for invalid in ["loud", "NaN", "inf", "1..2", "", "0.5 invalid"] {
            e.begin(7, "", false);
            e.edit(TextEdit::Insert(invalid));
            assert_eq!(e.commit(parse, |n| n), None);
        }
        e.begin(7, "0.3", true);
        e.edit(TextEdit::Insert("0.9"));
        e.cancel();
        assert_eq!(e.commit(parse, |n| n), None);
        assert!(!e.edit(TextEdit::Backspace));
    }

    #[test]
    fn selection_backspace_and_character_movement_share_the_text_field() {
        let mut e = ValueEntry::default();
        e.begin(0, "é🦀", true);
        e.edit(TextEdit::Backspace);
        assert_eq!(e.field().expect("field").text(), "");
        e.edit(TextEdit::Insert("0.45"));
        e.edit(TextEdit::Backspace);
        e.edit(TextEdit::Home);
        e.edit(TextEdit::Delete);
        assert_eq!(e.field().expect("field").text(), ".4");
        e.edit(TextEdit::SelectAll);
        e.edit(TextEdit::Insert("1"));
        assert_eq!(e.commit(parse, |n| n), Some((0, 1.0)));
    }
}
