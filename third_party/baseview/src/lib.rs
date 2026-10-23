#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "linux")]
mod x11;

mod application_class;
mod clipboard;
mod event;
mod keyboard;
mod mouse_cursor;
mod window;
mod window_info;
mod window_open_options;

// The X11 requests can be tested without an X server on every platform.
#[cfg(any(target_os = "linux", test))]
#[path = "x11/window_identity.rs"]
mod x11_window_identity;

#[cfg(feature = "opengl")]
pub mod gl;

pub use application_class::*;
pub use clipboard::*;
pub use event::*;
pub use mouse_cursor::MouseCursor;
pub use window::*;
pub use window_info::*;
pub use window_open_options::*;
