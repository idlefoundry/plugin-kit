//! Editor machinery without a window, framework or look. The CA-72's one animation clock
//! (R48) and still parts warmed as an editor opens (R44). Browser, menu, text and drag state
//! are left for a later round (`docs/editor-split.md`).

pub mod cache;
pub mod clock;
