//! A plug-in instance's own helper thread (the CA-72's decisions.md R23): it does for the audio
//! thread what must not be done there, starting and stopping the instance's workers
//! ([`crate::Crew::serve`]) and anything else the plug-in gives it, when the audio thread asks
//! ([`Helper::ask`]: a bit set and an unpark, allocating nothing and waiting for nothing). In
//! between it looks at the other processes' seats every [`LOOK`], so that the shares follow
//! them ([`Budget::refresh`]).
//!
//! A host's own background thread (nih-plug's) would hold the plug-in alive while it runs a
//! task: a host that destroyed the plug-in then got back at once and could unload its library
//! under that thread and the voices' workers (on Windows, a crash: the CA-72's R23). The helper
//! is the plug-in's: dropped, it is stopped and joined (a task in progress finished first), so
//! no thread of its runs once the plug-in is gone; the plug-in then stops its pool.

use crate::budget::{Budget, LOOK};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

/// The ask for the instance's workers to be served ([`crate::Crew::serve`]); a plug-in's own
/// asks take the other bits.
pub const SERVE: u32 = 1;

#[derive(Debug, Default)]
struct Shared {
    asks: AtomicU32,
    quit: AtomicBool,
}

/// The helper thread, stopped and joined when dropped.
#[derive(Debug)]
pub struct Helper {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Helper {
    /// The helper started, named `name`, doing `work` with the asks made since it last did
    /// (never 0), and refreshing `budget`'s view of the other processes' seats meanwhile. Not on
    /// the audio thread.
    pub fn start<F>(name: &str, budget: &'static Budget, mut work: F) -> std::io::Result<Helper>
    where
        F: FnMut(u32) + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                while !s.quit.load(Ordering::Acquire) {
                    let asks = s.asks.swap(0, Ordering::AcqRel);
                    if asks == 0 {
                        // (An ask made since leaves the unpark's token: this returns at once.)
                        std::thread::park_timeout(LOOK);
                        budget.refresh();
                        continue;
                    }
                    work(asks);
                }
            })?;
        Ok(Helper {
            shared,
            thread: Some(thread),
        })
    }

    /// `asks` done as soon as the helper can. On the audio thread: allocates nothing and does
    /// not wait.
    pub fn ask(&self, asks: u32) {
        if asks == 0 {
            return;
        }
        self.shared.asks.fetch_or(asks, Ordering::Release);
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }
}

impl Drop for Helper {
    /// Stopped (a task in progress finished first) and joined. Not on the audio thread.
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static NONE: Budget = Budget::new(|_| 0, || 0, || None);

    /// Asked, the helper does the work with the asks; dropped mid-task, it is waited for.
    #[test]
    fn a_helper_works_when_asked_and_is_waited_for() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, hold) = (seen.clone(), Arc::new(AtomicBool::new(false)));
        let h2 = hold.clone();
        let helper = Helper::start("test-helper", &NONE, move |asks| {
            s.lock().unwrap().push(asks);
            while h2.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_micros(100));
            }
        })
        .unwrap();
        helper.ask(SERVE | 4);
        let t = Instant::now();
        while seen.lock().unwrap().is_empty() {
            assert!(t.elapsed() < Duration::from_secs(10), "never worked");
            std::thread::sleep(Duration::from_micros(200));
        }
        assert_eq!(seen.lock().unwrap()[0], SERVE | 4);
        hold.store(true, Ordering::Release);
        helper.ask(SERVE);
        let ended = Arc::new(AtomicBool::new(false));
        let e = ended.clone();
        let dropper = std::thread::spawn(move || {
            drop(helper);
            e.store(true, Ordering::Release);
        });
        std::thread::sleep(Duration::from_millis(100));
        hold.store(false, Ordering::Release);
        dropper.join().unwrap();
        assert!(ended.load(Ordering::Acquire));
        // (The work closure, and what it held, gone with the thread.)
        assert_eq!(Arc::strong_count(&seen), 1);
    }
}
