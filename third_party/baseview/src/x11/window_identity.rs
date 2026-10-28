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

/// ICCCM `WM_SIZE_HINTS` flags: the program's minimum and maximum sizes.
const P_MIN_SIZE: u32 = 1 << 4;
const P_MAX_SIZE: u32 = 1 << 5;

/// A top-level window's size, `width` by `height` physical pixels, told to the window manager as
/// both its least and its greatest (ICCCM `WM_NORMAL_HINTS`, `WM_SIZE_HINTS/32`). Its handler
/// sizes the window and draws what it holds at that size; at any other the handler's drawing
/// stays in the top left corner, black beyond it or cut off. A tiling window manager floats a
/// window whose least and greatest sizes are the same (Hyprland does so for XWayland), and a
/// floating window is kept at that size. Set before mapping, and again before each resize the
/// handler asks for, which the window manager would otherwise hold to the old size.
pub(crate) fn set_fixed_size(
    conn: &impl RequestConnection, window: Window, width: u32, height: u32,
) -> Result<(), ConnectionError> {
    let (width, height) = (width.max(1), height.max(1));
    // flags, x, y, width, height (obsolete), min, max, increments, aspects, base, gravity.
    let mut hints = [0u32; 18];
    hints[0] = P_MIN_SIZE | P_MAX_SIZE;
    hints[5] = width;
    hints[6] = height;
    hints[7] = width;
    hints[8] = height;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NORMAL_HINTS,
        AtomEnum::WM_SIZE_HINTS,
        &hints,
    )?;
    Ok(())
}

/// A top-level window's class: the application's, or failing that the window's initial title.
pub(crate) fn top_level_class<'a>(title: &'a str, application: Option<&'a str>) -> &'a str {
    application.unwrap_or(title)
}

#[cfg(test)]
#[path = "window_identity/tests.rs"]
mod tests;
