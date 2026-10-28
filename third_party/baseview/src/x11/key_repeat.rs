//! A held key's repeats on X11 (PATCHES.md, change 14). The X server repeats a held key by
//! itself, and by default sends each repeat as a release and a press, which a handler cannot
//! tell from the key let go and struck again. Asked for detectable auto-repeat (XKB's per-client
//! flag, which `XkbSetDetectableAutoRepeat` sets), it sends the presses alone, and the release
//! once the key is let go. Either way the handler is given each repeat as a press marked
//! `repeat`, and no release until the key is let go:
//! - a press of a key already down is a repeat (detectable auto-repeat);
//! - a release followed at once by a press of the same key at the same time is a repeat
//!   (without it: the server's pair, sent together, with one timestamp, which a hand cannot do),
//!   and the release is dropped.
//!
//! The keys down are forgotten whenever a release may have gone elsewhere (the focus moved, the
//! pointer left a window in another's), so that a key let go meanwhile is struck afresh.

use x11rb::protocol::xproto::{KeyPressEvent, KeyReleaseEvent};
use x11rb::protocol::Event;

/// The keys down in one window, as its key events tell it, by keycode.
#[derive(Debug, Default)]
pub(crate) struct KeyRepeats {
    down: [u64; 4],
}

impl KeyRepeats {
    fn bit(keycode: u8) -> (usize, u64) {
        (usize::from(keycode / 64), 1 << (keycode % 64))
    }

    /// A key pressed: whether it is one of the server's repeats (the key already down).
    pub(crate) fn press(&mut self, press: &KeyPressEvent) -> bool {
        let (word, bit) = Self::bit(press.detail);
        let repeat = self.down[word] & bit != 0;
        self.down[word] |= bit;
        repeat
    }

    /// A key let go, `next` the event after it if it has come: whether the handler is to be
    /// told. Not if it is the first half of the server's repeat, the press `next`: the key then
    /// stays down, so that the press is a repeat.
    pub(crate) fn release(&mut self, release: &KeyReleaseEvent, next: Option<&Event>) -> bool {
        if let Some(Event::KeyPress(press)) = next {
            if repeat_pair(release, press) {
                return false;
            }
        }
        let (word, bit) = Self::bit(release.detail);
        self.down[word] &= !bit;
        true
    }

    /// Every key forgotten: their releases may go to another window.
    pub(crate) fn forget(&mut self) {
        self.down = [0; 4];
    }
}

/// A release and the press after it are the server's repeat: one key, one window, one time.
fn repeat_pair(release: &KeyReleaseEvent, press: &KeyPressEvent) -> bool {
    release.detail == press.detail && release.time == press.time && release.event == press.event
}

#[cfg(test)]
#[path = "key_repeat/tests.rs"]
mod tests;
