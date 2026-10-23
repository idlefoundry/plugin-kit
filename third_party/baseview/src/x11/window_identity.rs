use x11rb::connection::RequestConnection;
use x11rb::errors::ConnectionError;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;

pub(crate) fn set_identity_and_map(
    conn: &impl RequestConnection, window: Window, title: &str, class: Option<&str>,
) -> Result<(), ConnectionError> {
    // Only top-level windows have a class: hosted editors leave identity to their host.
    if let Some(class) = class {
        // ICCCM: STRING/8, instance and class each terminated by NUL, set before mapping.
        // The application's class, not this window's title, matches the desktop entry's
        // StartupWMClass, so a window with a title of its own (an error) groups with the rest.
        let wm_class = format!("{class}\0{class}\0");
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            wm_class.as_bytes(),
        )?;
    }
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )?;
    conn.map_window(window)?;
    Ok(())
}

/// A top-level window's class: the application's, or failing that the window's initial title.
pub(crate) fn top_level_class<'a>(title: &'a str, application: Option<&'a str>) -> &'a str {
    application.unwrap_or(title)
}

#[cfg(test)]
#[path = "window_identity/tests.rs"]
mod tests;
