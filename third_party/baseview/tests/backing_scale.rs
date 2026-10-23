//! An editor's window on macOS as it moves to a screen of another scale (PATCHES.md, change 8;
//! the CA-72's `docs/decisions.md` R-TWO-SCREENS, its issue #48, whose test this is): AppKit
//! tells baseview's view of the new scale but leaves the view's layer at the old one, and
//! softbuffer lays its tiles out at its layer's scale, which follows the view's. A frame drawn
//! for a 1x screen in a window just moved there from a Retina one showed at half its size in the
//! window's top left corner, the rest empty, until the editor was closed. Here the view's layer
//! is left at the other scale, as AppKit leaves it, and the view told its backing properties
//! changed: the layer takes the window's scale, softbuffer's with it, and the frame presented
//! then fills the view, no more, no less. The window is never shown.
//!
//! On the main thread, which AppKit and softbuffer's macOS backend require and the test harness
//! does not run tests on (`harness = false`); elsewhere it does nothing.

#![allow(clippy::unwrap_used)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    macos::the_layer_takes_the_windows_scale_and_the_frame_fills_the_view();
    println!("the layer takes the window's scale and the frame fills the view ... ok");
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::ffi::c_void;
    use std::num::NonZeroU32;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use baseview::{
        Event, EventStatus, Size, Window, WindowEvent, WindowHandler, WindowOpenOptions,
        WindowScalePolicy,
    };
    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle_06 as rwh;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    /// The view's size, in points.
    const W: f64 = 600.0;
    const H: f64 = 300.0;

    /// The host's view the editor's opens in, as baseview takes it.
    struct Parent(*mut c_void);

    // SAFETY: the view outlives the window opened in it.
    unsafe impl raw_window_handle::HasRawWindowHandle for Parent {
        fn raw_window_handle(&self) -> raw_window_handle::RawWindowHandle {
            let mut handle = raw_window_handle::AppKitWindowHandle::empty();
            handle.ns_view = self.0;
            raw_window_handle::RawWindowHandle::AppKit(handle)
        }
    }

    /// baseview's view as softbuffer takes it.
    #[derive(Clone)]
    struct Target(NonNull<c_void>);

    impl rwh::HasWindowHandle for Target {
        fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
            let raw = rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(self.0));
            // SAFETY: the view outlives the surface made on it.
            Ok(unsafe { rwh::WindowHandle::borrow_raw(raw) })
        }
    }

    impl rwh::HasDisplayHandle for Target {
        fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
            let raw = rwh::RawDisplayHandle::AppKit(rwh::AppKitDisplayHandle::new());
            // SAFETY: AppKit's display handle carries nothing.
            Ok(unsafe { rwh::DisplayHandle::borrow_raw(raw) })
        }
    }

    /// A handler that keeps the scales its window was resized to.
    struct Handler(Arc<Mutex<Vec<f64>>>);

    impl WindowHandler for Handler {
        fn on_frame(&mut self, _window: &mut Window) {}

        fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
            if let Event::Window(WindowEvent::Resized(info)) = event {
                self.0.lock().unwrap().push(info.scale());
            }
            EventStatus::Captured
        }
    }

    /// An Objective-C class of AppKit's, by name.
    fn class(name: &str) -> &'static AnyClass {
        AnyClass::get(&std::ffi::CString::new(name).unwrap()).unwrap()
    }

    fn scale_of(layer: &AnyObject) -> f64 {
        // SAFETY: a CALayer's scale factor.
        unsafe { msg_send![layer, contentsScale] }
    }

    /// A layer's sublayers.
    fn sublayers(layer: &AnyObject) -> Vec<Retained<AnyObject>> {
        // SAFETY: a CALayer's sublayers, an NSArray of CALayers.
        unsafe {
            let sublayers: Option<Retained<AnyObject>> = msg_send![layer, sublayers];
            let Some(sublayers) = sublayers else {
                return Vec::new();
            };
            let n: usize = msg_send![&*sublayers, count];
            (0..n).map(|i| msg_send![&*sublayers, objectAtIndex: i]).collect()
        }
    }

    /// A frame of the view's size at `scale` shown on `surface`, every pixel of it.
    fn present(surface: &mut softbuffer::Surface<Target, Target>, scale: f64) {
        let (w, h) = ((W * scale).round() as u32, (H * scale).round() as u32);
        surface.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap()).unwrap();
        let mut buffer = surface.buffer_mut().unwrap();
        buffer.fill(0x003b_2213);
        buffer.present().unwrap();
    }

    /// Where softbuffer's tiles reach, in points (right, bottom), and each tile's scale.
    fn tiled(layer: &AnyObject) -> ((f64, f64), Vec<f64>) {
        let tiles = sublayers(layer);
        let reach = tiles.iter().fold((0.0f64, 0.0f64), |(r, b), t| {
            // SAFETY: a CALayer's frame.
            let f: NSRect = unsafe { msg_send![&**t, frame] };
            (r.max(f.origin.x + f.size.width), b.max(f.origin.y + f.size.height))
        });
        (reach, tiles.iter().map(|t| scale_of(t)).collect())
    }

    pub fn the_layer_takes_the_windows_scale_and_the_frame_fills_the_view() {
        // SAFETY: AppKit's classes and methods, on the main thread: the application, a window
        // (not shown) and its content view, the host's view.
        let (window, content) = unsafe {
            let app = class("NSApplication");
            let _: *mut AnyObject = msg_send![app, sharedApplication];
            let class = class("NSWindow");
            let window: Allocated<AnyObject> = msg_send![class, alloc];
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(W, H));
            // (Borderless, buffered.)
            let window: Retained<AnyObject> = msg_send![
                window,
                initWithContentRect: rect,
                styleMask: 0usize,
                backing: 2usize,
                defer: Bool::NO
            ];
            let _: () = msg_send![&*window, setReleasedWhenClosed: Bool::NO];
            let content: *mut AnyObject = msg_send![&*window, contentView];
            (window, content)
        };
        // SAFETY: an NSWindow's scale.
        let backing: f64 = unsafe { msg_send![&*window, backingScaleFactor] };
        let other = if backing > 1.5 { 1.0 } else { 2.0 };

        let resized = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&resized);
        let mut handle = Window::open_parented(
            &Parent(content.cast()),
            WindowOpenOptions {
                title: String::from("Backing scale test"),
                size: Size::new(W, H),
                scale: WindowScalePolicy::SystemScaleFactor,
            },
            move |_| Handler(kept),
        );
        // SAFETY: the content view's subviews: baseview's view, just added.
        let view: Retained<AnyObject> = unsafe {
            let subviews: Retained<AnyObject> = msg_send![&*content, subviews];
            msg_send![&*subviews, lastObject]
        };
        let target = Target(NonNull::from(&*view).cast());
        let context = softbuffer::Context::new(target.clone()).unwrap();
        let mut surface = softbuffer::Surface::new(&context, target).unwrap();
        // SAFETY: the view's layer, made by softbuffer (it wants one).
        let root: Retained<AnyObject> = unsafe { msg_send![&*view, layer] };
        let layer = sublayers(&root).into_iter().next().unwrap();
        present(&mut surface, backing);
        assert_eq!(tiled(&layer).0, (W, H));

        // The window on a screen of the other scale: AppKit leaves the view's layer as it was
        // (softbuffer's follows it), ...
        // SAFETY: a CALayer's scale factor set.
        let _: () = unsafe { msg_send![&*root, setContentsScale: other] };
        assert_eq!(scale_of(&layer), other);
        // ... and tells the view (the window's scale, here as it was: no resize follows).
        // SAFETY: the method baseview's view answers to, as AppKit calls it.
        let _: () =
            unsafe { msg_send![&*view, viewDidChangeBackingProperties: None::<&AnyObject>] };
        assert_eq!(scale_of(&root), backing, "the view's layer left at another scale");
        assert_eq!(scale_of(&layer), backing, "softbuffer's layer left at another scale");

        // The frame for the window's scale shown: in tiles at that scale, filling the view (at
        // the old scale it reached half or twice as far).
        present(&mut surface, backing);
        let (reach, scales) = tiled(&layer);
        assert_eq!(reach, (W, H), "the frame not the view's size");
        assert!(scales.iter().all(|&k| k == backing), "tiles at {:?}", scales);
        assert!(resized.lock().unwrap().iter().all(|&k| k == backing));

        drop(surface);
        handle.close();
        // SAFETY: an NSWindow closed, on the main thread.
        let _: () = unsafe { msg_send![&*window, close] };
    }
}
