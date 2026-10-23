//! A window of its own is an application's (PATCHES.md, change 11): the menus every macOS
//! application has, and every way of quitting it (the Quit item and ⌘Q, the Dock's Quit, logging
//! out) closing its window as the window's close button does, so that whoever opened it sees
//! `Window::open_blocking` return, as it does for the close button, and shuts down as then.
//!
//! Upstream's `open_blocking` gave the application no main menu: no Quit item, so no ⌘Q (the
//! key went to the view, whose handler had nothing to do with it), and AppKit's own quitting
//! (`terminate:`) ends the process within AppKit, past everything after `open_blocking`.

use cocoa::appkit::{NSApp, NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use cocoa::base::{id, nil, BOOL};
use cocoa::foundation::{NSAutoreleasePool, NSString, NSUInteger};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

/// The Apple event every way of quitting from outside sends (`kCoreEventClass`,
/// `kAEQuitApplication`): the Dock's Quit, AppleScript's `quit`, logging out, restarting and
/// shutting down.
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const QUIT_APPLICATION: u32 = u32::from_be_bytes(*b"quit");

/// `NSTerminateCancel`: AppKit leaves the process alone (it ends once its window has closed).
const TERMINATE_CANCEL: NSUInteger = 0;

const DELEGATE_CLASS: &str = "BaseviewApplicationDelegate";

/// The prefix of the view classes baseview makes (`view.rs`), each window's delegate.
const VIEW_CLASS_PREFIX: &str = "BaseviewNSView_";

/// The application named `name` given its menus and the delegate that turns quitting into
/// closing its windows. Once a process: a second window of its own (the CA-72's error window,
/// after its editor's) keeps them.
pub(super) unsafe fn install(app: id, name: &str) {
    let delegate: id = msg_send![app, delegate];
    if delegate != nil {
        return;
    }
    let pool = NSAutoreleasePool::new(nil);
    let (bar, windows) = menus(name);
    app.setMainMenu_(bar);
    app.setWindowsMenu_(windows);
    // The application keeps no hold on its delegate: this one is never released.
    let delegate: id = msg_send![delegate_class(), new];
    let () = msg_send![app, setDelegate: delegate];
    let () = msg_send![pool, drain];
}

/// The main menu and its Window menu: the application's (About, Hide ⌘H, Hide Others ⌥⌘H,
/// Show All, Quit ⌘Q) and the Window menu (Minimize ⌘M, Close ⌘W), as every macOS
/// application's. The items act on whatever answers first (the application, the window).
pub(super) unsafe fn menus(name: &str) -> (id, id) {
    let command = NSEventModifierFlags::NSCommandKeyMask;
    let none = NSEventModifierFlags::empty();
    let bar: id = NSMenu::new(nil).autorelease();

    let application: id = NSMenu::new(nil).autorelease();
    application.addItem_(item(
        &format!("About {name}"),
        sel!(orderFrontStandardAboutPanel:),
        "",
        none,
    ));
    application.addItem_(NSMenuItem::separatorItem(nil));
    application.addItem_(item(&format!("Hide {name}"), sel!(hide:), "h", command));
    application.addItem_(item(
        "Hide Others",
        sel!(hideOtherApplications:),
        "h",
        command | NSEventModifierFlags::NSAlternateKeyMask,
    ));
    application.addItem_(item("Show All", sel!(unhideAllApplications:), "", none));
    application.addItem_(NSMenuItem::separatorItem(nil));
    application.addItem_(item(&format!("Quit {name}"), sel!(terminate:), "q", command));
    let holder: id = NSMenuItem::new(nil).autorelease();
    holder.setSubmenu_(application);
    bar.addItem_(holder);

    let windows: id = NSMenu::alloc(nil).initWithTitle_(string("Window")).autorelease();
    windows.addItem_(item("Minimize", sel!(performMiniaturize:), "m", command));
    windows.addItem_(item("Close", sel!(performClose:), "w", command));
    let holder: id = NSMenuItem::new(nil).autorelease();
    holder.setSubmenu_(windows);
    bar.addItem_(holder);

    (bar, windows)
}

unsafe fn string(text: &str) -> id {
    NSString::alloc(nil).init_str(text).autorelease()
}

unsafe fn item(title: &str, action: Sel, key: &str, modifiers: NSEventModifierFlags) -> id {
    let item: id = NSMenuItem::alloc(nil)
        .initWithTitle_action_keyEquivalent_(string(title), action, string(key))
        .autorelease();
    if !key.is_empty() {
        item.setKeyEquivalentModifierMask_(modifiers);
    }
    item
}

unsafe fn delegate_class() -> &'static Class {
    if let Some(class) = Class::get(DELEGATE_CLASS) {
        return class;
    }
    let mut class = ClassDecl::new(DELEGATE_CLASS, class!(NSObject)).unwrap();
    class.add_method(
        sel!(applicationWillFinishLaunching:),
        will_finish_launching as extern "C" fn(&Object, Sel, id),
    );
    class.add_method(
        sel!(applicationShouldTerminate:),
        should_terminate as extern "C" fn(&Object, Sel, id) -> NSUInteger,
    );
    class.add_method(
        sel!(handleQuitEvent:withReplyEvent:),
        quit_event as extern "C" fn(&Object, Sel, id, id),
    );
    class.register()
}

/// The quit Apple event is this delegate's from now on, in place of AppKit's own handler
/// (installed before this is called), which calls `terminate:`.
extern "C" fn will_finish_launching(this: &Object, _: Sel, _: id) {
    unsafe {
        let manager: id = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
        let () = msg_send![
            manager,
            setEventHandler: this
            andSelector: sel!(handleQuitEvent:withReplyEvent:)
            forEventClass: CORE_EVENT_CLASS
            andEventID: QUIT_APPLICATION
        ];
    }
}

/// The Quit item, ⌘Q, and anything else that calls `terminate:`: the windows closed, and AppKit
/// told to leave the process be. (Not the quit Apple event, whose cancelling would cancel a
/// logout too: `quit_event` answers that one.)
extern "C" fn should_terminate(_: &Object, _: Sel, _: id) -> NSUInteger {
    unsafe { close_windows() };
    TERMINATE_CANCEL
}

/// The Dock's Quit, AppleScript's, logging out: the windows closed, and the event answered with
/// no error, so that a logout goes on, waiting for the process to end.
extern "C" fn quit_event(_: &Object, _: Sel, _event: id, _reply: id) {
    unsafe { close_windows() };
}

/// Each of baseview's own windows closed as its close button closes it: its view, the window's
/// delegate, asked `windowShouldClose:` (which tells the handler `WillClose`, closes the window
/// and stops the application, answering NO for AppKit to leave the window to it).
unsafe fn close_windows() {
    let pool = NSAutoreleasePool::new(nil);
    // (A copy: closing a window takes it out of the application's list.)
    let windows: id = msg_send![NSApp(), windows];
    let windows: id = msg_send![windows, copy];
    let count: NSUInteger = msg_send![windows, count];
    for i in 0..count {
        let window: id = msg_send![windows, objectAtIndex: i];
        let delegate: id = msg_send![window, delegate];
        if delegate != nil && is_view(delegate) {
            let _: BOOL = msg_send![delegate, windowShouldClose: window];
        }
    }
    let () = msg_send![windows, release];
    let () = msg_send![pool, drain];
}

unsafe fn is_view(object: id) -> bool {
    let class: *const Class = msg_send![object, class];
    !class.is_null() && (*class).name().starts_with(VIEW_CLASS_PREFIX)
}
