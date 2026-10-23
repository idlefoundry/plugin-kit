//! On X11 the handler is told when the X server asks for its window to be drawn again
//! (PATCHES.md, change 12; the CA-72's `docs/decisions.md` R-LINUX-RESIZE, its issue #65). The
//! CA-72's test, `a_panel_grown_is_shown_again_as_x_asks`, clicks PANEL in its editor; this is
//! the same without an editor: a window parented in a host's window of its own grows, and its
//! handler is told `Damaged` once it has its new size. Before the change baseview dropped the
//! `Expose` that asks for it, and the handler heard nothing until it drew of itself.
//!
//! It needs an X server (`DISPLAY`; under Wayland, XWayland's) and opens only its own windows,
//! off the screen and unmanaged (override-redirect), so nothing is shown and nothing takes the
//! focus: `cargo test --manifest-path third_party/baseview/Cargo.toml --test x11_expose --
//! --ignored`.

#![cfg(target_os = "linux")]

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use baseview::{
    Event, EventStatus, Size, Window, WindowEvent, WindowHandler, WindowOpenOptions,
    WindowScalePolicy,
};
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle, XcbWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, WindowClass};

/// The window's width, and its height before and after it grows.
const W: u32 = 240;
const SHORT: u32 = 120;
const TALL: u32 = 360;
/// Off every screen.
const ORIGIN: (i16, i16) = (-16000, -16000);

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

/// A handler that takes the heights it is asked for and reports what it is told.
struct Handler {
    heights: Receiver<u32>,
    damaged: Sender<(u32, u32)>,
    size: (u32, u32),
}

impl WindowHandler for Handler {
    fn on_frame(&mut self, window: &mut Window) {
        if let Ok(height) = self.heights.try_recv() {
            window.resize(Size::new(f64::from(W), f64::from(height)));
        }
    }

    fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
        match event {
            Event::Window(WindowEvent::Resized(info)) => {
                let size = info.physical_size();
                self.size = (size.width, size.height);
            }
            // With the size the handler had then, which it would draw at.
            Event::Window(WindowEvent::Damaged) => {
                let _ = self.damaged.send(self.size);
            }
            _ => {}
        }
        EventStatus::Captured
    }
}

#[test]
#[ignore = "requires an X11 display; creates only its own off-screen windows"]
fn a_window_grown_is_told_to_draw_itself_again_at_its_new_size() {
    let (conn, screen) = x11rb::connect(None).expect("an X11 display (DISPLAY)");
    let root = conn.setup().roots[screen].root;
    // The host's window, room in it for the window grown.
    let host = conn.generate_id().expect("an id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        host,
        root,
        ORIGIN.0,
        ORIGIN.1,
        W as u16,
        TALL as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )
    .expect("the host's window")
    .check()
    .expect("the host's window made");
    conn.map_window(host).expect("the host's window mapped");
    conn.flush().expect("flushed");

    let (heights, asked) = mpsc::channel();
    let (told, damaged) = mpsc::channel();
    let mut handle = Window::open_parented(
        &Parent(host),
        WindowOpenOptions {
            title: "Expose test".into(),
            size: Size::new(f64::from(W), f64::from(SHORT)),
            scale: WindowScalePolicy::ScaleFactor(1.0),
        },
        move |_| Handler { heights: asked, damaged: told, size: (W, SHORT) },
    );
    let child = match handle.raw_window_handle() {
        RawWindowHandle::Xlib(h) => h.window as u32,
        RawWindowHandle::Xcb(h) => h.window,
        _ => panic!("an X11 window"),
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Mapped: the server may ask for it to be drawn (it does under XWayland).
        std::thread::sleep(Duration::from_millis(300));
        for _ in 0..3 {
            for height in [SHORT, TALL] {
                while damaged.try_recv().is_ok() {}
                heights.send(height).expect("the handler's");
                let until = Instant::now() + Duration::from_secs(2);
                loop {
                    let geometry = conn
                        .get_geometry(child)
                        .expect("asked")
                        .reply()
                        .expect("the window's geometry");
                    if (u32::from(geometry.width), u32::from(geometry.height)) == (W, height) {
                        break;
                    }
                    assert!(Instant::now() < until, "the window took its new size");
                    std::thread::sleep(Duration::from_millis(1));
                }
                if height == SHORT {
                    continue;
                }
                let size = damaged
                    .recv_timeout(Duration::from_secs(1))
                    .expect("the handler told to draw its window grown again");
                assert_eq!(size, (W, TALL), "told before its new size");
            }
        }
    }));

    handle.close();
    let until = Instant::now() + Duration::from_secs(5);
    while handle.is_open() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = conn.destroy_window(host);
    let _ = conn.flush();
    assert!(!handle.is_open(), "the window closed");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
