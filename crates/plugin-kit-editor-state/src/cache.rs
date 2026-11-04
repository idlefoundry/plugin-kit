//! Still parts built as an editor opens and shared while editors hold them, from the CA-72's
//! drawer `shared_still` and warming thread (R44). Unlike its check-then-build path, concurrent
//! opens reserve one build for a key. Pictures and their appearance stay with the caller.

use std::{
    collections::HashMap,
    fmt,
    hash::Hash,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, OnceLock, Weak},
    thread::{self, JoinHandle},
};

/// A still's caller identity, appearance generation and exact positive scale. The identity
/// must distinguish instruments and parts; the generation changes when their appearance does.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StillKey<I> {
    pub caller: I,
    pub appearance: u64,
    scale_bits: u64,
}

impl<I> StillKey<I> {
    pub fn new(caller: I, appearance: u64, scale: f64) -> Result<Self, &'static str> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("a still's scale must be finite and positive");
        }
        Ok(Self {
            caller,
            appearance,
            scale_bits: scale.to_bits(),
        })
    }

    pub fn scale(&self) -> f64 {
        f64::from_bits(self.scale_bits)
    }
}

/// A builder panicked. Its entry remains a failure while held, rather than staying pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildError;

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the still's builder panicked")
    }
}
impl std::error::Error for BuildError {}

type Built<V> = Result<Arc<V>, BuildError>;

struct Entry<V> {
    result: Arc<OnceLock<Built<V>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl<V> fmt::Debug for Entry<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("finished", &self.result.get().is_some())
            .finish_non_exhaustive()
    }
}

impl<V> Entry<V> {
    fn join(&self) {
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl<V> Drop for Entry<V> {
    fn drop(&mut self) {
        // The worker holds the result cell alone, never this entry. The last editor therefore
        // joins it here, never on the worker itself, before plug-in code can be unloaded.
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

/// An editor's lease on a build. Keep it while the editor uses the still, including after
/// [`Self::get`] returns pixels. The last lease joins its worker on drop; no thread is detached.
#[derive(Debug)]
pub struct Still<V> {
    entry: Arc<Entry<V>>,
}

impl<V> Clone for Still<V> {
    fn clone(&self) -> Self {
        Self {
            entry: Arc::clone(&self.entry),
        }
    }
}

impl<V> Still<V> {
    /// Poll without waiting for a builder. A ready value is shared, never cloned.
    pub fn get(&self) -> Option<Built<V>> {
        self.entry.result.get().cloned()
    }

    /// Wait when the picture is first needed, as the source drawer does. Ordinary frame
    /// polling should use [`Self::get`]. Waiting and dropping must not run on an audio thread.
    pub fn wait(&self) -> Built<V> {
        self.entry.join();
        self.get().unwrap_or(Err(BuildError))
    }
}

/// Share one cache between editors (usually one static cache for a part's value type).
/// Keys are caller-supplied; [`StillKey`] includes the required identity, generation and scale.
/// Only weak leases remain in the cache. Once every editor releases a key, its picture can
/// be freed and the next open builds afresh. Dead keys are removed at the next warm.
pub struct StillCache<K, V> {
    entries: Mutex<HashMap<K, Weak<Entry<V>>>>,
}

impl<K, V> fmt::Debug for StillCache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StillCache").finish_non_exhaustive()
    }
}

impl<K, V> Default for StillCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> StillCache<K, V> {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash, V: Send + Sync + 'static> StillCache<K, V> {
    /// Begin warming as the editor opens, or lease the existing build, even if still in
    /// flight. The closure runs once on its own thread, never under the cache's lock. A
    /// thread-start failure is returned and reserves nothing, so a later call can retry.
    pub fn warm(&self, key: K, make: impl FnOnce() -> V + Send + 'static) -> io::Result<Still<V>> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, entry| entry.strong_count() > 0);
        if let Some(entry) = entries.get(&key).and_then(Weak::upgrade) {
            return Ok(Still { entry });
        }
        let result = Arc::new(OnceLock::new());
        let produced = Arc::clone(&result);
        let worker = thread::Builder::new()
            .name("plugin-kit-still".into())
            .spawn(move || {
                let value = catch_unwind(AssertUnwindSafe(make))
                    .map(Arc::new)
                    .map_err(|_| BuildError);
                let _ = produced.set(value);
            })?;
        let entry = Arc::new(Entry {
            result,
            worker: Mutex::new(Some(worker)),
        });
        entries.insert(key, Arc::downgrade(&entry));
        Ok(Still { entry })
    }
}
