//! A held key reaches the handler as one press, then presses marked `repeat`, and one release as
//! it is let go, on a real X server repeating it by itself (PATCHES.md, change 14). Before the
//! change the server's repeats came as a release and a press each, marked as nothing: a held
//! note was let go and struck again some 25 times a second.
//!
//! It needs an X server (`DISPLAY`; under Wayland, XWayland's) with its auto-repeat on, and
//! holds a key with XTEST, as `xdotool keydown` does. Ignored, run by hand:
//! `cargo test --manifest-path third_party/baseview/Cargo.toml --test x11_key_repeat --
//! --ignored --test-threads=1`.
//! - `a_held_key_*` opens only its own windows, off the screen and unmanaged
//!   (override-redirect), and takes the keyboard for its own window while it holds the key
//!   (an active grab: no focus is asked of a window manager), so it runs under XWayland too.
//! - `losing_the_focus_*` opens a window of its own and gives it the focus: on a server with
//!   no window manager (Xvfb, `xvfb-run -a`).

#![cfg(target_os = "linux")]

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use baseview::{
    Event, EventStatus, Size, Window, WindowEvent, WindowHandler, WindowOpenOptions,
    WindowScalePolicy,
};
use keyboard_types::{Code, KeyState};
use raw_window_handle::{
    HasRawDisplayHandle, HasRawWindowHandle, RawDisplayHandle, RawWindowHandle, XcbWindowHandle,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AutoRepeatMode, ConnectionExt as _, CreateWindowAux, InputFocus, WindowClass, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// A on an evdev keymap (Xvfb's and XWayland's).
const A: u8 = 38;
/// Held this long: past the server's delay before it repeats (660 ms unless set otherwise),
/// and some 20 repeats more.
const HOLD: Duration = Duration::from_millis(1600);
/// Off every screen.
const ORIGIN: (i16, i16) = (-16000, -16000);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Seen {
    Down { repeat: bool },
    Up,
    Focused,
    Unfocused,
}

/// What the build closure found: the window, and whether the server marks the repeats.
struct Opened {
    window: u32,
    detectable: bool,
}

/// How the window is to have the keys.
#[derive(Clone, Copy)]
enum Keys {
    /// The keyboard grabbed for the window by its own connection; `detectable` false turns
    /// XKB's detectable auto-repeat off again for that connection, as a server without it would
    /// have it.
    Grabbed { detectable: bool },
    /// Given the focus by the test (no window manager).
    Focus,
}

struct Recorder {
    seen: Sender<(Instant, Seen)>,
    close: Receiver<()>,
}

impl WindowHandler for Recorder {
    fn on_frame(&mut self, window: &mut Window) {
        if self.close.try_recv().is_ok() {
            window.close();
        }
    }

    fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
        let seen = match event {
            Event::Keyboard(k) if k.code == Code::KeyA => match k.state {
                KeyState::Down => Seen::Down { repeat: k.repeat },
                KeyState::Up => Seen::Up,
            },
            Event::Window(WindowEvent::Focused) => Seen::Focused,
            Event::Window(WindowEvent::Unfocused) => Seen::Unfocused,
            _ => return EventStatus::Ignored,
        };
        let _ = self.seen.send((Instant::now(), seen));
        EventStatus::Captured
    }
}

/// The host's window the test's opens in, as baseview takes it.
struct Parent(u32);

// SAFETY: the host's window outlives the window opened in it.
unsafe impl HasRawWindowHandle for Parent {
    fn raw_window_handle(&self) -> RawWindowHandle {
        let mut handle = XcbWindowHandle::empty();
        handle.window = self.0;
        RawWindowHandle::Xcb(handle)
    }
}

fn options() -> WindowOpenOptions {
    WindowOpenOptions {
        title: "x11_key_repeat".into(),
        size: Size::new(160.0, 120.0),
        scale: WindowScalePolicy::ScaleFactor(1.0),
    }
}

/// The window's build: its handler, and on its own connection what `keys` asks.
fn build(
    window: &mut Window, keys: Keys, opened: Sender<Opened>, seen: Sender<(Instant, Seen)>,
    close: Receiver<()>,
) -> Recorder {
    let id = match window.raw_window_handle() {
        RawWindowHandle::Xlib(h) => h.window as u32,
        RawWindowHandle::Xcb(h) => h.window,
        h => panic!("not an X11 window: {:?}", h),
    };
    let RawDisplayHandle::Xlib(display) = window.raw_display_handle() else {
        panic!("not an Xlib display");
    };
    let dpy = display.display as *mut x11::xlib::Display;
    if let Keys::Grabbed { detectable } = keys {
        // SAFETY: baseview's own display, open for as long as its window.
        unsafe {
            if !detectable {
                let mut supported = 0;
                x11::xlib::XkbSetDetectableAutoRepeat(dpy, x11::xlib::False, &mut supported);
            }
            let grabbed = x11::xlib::XGrabKeyboard(
                dpy,
                u64::from(id),
                x11::xlib::False,
                x11::xlib::GrabModeAsync,
                x11::xlib::GrabModeAsync,
                x11::xlib::CurrentTime,
            );
            assert_eq!(grabbed, x11::xlib::GrabSuccess, "the keyboard grabbed for the window");
            x11::xlib::XFlush(dpy);
        }
    }
    let detectable =
        window.key_repeats_detectable() && !matches!(keys, Keys::Grabbed { detectable: false });
    opened.send(Opened { window: id, detectable }).unwrap();
    Recorder { seen, close }
}

/// The key `A` pressed (`down`) or let go, as a keyboard does it (XTEST).
fn key(conn: &RustConnection, root: u32, down: bool) {
    let kind = if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT };
    conn.xtest_fake_input(kind, A, 0, root, 0, 0, 0).unwrap();
    conn.sync().unwrap();
}

/// Everything seen until `quiet` passes with nothing more.
fn drain(seen: &Receiver<(Instant, Seen)>, quiet: Duration) -> Vec<Seen> {
    let mut out = Vec::new();
    while let Ok((_, s)) = seen.recv_timeout(quiet) {
        out.push(s);
    }
    out
}

fn auto_repeat_on(conn: &RustConnection) {
    let control = conn.get_keyboard_control().unwrap().reply().unwrap();
    assert!(
        control.global_auto_repeat == AutoRepeatMode::ON,
        "the X server's auto-repeat is off (`xset r on`)"
    );
}

/// A held, then let go; then struck twice quickly. What the handler was told.
fn hold_a(keys: Keys) -> (Vec<Seen>, Vec<Seen>, bool) {
    let (conn, screen) = x11rb::connect(None).expect("an X11 display (DISPLAY)");
    auto_repeat_on(&conn);
    let root = conn.setup().roots[screen].root;
    let host = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        host,
        root,
        ORIGIN.0,
        ORIGIN.1,
        160,
        120,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )
    .unwrap();
    conn.map_window(host).unwrap();
    conn.sync().unwrap();

    let (opened_tx, opened) = mpsc::channel();
    let (seen_tx, seen) = mpsc::channel();
    let (close_tx, close) = mpsc::channel();
    let mut handle = Window::open_parented(&Parent(host), options(), move |window| {
        build(window, keys, opened_tx, seen_tx, close)
    });
    let opened = opened.recv_timeout(Duration::from_secs(10)).expect("the window opened");
    // The grab taken and the window mapped before the key goes down.
    thread::sleep(Duration::from_millis(300));

    key(&conn, root, true);
    thread::sleep(HOLD);
    key(&conn, root, false);
    let held = drain(&seen, Duration::from_millis(400));
    // Let go and struck again at once, by hand's measure: 30 ms apart.
    key(&conn, root, true);
    thread::sleep(Duration::from_millis(30));
    key(&conn, root, false);
    thread::sleep(Duration::from_millis(30));
    key(&conn, root, true);
    thread::sleep(Duration::from_millis(30));
    key(&conn, root, false);
    let again = drain(&seen, Duration::from_millis(400));

    let _ = close_tx.send(());
    thread::sleep(Duration::from_millis(100));
    handle.close();
    conn.destroy_window(host).unwrap();
    conn.sync().unwrap();
    (held, again, opened.detectable)
}

/// One press, at least five repeats, and one release at the end, nothing between.
fn check_held(held: &[Seen]) {
    eprintln!("held: {} events: {:?}", held.len(), summary(held));
    assert_eq!(held.first(), Some(&Seen::Down { repeat: false }), "{:?}", held);
    assert_eq!(held.last(), Some(&Seen::Up), "{:?}", held);
    let middle = &held[1..held.len() - 1];
    assert!(
        middle.iter().all(|s| *s == Seen::Down { repeat: true }),
        "only repeats while held: {:?}",
        summary(held)
    );
    assert!(middle.len() >= 5, "the server repeated the key: {} repeats", middle.len());
}

fn check_again(again: &[Seen]) {
    let down = Seen::Down { repeat: false };
    assert_eq!(again, [down, Seen::Up, down, Seen::Up], "struck twice");
}

/// The events, runs of one collapsed: `(event, how many)`.
fn summary(seen: &[Seen]) -> Vec<(Seen, usize)> {
    let mut out: Vec<(Seen, usize)> = Vec::new();
    for s in seen {
        match out.last_mut() {
            Some((last, n)) if last == s => *n += 1,
            _ => out.push((*s, 1)),
        }
    }
    out
}

#[test]
#[ignore = "requires an X11 display; creates only its own off-screen windows"]
fn a_held_key_is_one_press_and_its_repeats() {
    let (held, again, detectable) = hold_a(Keys::Grabbed { detectable: true });
    eprintln!("detectable auto-repeat: {}", detectable);
    check_held(&held);
    check_again(&again);
}

#[test]
#[ignore = "requires an X11 display; creates only its own off-screen windows"]
fn a_held_key_is_one_press_and_its_repeats_without_detectable_auto_repeat() {
    let (held, again, detectable) = hold_a(Keys::Grabbed { detectable: false });
    assert!(!detectable);
    check_held(&held);
    check_again(&again);
}

#[test]
#[ignore = "requires an X11 display with no window manager (xvfb-run -a)"]
fn losing_the_focus_forgets_the_keys_down() {
    let (conn, screen) = x11rb::connect(None).expect("an X11 display (DISPLAY)");
    auto_repeat_on(&conn);
    let root = conn.setup().roots[screen].root;
    let (opened_tx, opened) = mpsc::channel();
    let (seen_tx, seen) = mpsc::channel();
    let (close_tx, close) = mpsc::channel();
    let thread = thread::spawn(move || {
        Window::open_blocking(options(), move |window| {
            build(window, Keys::Focus, opened_tx, seen_tx, close)
        })
    });
    let window = opened.recv_timeout(Duration::from_secs(10)).expect("the window opened").window;
    thread::sleep(Duration::from_millis(300));
    let focus = |to: u32| {
        conn.set_input_focus(InputFocus::NONE, to, x11rb::CURRENT_TIME).unwrap();
        conn.sync().unwrap();
        thread::sleep(Duration::from_millis(200));
    };
    focus(window);
    key(&conn, root, true);
    thread::sleep(HOLD);
    // The focus gone while A is held, A let go elsewhere, the focus back, and A struck again.
    focus(x11rb::NONE);
    key(&conn, root, false);
    focus(window);
    key(&conn, root, true);
    thread::sleep(Duration::from_millis(100));
    key(&conn, root, false);
    let seen_all = drain(&seen, Duration::from_millis(400));
    let _ = close_tx.send(());
    thread.join().unwrap();

    eprintln!("{:?}", summary(&seen_all));
    let down = Seen::Down { repeat: false };
    let repeat = Seen::Down { repeat: true };
    let s = summary(&seen_all);
    assert_eq!(s.first().map(|x| x.0), Some(Seen::Focused), "{:?}", s);
    assert_eq!(s[1], (down, 1), "{:?}", s);
    assert_eq!(s[2].0, repeat, "{:?}", s);
    assert!(s[2].1 >= 5, "{:?}", s);
    assert_eq!(&s[3..], [(Seen::Unfocused, 1), (Seen::Focused, 1), (down, 1), (Seen::Up, 1)]);
}
