//! A instrument listening for agents kept awake. On macOS, App Nap slows a hidden application that
//! plays no audio until its window's frames, which carry the agents' requests out, come seconds
//! apart, and requests time out. While the socket is open the application tells the system
//! that it is doing what its user asked, and that its timing matters
//! (`NSActivityUserInitiatedAllowingIdleSystemSleep | NSActivityLatencyCritical`): App Nap lets
//! it be, and its frames' timer is not stretched while it is hidden. The computer may still
//! sleep, and only while agents are allowed is this asked. Elsewhere nothing is needed.

/// Held while agents may reach the application.
#[derive(Debug)]
pub struct Awake {
    #[cfg(target_os = "macos")]
    _token: mac::Token,
}

impl Awake {
    pub fn begin() -> Awake {
        Awake {
            #[cfg(target_os = "macos")]
            _token: mac::Token::begin("Answering AI agents (agents allowed)"),
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;

    /// `NSActivityUserInitiated` without `NSActivityIdleSystemSleepDisabled`, and
    /// `NSActivityLatencyCritical`.
    const OPTIONS: u64 = (0x00FF_FFFF & !(1 << 20)) | 0xFF_0000_0000;

    /// The activity NSProcessInfo gave, ended as it is dropped.
    #[derive(Debug)]
    pub struct Token(id);

    // SAFETY: an activity token is an immutable Foundation object that NSProcessInfo, which is
    // thread-safe, accepts back from any thread.
    #[allow(unsafe_code)]
    unsafe impl Send for Token {}

    // (objc 0.2's `msg_send!` asks after a `cargo-clippy` feature this crate does not have.)
    #[allow(unsafe_code, unexpected_cfgs)]
    impl Token {
        pub fn begin(reason: &str) -> Token {
            use objc::{class, msg_send, sel, sel_impl};
            // SAFETY: NSProcessInfo's shared instance and its activity methods, callable from
            // any thread. The reason is copied by the call and released after it; the token,
            // autoreleased, is retained until `drop` ends the activity and releases it.
            unsafe {
                let info: id = msg_send![class!(NSProcessInfo), processInfo];
                let reason = NSString::alloc(nil).init_str(reason);
                let token: id = msg_send![info, beginActivityWithOptions: OPTIONS reason: reason];
                let _: () = msg_send![reason, release];
                let _: id = msg_send![token, retain];
                Token(token)
            }
        }
    }

    #[allow(unsafe_code, unexpected_cfgs)]
    impl Drop for Token {
        fn drop(&mut self) {
            use objc::{class, msg_send, sel, sel_impl};
            if self.0 == nil {
                return;
            }
            // SAFETY: the token `begin` retained, ended once and released once.
            unsafe {
                let info: id = msg_send![class!(NSProcessInfo), processInfo];
                let _: () = msg_send![info, endActivity: self.0];
                let _: () = msg_send![self.0, release];
            }
        }
    }
}
