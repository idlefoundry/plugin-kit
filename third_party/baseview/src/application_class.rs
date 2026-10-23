//! The application's identity for its top-level windows, kept apart from their titles.

use std::sync::OnceLock;

static CLASS: OnceLock<String> = OnceLock::new();

/// Name the application whose top-level windows these are, before the first one opens. On
/// Linux this becomes every top-level window's X11 `WM_CLASS`, which a desktop entry's
/// `StartupWMClass` matches, whatever each window's title. The first name given stays.
/// Without one, a top-level window's initial title is its class. Other platforms ignore it.
pub fn set_application_class(class: &str) {
    let _ = CLASS.set(class.to_owned());
}

/// The name given to [`set_application_class`], if any.
pub fn application_class() -> Option<&'static str> {
    CLASS.get().map(String::as_str)
}
