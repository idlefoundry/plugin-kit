//! Softbuffer implementation using CoreGraphics.
use crate::backend_interface::*;
use crate::error::InitError;
use crate::{util, Rect, SoftBufferError};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel, ClassType, MainThreadMarker};
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGImageComponentInfo, CGImagePixelFormatInfo,
};
use objc2_foundation::{
    ns_string, NSDictionary, NSKeyValueChangeKey, NSKeyValueChangeNewKey,
    NSKeyValueObservingOptions, NSNumber, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSString, NSValue,
};
use objc2_quartz_core::{kCAGravityTopLeft, CALayer, CATransaction};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};

use std::ffi::{c_void, CString};
use std::marker::PhantomData;
use std::mem::size_of;
use std::num::NonZeroU32;
use std::ops::Deref;
use std::ptr::{self, slice_from_raw_parts_mut, NonNull};
use std::sync::OnceLock;

/// The class of our observers, this copy of softbuffer's own.
///
/// Patched for the Idle Foundry plug-ins (`PATCHES.md`): upstream defines it with
/// `define_class!` under a fixed name, `SoftbufferObserver`. The Objective-C runtime holds one
/// class of a name per process, and a host that loads several plug-ins into one process may
/// hold several copies of softbuffer, one in each plug-in's library: the second copy to make a
/// surface could not define the class again, and panicked. This copy registers the class under
/// the first name of `SoftbufferObserver1`, `SoftbufferObserver2`, ... that no other has taken
/// (never upstream's, which an unpatched copy may still need), so each copy runs only its own
/// code. The class has no state: an observation's context is the layer it updates.
fn observer_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = (1u64..)
            .find_map(|n| {
                let name = CString::new(format!("SoftbufferObserver{n}")).unwrap();
                ClassBuilder::new(&name, NSObject::class())
            })
            .expect("no free name for softbuffer's observer class");
        // SAFETY: the signature is that of NSKeyValueObserving's
        // `-observeValueForKeyPath:ofObject:change:context:`, which the method overrides.
        unsafe {
            builder.add_method(
                sel!(observeValueForKeyPath:ofObject:change:context:),
                observe_value as unsafe extern "C-unwind" fn(_, _, _, _, _, _),
            );
        }
        builder.register()
    })
}

/// NSKeyValueObserving
unsafe extern "C-unwind" fn observe_value(
    _this: &NSObject,
    _cmd: Sel,
    key_path: Option<&NSString>,
    _object: Option<&AnyObject>,
    change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
    context: *mut c_void,
) {
    // SAFETY: the context is our layer, which `CGImpl` keeps until its `Drop` has removed the
    // observations (`CGImpl::new`).
    let layer = unsafe { &*context.cast::<CALayer>() };
    Observer::update(layer, key_path, change);
}

/// An observer: an instance of `observer_class()`.
#[derive(Debug)]
struct Observer(Retained<NSObject>);

// SAFETY: The class has no state of its own (upstream's observer was `Send` and `Sync` through
// its ivars, `SendCALayer`), and `NSObject` may be retained and released on any thread.
unsafe impl Send for Observer {}
// SAFETY: Same as above.
unsafe impl Sync for Observer {}

impl Deref for Observer {
    type Target = NSObject;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Observer {
    fn new() -> Self {
        // SAFETY: `+new` of a subclass of `NSObject` gives a new, initialized instance.
        Self(unsafe { msg_send![observer_class(), new] })
    }

    fn update(
        layer: &CALayer,
        key_path: Option<&NSString>,
        change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
    ) {
        let change =
            change.expect("requested a change dictionary in `addObserver`, but none was provided");
        let new = change
            .objectForKey(unsafe { NSKeyValueChangeNewKey })
            .expect("requested change dictionary did not contain `NSKeyValueChangeNewKey`");

        // NOTE: Setting these values usually causes a quarter second animation to occur, which is
        // undesirable.
        //
        // However, since we're setting them inside an observer, there already is a transaction
        // ongoing, and as such we don't need to wrap this in a `CATransaction` ourselves.

        if key_path == Some(ns_string!("contentsScale")) {
            let new = new.downcast::<NSNumber>().unwrap();
            let scale_factor = new.as_cgfloat();

            // Set the scale factor of the layer to match the root layer when it changes (e.g. if
            // moved to a different monitor, or monitor settings changed).
            layer.setContentsScale(scale_factor);
        } else if key_path == Some(ns_string!("bounds")) {
            let new = new.downcast::<NSValue>().unwrap();
            let bounds = new.get_rect().expect("new bounds value was not CGRect");

            // Set `bounds` and `position` so that the new layer is inside the superlayer.
            //
            // This differs from just setting the `bounds`, as it also takes into account any
            // translation that the superlayer may have that we'd want to preserve.
            layer.setFrame(bounds);
        } else {
            panic!("unknown observed keypath {key_path:?}");
        }
    }
}

#[derive(Debug)]
pub struct CGImpl<D, W> {
    /// Our layer.
    layer: SendCALayer,
    /// The layer that our layer was created from.
    ///
    /// Can also be retrieved from `layer.superlayer()`.
    root_layer: SendCALayer,
    observer: Observer,
    color_space: CFRetained<CGColorSpace>,
    /// The width of the underlying buffer.
    width: usize,
    /// The height of the underlying buffer.
    height: usize,
    window_handle: W,
    _display: PhantomData<D>,
}

impl<D, W> Drop for CGImpl<D, W> {
    fn drop(&mut self) {
        // SAFETY: Registered in `new`, must be removed before the observer is deallocated.
        unsafe {
            self.root_layer
                .removeObserver_forKeyPath(&self.observer, ns_string!("contentsScale"));
            self.root_layer
                .removeObserver_forKeyPath(&self.observer, ns_string!("bounds"));
        }
    }
}

impl<D: HasDisplayHandle, W: HasWindowHandle> SurfaceInterface<D, W> for CGImpl<D, W> {
    type Context = D;
    type Buffer<'a>
        = BufferImpl<'a, D, W>
    where
        Self: 'a;

    fn new(window_src: W, _display: &D) -> Result<Self, InitError<W>> {
        // `NSView`/`UIView` can only be accessed from the main thread.
        let _mtm = MainThreadMarker::new().ok_or(SoftBufferError::PlatformError(
            Some("can only access Core Graphics handles from the main thread".to_string()),
            None,
        ))?;

        let root_layer = match window_src.window_handle()?.as_raw() {
            RawWindowHandle::AppKit(handle) => {
                // SAFETY: The pointer came from `WindowHandle`, which ensures that the
                // `AppKitWindowHandle` contains a valid pointer to an `NSView`.
                //
                // We use `NSObject` here to avoid importing `objc2-app-kit`.
                let view: &NSObject = unsafe { handle.ns_view.cast().as_ref() };

                // Force the view to become layer backed
                let _: () = unsafe { msg_send![view, setWantsLayer: Bool::YES] };

                // SAFETY: `-[NSView layer]` returns an optional `CALayer`
                let layer: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
                layer.expect("failed making the view layer-backed")
            }
            RawWindowHandle::UiKit(handle) => {
                // SAFETY: The pointer came from `WindowHandle`, which ensures that the
                // `UiKitWindowHandle` contains a valid pointer to an `UIView`.
                //
                // We use `NSObject` here to avoid importing `objc2-ui-kit`.
                let view: &NSObject = unsafe { handle.ui_view.cast().as_ref() };

                // SAFETY: `-[UIView layer]` returns `CALayer`
                let layer: Retained<CALayer> = unsafe { msg_send![view, layer] };
                layer
            }
            _ => return Err(InitError::Unsupported(window_src)),
        };

        // Add a sublayer, to avoid interfering with the root layer, since setting the contents of
        // e.g. a view-controlled layer is brittle.
        let layer = CALayer::new();
        root_layer.addSublayer(&layer);

        // Set the anchor point and geometry. Softbuffer's uses a coordinate system with the origin
        // in the top-left corner.
        //
        // NOTE: This doesn't really matter unless we start modifying the `position` of our layer
        // ourselves, but it's nice to have in place.
        layer.setAnchorPoint(CGPoint::new(0.0, 0.0));
        layer.setGeometryFlipped(true);

        // Do not use auto-resizing mask.
        //
        // This is done to work around a bug in macOS 14 and above, where views using auto layout
        // may end up setting fractional values as the bounds, and that in turn doesn't propagate
        // properly through the auto-resizing mask and with contents gravity.
        //
        // Instead, we keep the bounds of the layer in sync with the root layer using an observer,
        // see below.
        //
        // layer.setAutoresizingMask(kCALayerHeightSizable | kCALayerWidthSizable);

        let observer = Observer::new();
        // Observe changes to the root layer's bounds and scale factor, and apply them to our layer.
        //
        // The previous implementation updated the scale factor inside `resize`, but this works
        // poorly with transactions, and is generally inefficient. Instead, we update the scale
        // factor only when needed because the super layer's scale factor changed.
        //
        // Note that inherent in this is an explicit design decision: We control the `bounds` and
        // `contentsScale` of the layer directly, and instead let the `resize` call that the user
        // controls only be the size of the underlying buffer.
        //
        // The observations' context is our layer, which the observer updates (patched,
        // `PATCHES.md`).
        let context: *mut c_void = Retained::as_ptr(&layer).cast_mut().cast();
        // SAFETY: Observer deregistered in `Drop` before the observer object is deallocated, and
        // before our layer is released.
        unsafe {
            root_layer.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("contentsScale"),
                NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Initial,
                context,
            );
            root_layer.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("bounds"),
                NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Initial,
                context,
            );
        }

        // Set the content so that it is placed in the top-left corner if it does not have the same
        // size as the surface itself.
        //
        // TODO(madsmtm): Consider changing this to `kCAGravityResize` to stretch the content if
        // resized to something that doesn't fit, see #177.
        layer.setContentsGravity(unsafe { kCAGravityTopLeft });

        // Initialize color space here, to reduce work later on.
        let color_space = CGColorSpace::new_device_rgb().unwrap();

        // Grab initial width and height from the layer (whose properties have just been initialized
        // by the observer using `NSKeyValueObservingOptionInitial`).
        let size = layer.bounds().size;
        let scale_factor = layer.contentsScale();
        let width = (size.width * scale_factor) as usize;
        let height = (size.height * scale_factor) as usize;

        Ok(Self {
            layer: SendCALayer(layer),
            root_layer: SendCALayer(root_layer),
            observer,
            color_space,
            width,
            height,
            _display: PhantomData,
            window_handle: window_src,
        })
    }

    #[inline]
    fn window(&self) -> &W {
        &self.window_handle
    }

    fn resize(&mut self, width: NonZeroU32, height: NonZeroU32) -> Result<(), SoftBufferError> {
        self.width = width.get() as usize;
        self.height = height.get() as usize;
        Ok(())
    }

    fn buffer_mut(&mut self) -> Result<BufferImpl<'_, D, W>, SoftBufferError> {
        Ok(BufferImpl {
            buffer: util::PixelBuffer(vec![0; self.width * self.height]),
            imp: self,
        })
    }
}

#[derive(Debug)]
pub struct BufferImpl<'a, D, W> {
    imp: &'a mut CGImpl<D, W>,
    buffer: util::PixelBuffer,
}

impl<D: HasDisplayHandle, W: HasWindowHandle> BufferInterface for BufferImpl<'_, D, W> {
    fn width(&self) -> NonZeroU32 {
        NonZeroU32::new(self.imp.width as u32).unwrap()
    }

    fn height(&self) -> NonZeroU32 {
        NonZeroU32::new(self.imp.height as u32).unwrap()
    }

    #[inline]
    fn pixels(&self) -> &[u32] {
        &self.buffer
    }

    #[inline]
    fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.buffer
    }

    fn age(&self) -> u8 {
        0
    }

    fn present(self) -> Result<(), SoftBufferError> {
        unsafe extern "C-unwind" fn release(
            _info: *mut c_void,
            data: NonNull<c_void>,
            size: usize,
        ) {
            let data = data.cast::<u32>();
            let slice = slice_from_raw_parts_mut(data.as_ptr(), size / size_of::<u32>());
            // SAFETY: This is the same slice that we passed to `Box::into_raw` below.
            drop(unsafe { Box::from_raw(slice) })
        }

        let data_provider = {
            let len = self.buffer.len() * size_of::<u32>();
            let buffer: *mut [u32] = Box::into_raw(self.buffer.0.into_boxed_slice());
            // Convert slice pointer to thin pointer.
            let data_ptr = buffer.cast::<c_void>();

            // SAFETY: The data pointer and length are valid.
            // The info pointer can safely be NULL, we don't use it in the `release` callback.
            unsafe {
                CGDataProvider::with_data(ptr::null_mut(), data_ptr, len, Some(release)).unwrap()
            }
        };

        // `CGBitmapInfo` consists of a combination of `CGImageAlphaInfo`, `CGImageComponentInfo`
        // `CGImageByteOrderInfo` and `CGImagePixelFormatInfo` (see e.g. `CGBitmapInfoMake`).
        //
        // TODO: Use `CGBitmapInfo::new` once the next version of objc2-core-graphics is released.
        let bitmap_info = CGBitmapInfo(
            CGImageAlphaInfo::NoneSkipFirst.0
                | CGImageComponentInfo::Integer.0
                | CGImageByteOrderInfo::Order32Little.0
                | CGImagePixelFormatInfo::Packed.0,
        );

        let image = unsafe {
            CGImage::new(
                self.imp.width,
                self.imp.height,
                8,
                32,
                self.imp.width * 4,
                Some(&self.imp.color_space),
                bitmap_info,
                Some(&data_provider),
                ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .unwrap();

        // The CALayer has a default action associated with a change in the layer contents, causing
        // a quarter second fade transition to happen every time a new buffer is applied. This can
        // be avoided by wrapping the operation in a transaction and disabling all actions.
        CATransaction::begin();
        CATransaction::setDisableActions(true);

        // SAFETY: The contents is `CGImage`, which is a valid class for `contents`.
        unsafe { self.imp.layer.setContents(Some(image.as_ref())) };

        CATransaction::commit();
        Ok(())
    }

    fn present_with_damage(self, _damage: &[Rect]) -> Result<(), SoftBufferError> {
        self.present()
    }
}

#[derive(Debug)]
struct SendCALayer(Retained<CALayer>);

// SAFETY: CALayer is dubiously thread safe, like most things in Core Animation.
// But since we make sure to do our changes within a CATransaction, it is
// _probably_ fine for us to use CALayer from different threads.
//
// See also:
// https://developer.apple.com/documentation/quartzcore/catransaction/1448267-lock?language=objc
// https://stackoverflow.com/questions/76250226/how-to-render-content-of-calayer-on-a-background-thread
unsafe impl Send for SendCALayer {}
// SAFETY: Same as above.
unsafe impl Sync for SendCALayer {}

impl Deref for SendCALayer {
    type Target = CALayer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
