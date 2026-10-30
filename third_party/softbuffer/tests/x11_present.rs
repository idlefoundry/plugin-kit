//! A final frame must reach X11 even when the application draws nothing more.
#![cfg(all(target_os = "linux", feature = "x11"))]

use std::num::NonZeroU32;
use std::thread;
use std::time::{Duration, Instant};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle, XcbDisplayHandle, XcbWindowHandle,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, ImageFormat, WindowClass};

#[derive(Clone, Copy)]
struct Target(u32);
impl HasDisplayHandle for Target {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: None asks softbuffer to own its connection to the current display.
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Xcb(XcbDisplayHandle::new(None, 0)))
        })
    }
}
impl HasWindowHandle for Target {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the test's observer connection keeps this window alive until the surface drops.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Xcb(XcbWindowHandle::new(
                NonZeroU32::new(self.0).unwrap(),
            )))
        })
    }
}

#[test]
#[ignore = "requires an X11 display (e.g. Xvfb)"]
fn the_last_present_reaches_the_server_without_another_frame() {
    // Observe on another connection: reading pixels must not flush the presenter's queue.
    let (observer, screen) = x11rb::connect(None).unwrap();
    let screen = &observer.setup().roots[screen];
    let window = observer.generate_id().unwrap();
    observer
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            16,
            16,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .override_redirect(1),
        )
        .unwrap();
    observer.map_window(window).unwrap();
    observer.get_input_focus().unwrap().reply().unwrap();
    let target = Target(window);
    let context = softbuffer::Context::new(target).unwrap();
    let mut surface = softbuffer::Surface::new(&context, target).unwrap();
    surface
        .resize(NonZeroU32::new(16).unwrap(), NonZeroU32::new(16).unwrap())
        .unwrap();
    for (colour, partial) in [(0x00123456u32, false), (0x00654321, true)] {
        let mut buffer = surface.buffer_mut().unwrap();
        buffer.fill(colour);
        if partial {
            buffer
                .present_with_damage(&[softbuffer::Rect {
                    x: 0,
                    y: 0,
                    width: NonZeroU32::new(16).unwrap(),
                    height: NonZeroU32::new(16).unwrap(),
                }])
                .unwrap();
        } else {
            buffer.present().unwrap();
        }
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            let image = observer
                .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, 1, 1, u32::MAX)
                .unwrap()
                .reply()
                .unwrap();
            let pixel = u32::from_ne_bytes(image.data[..4].try_into().unwrap()) & 0x00ffffff;
            if pixel == colour {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "present stayed buffered: got {pixel:#x}, wanted {colour:#x}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    drop(surface);
    observer.destroy_window(window).unwrap();
    observer.flush().unwrap();
}
