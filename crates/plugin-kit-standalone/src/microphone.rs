//! Access to audio input. On macOS every input device is behind the microphone
//! permission, and opening one while its prompt waits for an answer waits too
//! (`CreateIOProcID` on the control thread, so the output would stop with it).
//! The control thread therefore reads the permission first, without waiting, and
//! asks for it with a request that answers later.

use crate::{Command, Negotiated, Snapshot};
use crossbeam_channel::Sender;

/// The permission as the operating system reports it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum Permission {
    Granted,
    /// Never asked: opening an input now would show the prompt and wait for it.
    Undetermined,
    /// Refused by the user, or not allowed on this machine (restricted).
    Denied,
}

/// An answer to a request, called once on any thread.
pub(crate) type Answer = Box<dyn FnOnce(bool) + Send>;

pub(crate) trait Microphone: Send {
    /// Never waits.
    fn status(&self) -> Permission;
    /// Shows the operating system's prompt and returns at once; `answered`
    /// learns the user's choice.
    fn request(&self, answered: Answer);
    /// The app as the privacy settings list it.
    fn app_name(&self) -> String;
}

pub(crate) fn system() -> Box<dyn Microphone> {
    #[cfg(target_os = "macos")]
    if let Some(tcc) = mac::Tcc::new() {
        return Box::new(tcc);
    }
    Box::new(Unrestricted)
}

/// Platforms (and unbundled macOS builds) where opening an input asks nothing
/// of the control thread: it is opened as it always was.
struct Unrestricted;

impl Microphone for Unrestricted {
    fn status(&self) -> Permission {
        Permission::Granted
    }
    fn request(&self, answered: Answer) {
        answered(true);
    }
    fn app_name(&self) -> String {
        "this app".into()
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    //! AVFoundation's `AVCaptureDevice` through the Objective-C runtime, and its
    //! completion handler as a global block (the Blocks ABI), so the kit takes no
    //! Objective-C crates.
    use super::{Answer, Microphone, Permission};
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::{Mutex, OnceLock};

    type Id = *const c_void;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: Id;
    }
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
    #[link(name = "System")]
    unsafe extern "C" {
        static _NSConcreteGlobalBlock: [*const c_void; 32];
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFBundleGetMainBundle() -> Id;
        fn CFBundleGetValueForInfoDictionaryKey(bundle: Id, key: Id) -> Id;
        fn CFStringCreateWithCString(allocator: Id, text: *const c_char, encoding: u32) -> Id;
        fn CFStringGetCString(text: Id, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFGetTypeID(value: Id) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(value: Id);
    }
    const UTF8: u32 = 0x0800_0100;

    /// A string from the main bundle's Info.plist, if it has one.
    fn info_string(key: &CStr) -> Option<String> {
        // SAFETY: the key is a valid C string and is released after the lookup;
        // the value belongs to the bundle (a Get function) and is only read here,
        // after its type is checked, into a buffer of the size passed.
        unsafe {
            let bundle = CFBundleGetMainBundle();
            if bundle.is_null() {
                return None;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), UTF8);
            if key.is_null() {
                return None;
            }
            let value = CFBundleGetValueForInfoDictionaryKey(bundle, key);
            CFRelease(key);
            if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
                return None;
            }
            let mut buffer = [0 as c_char; 512];
            (CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0).then(
                || {
                    CStr::from_ptr(buffer.as_ptr())
                        .to_string_lossy()
                        .into_owned()
                },
            )
        }
    }

    pub(super) struct Tcc {
        name: String,
    }

    impl Tcc {
        /// Only an app whose Info.plist says why it wants the microphone may ask:
        /// without the purpose string the system ends the process instead. A
        /// binary run from a terminal has none (its permission is the terminal's),
        /// and keeps the plain behaviour.
        pub(super) fn new() -> Option<Self> {
            info_string(c"NSMicrophoneUsageDescription")?;
            let name = info_string(c"CFBundleDisplayName")
                .or_else(|| info_string(c"CFBundleName"))
                .unwrap_or_else(|| "this app".into());
            Some(Self { name })
        }
    }

    fn capture_device_class() -> Id {
        // SAFETY: a constant C string naming an AVFoundation class, which is
        // linked above.
        unsafe { objc_getClass(c"AVCaptureDevice".as_ptr()) }
    }

    impl Microphone for Tcc {
        fn status(&self) -> Permission {
            // +[AVCaptureDevice authorizationStatusForMediaType:] returns an
            // NSInteger: 0 not determined, 1 restricted, 2 denied, 3 authorized.
            // SAFETY: the class and selector exist from macOS 10.14 (the app's
            // minimum is 11); objc_msgSend is called through the method's own
            // signature, inside an autorelease pool.
            let status = unsafe {
                let class = capture_device_class();
                if class.is_null() {
                    return Permission::Granted;
                }
                let send: unsafe extern "C" fn(Id, Id, Id) -> isize =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let pool = objc_autoreleasePoolPush();
                let status = send(
                    class,
                    sel_registerName(c"authorizationStatusForMediaType:".as_ptr()),
                    AVMediaTypeAudio,
                );
                objc_autoreleasePoolPop(pool);
                status
            };
            match status {
                3 => Permission::Granted,
                0 => Permission::Undetermined,
                _ => Permission::Denied,
            }
        }

        fn request(&self, answered: Answer) {
            answers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(answered);
            // SAFETY: as in `status`; the handler is a global block that lives
            // for the process, which AVFoundation may copy (a no-op for a global
            // block) and call once on a queue of its own.
            unsafe {
                let class = capture_device_class();
                if class.is_null() {
                    answer(std::ptr::null(), 1);
                    return;
                }
                let send: unsafe extern "C" fn(Id, Id, Id, *const Block) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let pool = objc_autoreleasePoolPush();
                send(
                    class,
                    sel_registerName(c"requestAccessForMediaType:completionHandler:".as_ptr()),
                    AVMediaTypeAudio,
                    handler(),
                );
                objc_autoreleasePoolPop(pool);
            }
        }

        fn app_name(&self) -> String {
            self.name.clone()
        }
    }

    /// Requests waiting for their answer; one handler serves them all.
    fn answers() -> &'static Mutex<Vec<Answer>> {
        static ANSWERS: OnceLock<Mutex<Vec<Answer>>> = OnceLock::new();
        ANSWERS.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// `void (^)(BOOL granted)` with nothing captured.
    #[repr(C)]
    struct Block {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*const Block, u8),
        descriptor: *const Descriptor,
    }
    #[repr(C)]
    struct Descriptor {
        reserved: usize,
        size: usize,
    }
    const BLOCK_IS_GLOBAL: i32 = 1 << 28;

    unsafe extern "C" fn answer(_block: *const Block, granted: u8) {
        let waiting = std::mem::take(
            &mut *answers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for answered in waiting {
            answered(granted != 0);
        }
    }

    fn handler() -> *const Block {
        static HANDLER: OnceLock<usize> = OnceLock::new();
        *HANDLER.get_or_init(|| {
            static DESCRIPTOR: Descriptor = Descriptor {
                reserved: 0,
                size: size_of::<Block>(),
            };
            let block = Box::leak(Box::new(Block {
                // The runtime's class for global blocks; only its address is taken.
                isa: (&raw const _NSConcreteGlobalBlock).cast(),
                flags: BLOCK_IS_GLOBAL,
                reserved: 0,
                invoke: answer,
                descriptor: &DESCRIPTOR,
            }));
            std::ptr::from_ref(block) as usize
        }) as *const Block
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The status call answers at once and asks nothing (a test binary has
        /// no purpose string, so it is never allowed to request).
        #[test]
        fn the_permission_is_read_without_waiting() {
            assert!(Tcc::new().is_none(), "an unbundled binary must not ask");
            let start = std::time::Instant::now();
            let tcc = Tcc {
                name: "Test".into(),
            };
            let _ = tcc.status();
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
            assert_eq!(handler(), handler(), "one handler for the process");
        }
    }
}

/// Why the running streams lack the chosen input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Held {
    #[default]
    No,
    Waiting,
    Denied,
}

/// The control thread's view of the permission: what may be opened, the one
/// request, and the status line it owns.
pub(crate) struct Gate {
    microphone: Box<dyn Microphone>,
    commands: Sender<Command>,
    asked: bool,
    held: Held,
    reported: Option<String>,
}

impl Gate {
    pub(crate) fn new(microphone: Box<dyn Microphone>, commands: Sender<Command>) -> Self {
        Self {
            microphone,
            commands,
            asked: false,
            held: Held::No,
            reported: None,
        }
    }

    /// The configuration that may be opened now: the input only once access is
    /// granted, the output in any case. An undetermined permission is asked for
    /// once; the answer comes back to the control thread as a command. The
    /// configuration keeps its input, so the choice is shown and saved.
    pub(crate) fn admit(&mut self, config: &Negotiated) -> Negotiated {
        self.held = match config.input {
            None => Held::No,
            Some(_) => match self.microphone.status() {
                Permission::Granted => Held::No,
                Permission::Undetermined => {
                    if !self.asked {
                        self.asked = true;
                        let commands = self.commands.clone();
                        self.microphone.request(Box::new(move |granted| {
                            // A full queue loses only this message: the next
                            // device poll reads the permission again.
                            let _ = commands.try_send(Command::Microphone(granted));
                        }));
                    }
                    Held::Waiting
                }
                Permission::Denied => Held::Denied,
            },
        };
        let mut admitted = config.clone();
        if self.held != Held::No {
            admitted.input = None;
            admitted.input_stream_channels = 0;
        }
        admitted
    }

    /// The user answered the prompt. True when the input may be opened now.
    pub(crate) fn answered(&mut self, granted: bool) -> bool {
        if self.held == Held::No {
            return false;
        }
        if !granted {
            self.held = Held::Denied;
        }
        granted
    }

    /// Read again on each device poll, for an answer whose message was lost or
    /// access allowed later in System Settings. True when the input may be
    /// opened now.
    pub(crate) fn recheck(&mut self) -> bool {
        if self.held == Held::No {
            return false;
        }
        match self.microphone.status() {
            Permission::Granted => true,
            Permission::Denied => {
                self.held = Held::Denied;
                false
            }
            Permission::Undetermined => false,
        }
    }

    pub(crate) fn notice(&self) -> Option<String> {
        match self.held {
            Held::No => None,
            Held::Waiting => Some(WAITING.into()),
            Held::Denied => Some(format!(
                "Microphone access denied. Allow {} in System Settings › Privacy & Security › Microphone.",
                self.microphone.app_name()
            )),
        }
    }

    /// Show the notice in the status, ahead of a device-list warning (`shared`,
    /// which republishes itself once this clears) but never over another
    /// owner's warning; clear only what this published.
    pub(crate) fn publish(&mut self, snapshot: &mut Snapshot, shared: Option<&String>) {
        let notice = self.notice();
        let current = snapshot.warning.as_ref();
        let ours = current.is_some() && current == self.reported.as_ref();
        let free = current.is_none() || current == shared;
        if notice.is_some() && (ours || free) {
            snapshot.warning.clone_from(&notice);
            self.reported = notice;
        } else if ours {
            snapshot.warning = None;
            self.reported = None;
        } else {
            self.reported = None;
        }
    }
}

pub(crate) const WAITING: &str =
    "Waiting for microphone permission… The external input starts once access is allowed.";
