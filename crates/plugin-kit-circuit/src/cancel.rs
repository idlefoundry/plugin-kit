//! A build stopped part way (a plug-in closed while it builds a voice: the host must not wait
//! minutes for it). A build run through [`cancellable`] looks at its token at points in its long
//! loops ([`point`]: the full model's steps, the bank's run, the scenarios, the build's phases)
//! and, once the token is set, unwinds to [`cancellable`], which returns `None`. The threads a
//! build spreads its work over carry the token too. Unwinding is the build's only way out: it
//! holds nothing that outlives it.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a cancelled build unwinds with.
#[derive(Debug)]
pub struct Cancelled;

thread_local! {
    static TOKEN: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// `f` run with `token`: `None` if the token was set while it ran (it stopped at its next
/// point), else its result. A panic of any other kind goes on unwinding.
pub fn cancellable<R>(token: Arc<AtomicBool>, f: impl FnOnce() -> R) -> Option<R> {
    let before = TOKEN.with(|t| t.replace(Some(token)));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    TOKEN.with(|t| *t.borrow_mut() = before);
    match r {
        Ok(v) => Some(v),
        Err(e) if e.is::<Cancelled>() => None,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// The calling thread's token, to hand to a thread it starts ([`with`]).
pub fn token() -> Option<Arc<AtomicBool>> {
    TOKEN.with(|t| t.borrow().clone())
}

/// `f` run with `token` set on this thread (a build's helper thread, [`token`]'s from its
/// parent).
pub fn with<R>(token: Option<Arc<AtomicBool>>, f: impl FnOnce() -> R) -> R {
    let before = TOKEN.with(|t| t.replace(token));
    let r = f();
    TOKEN.with(|t| *t.borrow_mut() = before);
    r
}

/// A point a build may stop at: the token looked at every call. (It once looked every 1024th:
/// a step of the hand clap's full model takes milliseconds, so that was seconds between looks,
/// D106; a look is a few nanoseconds, a step of any model at least tens of microseconds.)
#[inline]
pub fn point() {
    now();
}

/// A point a build may stop at, looked at every time (a phase's end).
pub fn now() {
    let set = TOKEN.with(|t| {
        t.borrow()
            .as_ref()
            .is_some_and(|t| t.load(Ordering::Relaxed))
    });
    if set {
        // Unwound without the panic hook: a host's log (nih-plug's hook writes every panic
        // there, with its backtrace) shows nothing for a build stopped on purpose.
        std::panic::resume_unwind(Box::new(Cancelled));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_token_stops_the_work_at_its_next_point() {
        let token = Arc::new(AtomicBool::new(false));
        let t = Arc::clone(&token);
        let mut done = 0;
        let r = cancellable(token, || {
            for i in 0..100_000 {
                if i == 5000 {
                    t.store(true, Ordering::Relaxed);
                }
                point();
                done = i;
            }
            7
        });
        assert_eq!(r, None);
        assert!(done < 5000 + 1024, "{done}");
        assert_eq!(cancellable(Arc::new(AtomicBool::new(false)), || 7), Some(7));
    }
}
