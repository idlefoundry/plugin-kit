//! Editor machinery without a window, framework or look. The CA-72's one animation clock
//! (R48), still parts warmed as an editor opens (R44), discrete drags (R51), typed
//! values (R52, R-KNOB-ENTRY), character carets and menus (R10, R18, R34, R51).
//! Preset browser state follows in this round (`docs/editor-split.md`).

pub mod cache;
pub mod clock;
pub mod drag;
pub mod menu;
pub mod text;
pub mod value;
