//! Editor machinery without a window, framework or look. The CA-72's one animation clock
//! (R48), still parts warmed as an editor opens (R44), discrete drags (R51), typed
//! values (R52, R-KNOB-ENTRY), character carets, preset browser state and menus
//! (R10, R18, R34, R51). All drawing, dimensions, library I/O and host actions
//! belong to the caller (`docs/editor-split.md`).

pub mod browser;
pub mod cache;
pub mod clock;
pub mod drag;
pub mod menu;
pub mod text;
pub mod value;
