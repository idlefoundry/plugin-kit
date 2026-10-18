//! The pointer's look on macOS (the CA-72's change 7, `PATCHES.md`): a [`MouseCursor`] as
//! AppKit's `NSCursor`. The diagonal resize arrows are those AppKit's own window corners use
//! (`_windowResize…Cursor`), where `NSCursor` has them; a look AppKit has nothing like is the
//! arrow.

use cocoa::base::{id, BOOL, NO};
use objc::runtime::Sel;
use objc::{class, msg_send, sel, sel_impl};

use crate::MouseCursor;

/// `cursor`'s `NSCursor`.
pub(super) fn ns_cursor(cursor: MouseCursor) -> id {
    use MouseCursor::*;
    let name: Sel = match cursor {
        Hand => sel!(pointingHandCursor),
        HandGrabbing => sel!(closedHandCursor),
        Text => sel!(IBeamCursor),
        VerticalText => sel!(IBeamCursorForVerticalLayout),
        NotAllowed | PtrNotAllowed => sel!(operationNotAllowedCursor),
        Alias => sel!(dragLinkCursor),
        Copy => sel!(dragCopyCursor),
        Move | AllScroll => sel!(openHandCursor),
        Cell | Crosshair => sel!(crosshairCursor),
        EResize => sel!(resizeRightCursor),
        WResize => sel!(resizeLeftCursor),
        NResize => sel!(resizeUpCursor),
        SResize => sel!(resizeDownCursor),
        EwResize | ColResize => sel!(resizeLeftRightCursor),
        NsResize | RowResize => sel!(resizeUpDownCursor),
        NwseResize | NwResize | SeResize => sel!(_windowResizeNorthWestSouthEastCursor),
        NeswResize | NeResize | SwResize => sel!(_windowResizeNorthEastSouthWestCursor),
        Default | Help | Hidden | Working | PtrWorking | ZoomIn | ZoomOut => sel!(arrowCursor),
    };
    unsafe {
        let class = class!(NSCursor);
        let known: BOOL = msg_send![class, respondsToSelector: name];
        if known == NO {
            return msg_send![class, arrowCursor];
        }
        msg_send![class, performSelector: name]
    }
}

/// `cursor` shown now.
pub(super) fn show(cursor: MouseCursor) {
    unsafe {
        let _: () = msg_send![ns_cursor(cursor), set];
    }
}
