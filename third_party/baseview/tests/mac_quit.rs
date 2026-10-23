//! Quitting a window of its own on macOS (PATCHES.md, change 11; the CA-72's
//! `docs/decisions.md` R-QUIT, whose test this is): an application's window of its own has the
//! menus every macOS application has, ⌘Q quits it before the key reaches any view, and so does
//! the quit Apple event (the Dock's Quit, logging out), each by closing its window as the close
//! button does, so that `Window::open_blocking` returns and an application shuts down as then,
//! rather than AppKit ending the process. The quit Apple event is answered with no error (an
//! error would cancel a logout).
//!
//! On the main thread, which AppKit requires and the test harness does not run tests on
//! (`harness = false`); elsewhere it does nothing.

#![allow(clippy::unwrap_used)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    // (The test's other process, which sends the quit event as the Dock does: AppKit's own
    // process would have its handler called at once, on the sending thread.)
    let arguments: Vec<String> = std::env::args().collect();
    if let [_, flag, pid] = arguments.as_slice() {
        if flag == "--send-quit" {
            let (sent, error) = macos::send_quit(pid.parse().unwrap());
            println!("{sent} {}", error.map_or("none".to_string(), |e| e.to_string()));
            return;
        }
    }
    macos::watchdog();
    macos::command_q_closes_the_window_before_any_view_has_the_key();
    println!("command-Q closes the window before any view has the key ... ok");
    macos::the_menus_are_an_applications();
    println!("the menus are an application's ... ok");
    macos::the_quit_apple_event_closes_the_window_and_is_answered_without_error();
    println!("the quit Apple event closes the window, answered without error ... ok");
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use baseview::{
        Event, EventStatus, Size, Window, WindowEvent, WindowHandler, WindowOpenOptions,
        WindowScalePolicy,
    };
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2_foundation::NSPoint;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    /// The window's title: the application's name in its menus.
    const NAME: &str = "Quit Test";

    /// What the window's handler saw.
    #[derive(Default)]
    struct Seen {
        frames: u32,
        keys: u32,
        closed: bool,
    }

    struct Handler {
        seen: Arc<Mutex<Seen>>,
        first: Option<Box<dyn FnOnce() + Send>>,
    }

    impl WindowHandler for Handler {
        fn on_frame(&mut self, _window: &mut Window) {
            self.seen.lock().unwrap().frames += 1;
            if let Some(first) = self.first.take() {
                first();
            }
        }

        fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
            let mut seen = self.seen.lock().unwrap();
            match event {
                Event::Keyboard(_) => seen.keys += 1,
                Event::Window(WindowEvent::WillClose) => seen.closed = true,
                _ => {}
            }
            EventStatus::Captured
        }
    }

    /// A window of its own opened, `first` run at its first frame, until it closes.
    fn open(first: impl FnOnce() + Send + 'static) -> Seen {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let handler_seen = Arc::clone(&seen);
        let options = WindowOpenOptions {
            title: NAME.into(),
            size: Size::new(200.0, 100.0),
            scale: WindowScalePolicy::SystemScaleFactor,
        };
        Window::open_blocking(options, move |_| Handler {
            seen: handler_seen,
            first: Some(Box::new(first)),
        });
        // (Taken before `seen` goes: at a block's end its temporaries outlive its locals.)
        let taken = std::mem::take(&mut *seen.lock().unwrap());
        taken
    }

    /// No test waits for ever: a window that never closes fails the run.
    pub fn watchdog() {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(30));
            eprintln!("mac_quit: a window did not close within 30 s");
            std::process::exit(1);
        });
    }

    /// An Objective-C class of AppKit's or Foundation's, by name.
    fn class(name: &str) -> &'static AnyClass {
        AnyClass::get(&CString::new(name).unwrap()).unwrap()
    }

    fn app() -> *mut AnyObject {
        let class = class("NSApplication");
        // SAFETY: the shared application, on the main thread.
        unsafe { msg_send![class, sharedApplication] }
    }

    fn string(text: &str) -> *mut AnyObject {
        let class = class("NSString");
        let text = CString::new(text).unwrap();
        // SAFETY: a NUL-terminated UTF-8 string, copied by NSString.
        unsafe { msg_send![class, stringWithUTF8String: text.as_ptr()] }
    }

    fn text(string: *mut AnyObject) -> String {
        // SAFETY: an NSString's own NUL-terminated bytes, read while it is alive.
        unsafe {
            let bytes: *const c_char = msg_send![string, UTF8String];
            CStr::from_ptr(bytes).to_string_lossy().into_owned()
        }
    }

    /// ⌘Q as the keyboard gives it, posted to the application: AppKit's `sendEvent:` gives a
    /// key with Command to the main menu before any view.
    pub fn command_q_closes_the_window_before_any_view_has_the_key() {
        let posted = Arc::new(AtomicBool::new(false));
        let was_posted = Arc::clone(&posted);
        let seen = open(move || {
            let class = class("NSEvent");
            let q = string("q");
            // NSEventTypeKeyDown, NSEventModifierFlagCommand, kVK_ANSI_Q.
            // SAFETY: NSEvent's own constructor, each argument of its type.
            let event: *mut AnyObject = unsafe {
                msg_send![
                    class,
                    keyEventWithType: 10usize,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: 1usize << 20,
                    timestamp: 0.0f64,
                    windowNumber: 0isize,
                    context: std::ptr::null_mut::<AnyObject>(),
                    characters: q,
                    charactersIgnoringModifiers: q,
                    isARepeat: false,
                    keyCode: 12u16
                ]
            };
            assert!(!event.is_null(), "a key event made");
            // SAFETY: an event posted to the application's queue, on the main thread.
            let _: () = unsafe { msg_send![app(), postEvent: event, atStart: false] };
            was_posted.store(true, Ordering::Release);
        });
        assert!(posted.load(Ordering::Acquire), "⌘Q posted");
        assert!(seen.closed, "⌘Q: the window closed as its close button closes it");
        assert_eq!(seen.keys, 0, "⌘Q: the key reached the view");
    }

    /// The main menu's items: their titles, keys, modifiers and actions.
    pub fn the_menus_are_an_applications() {
        // SAFETY: the application's menus, read on the main thread (as are those below).
        let bar: *mut AnyObject = unsafe { msg_send![app(), mainMenu] };
        assert!(!bar.is_null(), "a main menu");
        let items = |menu: *mut AnyObject| -> Vec<(String, String, usize, String)> {
            // SAFETY: a menu's items and their properties, each of its type.
            unsafe {
                let count: isize = msg_send![menu, numberOfItems];
                (0..count)
                    .map(|i| {
                        let item: *mut AnyObject = msg_send![menu, itemAtIndex: i];
                        let title: *mut AnyObject = msg_send![item, title];
                        let key: *mut AnyObject = msg_send![item, keyEquivalent];
                        let mask: usize = msg_send![item, keyEquivalentModifierMask];
                        let action: Option<Sel> = msg_send![item, action];
                        let action = action.map(|a| a.name().to_string_lossy().into_owned());
                        (text(title), text(key), mask, action.unwrap_or_default())
                    })
                    .collect()
            }
        };
        let submenu = |i: isize| -> *mut AnyObject {
            // SAFETY: an item of the main menu and its submenu.
            unsafe {
                let item: *mut AnyObject = msg_send![bar, itemAtIndex: i];
                msg_send![item, submenu]
            }
        };
        let command = 1usize << 20;
        let option = 1usize << 19;
        let menu = |entries: &[(&str, &str, usize, &str)]| -> Vec<(String, String, usize, String)> {
            entries
                .iter()
                .map(|(t, k, m, a)| (t.to_string(), k.to_string(), *m, a.to_string()))
                .collect()
        };
        let named = |what: &str| format!("{what} {NAME}");
        let (about, hide, quit) = (named("About"), named("Hide"), named("Quit"));
        let application = items(submenu(0));
        // (A separator's title is empty, its key none.)
        let shown: Vec<_> =
            application.into_iter().filter(|(title, ..)| !title.is_empty()).collect();
        assert_eq!(
            shown,
            menu(&[
                (&about, "", command, "orderFrontStandardAboutPanel:"),
                (&hide, "h", command, "hide:"),
                ("Hide Others", "h", command | option, "hideOtherApplications:"),
                ("Show All", "", command, "unhideAllApplications:"),
                (&quit, "q", command, "terminate:"),
            ]),
            "the application's menu"
        );
        // (AppKit adds to the Window menu its own: Close All, ⌥⌘W, and the windows open.)
        let mut windows = items(submenu(1));
        windows.truncate(2);
        assert_eq!(
            windows,
            menu(&[
                ("Minimize", "m", command, "performMiniaturize:"),
                ("Close", "w", command, "performClose:"),
            ]),
            "the Window menu"
        );
        // SAFETY: the application's Window menu, read on the main thread.
        let windows: *mut AnyObject = unsafe { msg_send![app(), windowsMenu] };
        assert_eq!(windows, submenu(1), "the application's Window menu");
    }

    #[repr(C)]
    struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AECreateDesc(kind: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
        fn AECreateAppleEvent(
            class: u32, id: u32, target: *const AEDesc, return_id: i16, transaction_id: i32,
            result: *mut AEDesc,
        ) -> i16;
        fn AESendMessage(
            event: *const AEDesc, reply: *mut AEDesc, mode: i32, timeout: isize,
        ) -> i32;
        fn AEGetParamPtr(
            event: *const AEDesc, keyword: u32, desired: u32, actual: *mut u32, data: *mut c_void,
            size: isize, actual_size: *mut isize,
        ) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    }

    fn code(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }

    /// The quit Apple event sent to process `pid`, its answer awaited: whether it was sent
    /// (0) and the error it was answered with, if any.
    pub fn send_quit(pid: i32) -> (i32, Option<i32>) {
        let empty = || AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
        let (mut target, mut event, mut reply) = (empty(), empty(), empty());
        // SAFETY: Apple's C calls, each with its own types; every descriptor disposed.
        unsafe {
            AECreateDesc(code(b"kpid"), (&raw const pid).cast(), 4, &mut target);
            // kAutoGenerateReturnID, kAnyTransactionID.
            AECreateAppleEvent(code(b"aevt"), code(b"quit"), &target, -1, 0, &mut event);
            // kAEWaitReply, ten seconds.
            let sent = AESendMessage(&event, &mut reply, 3, 600);
            let mut error = 0i32;
            let (mut actual, mut size) = (0u32, 0isize);
            let found = AEGetParamPtr(
                &reply,
                code(b"errn"),
                code(b"long"),
                &mut actual,
                (&raw mut error).cast(),
                4,
                &mut size,
            ) == 0;
            AEDisposeDesc(&mut reply);
            AEDisposeDesc(&mut event);
            AEDisposeDesc(&mut target);
            (sent, found.then_some(error))
        }
    }

    /// The quit Apple event, as the Dock sends it and as logging out does: from another
    /// process, its answer awaited.
    pub fn the_quit_apple_event_closes_the_window_and_is_answered_without_error() {
        let answer = Arc::new(Mutex::new(None));
        let answered = Arc::clone(&answer);
        let seen = open(move || {
            std::thread::spawn(move || {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--send-quit", &std::process::id().to_string()])
                    .output()
                    .unwrap();
                *answered.lock().unwrap() =
                    Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
            });
        });
        assert!(seen.closed, "quit: the window closed as its close button closes it");
        // The answer comes once the handler has returned; the window closed in it.
        for _ in 0..100 {
            if answer.lock().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let answer = answer.lock().unwrap().take();
        assert_eq!(answer.as_deref(), Some("0 none"), "quit: sent, and answered with no error");
    }
}
