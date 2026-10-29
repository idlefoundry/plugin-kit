//! A instrument listening for agents kept awake. On macOS, App Nap slows a hidden application that
//! plays no audio until its window's frames, which carry the agents' requests out, come seconds
//! apart, and requests time out. While the socket is open the application tells the system
//! that it is doing what its user asked, and that its timing matters
//! (`NSActivityUserInitiatedAllowingIdleSystemSleep | NSActivityLatencyCritical`): App Nap lets
//! it be, and its frames' timer is not stretched while it is hidden. The computer may still
//! sleep, and only while agents are allowed is this asked. Elsewhere nothing is needed.
//!
//! And a standalone quitting (decisions.md R-QUIT-NAP): napped, everything after the quit
//! arrived ran at the lowest priority, and shutting down could outlast plugin-kit-standalone's
//! two seconds and end in the error window. From its window's closing until the process ends it
//! holds a user-initiated activity (`NSActivityUserInitiatedAllowingIdleSystemSleep`, not latency
//! critical): [`quitting`].

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
            _token: mac::Token::begin("Answering AI agents (SETTINGS > AGENTS)", mac::AGENTS),
        }
    }
}

/// The standalone has begun to quit: on macOS, App Nap lifted until the process ends, so that
/// its window closes, its session is saved and its audio and MIDI stop at their ordinary pace
/// however long it was hidden. Once a process; the activity is never ended, the process's end
/// ends it. Elsewhere nothing.
pub fn quitting() {
    #[cfg(target_os = "macos")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::mem::forget(mac::Token::begin(
                "Quitting: saving the session",
                mac::QUITTING,
            ));
        });
    }
}

#[cfg(any(target_os = "macos", test))]
mod policy {
    pub const QUITTING: u64 = 0x00FF_FFFF & !(1 << 20);
    pub const AGENTS: u64 = QUITTING | 0xFF_0000_0000;
}

#[cfg(target_os = "macos")]
mod mac {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;

    pub use super::policy::{AGENTS, QUITTING};

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
        pub fn begin(reason: &str, options: u64) -> Token {
            use objc::{class, msg_send, sel, sel_impl};
            // SAFETY: NSProcessInfo's shared instance and its activity methods, callable from
            // any thread. The reason is copied by the call and released after it; the token,
            // autoreleased, is retained until `drop` ends the activity and releases it.
            unsafe {
                let info: id = msg_send![class!(NSProcessInfo), processInfo];
                let reason = NSString::alloc(nil).init_str(reason);
                let token: id = msg_send![info, beginActivityWithOptions: options reason: reason];
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

#[cfg(test)]
mod tests {
    #[test]
    fn activity_policy_allows_sleep_and_only_agents_need_latency_critical() {
        assert_eq!(super::policy::QUITTING & (1 << 20), 0);
        assert_eq!(super::policy::QUITTING & 0xFF_0000_0000, 0);
        assert_eq!(
            super::policy::AGENTS,
            super::policy::QUITTING | 0xFF_0000_0000
        );
        super::quitting();
        super::quitting();
        let _awake = super::Awake::begin();
    }
}
