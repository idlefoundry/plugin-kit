//! Software editors' drawing machinery, from the CA-72's `render.rs`, `strip/worn.rs`
//! and editor window (R44, R48, R58 to R60), and its `glow.rs` (R-LFO).
//!
//! Rebuild each damaged place from its background and layers in order. Clip a component's
//! incoming changes before merging them with its own, so an off-component change does not
//! enlarge its work (R58). The plug-in supplies every picture, colour and measurement.
//! Window ownership, repaint events and platform presentation stay in its adapter.

mod compose;
mod damage;
pub mod glow;
mod sparse;

pub use compose::{convert, copy, over};
pub use damage::{Damage, Rect};
pub use glow::Coverage;
pub use resvg::tiny_skia;
pub use sparse::Sparse;
