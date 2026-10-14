//! Reusable standalone services. Device access is confined to the control thread;
//! the callback owns its processor and only uses bounded queues and atomics.
mod devices;
mod persistence;
mod routing;
mod runtime;

pub use devices::{
    DeviceId, DeviceInfo, Devices, Negotiated, StreamCapability, negotiate, resolve_device,
};
pub use persistence::{SessionFile, load_session, save_session};
pub use routing::{MidiQueue, route_output, sanitize_input};
pub use runtime::{HostRuntime, spawn};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The identifier available from CPAL 0.15. Names are never treated as unique
/// without checking the current inventory; ambiguous identities are rejected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// None follows the system default output. Explicit devices never fall back.
    pub output: Option<DeviceId>,
    /// Zero-based hardware channel indices. Equal indices mean mono downmix.
    pub output_channels: [u16; 2],
    /// External input starts disabled.
    pub input: Option<DeviceId>,
    /// Equal indices duplicate one hardware input to the two auxiliary channels.
    pub input_channels: [u16; 2],
    /// Zero selects the output device's default sample rate.
    pub sample_rate: u32,
    /// Requested frames; actual callback sizes are reported separately.
    pub buffer_frames: u32,
    /// Empty means no hardware MIDI inputs.
    pub midi_inputs: Vec<String>,
    /// None receives all channels; Some(1)..Some(16) filters hardware MIDI.
    pub midi_channel: Option<u8>,
}

impl Config {
    pub fn initial() -> Self {
        Self {
            output_channels: [0, 1],
            buffer_frames: 256,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessConfig {
    pub sample_rate: f32,
    pub max_buffer_frames: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    /// Runtime connection identity (1..64); zero is reserved for local input.
    pub source: u16,
    pub data: [u8; 3],
    pub len: u8,
}

pub struct AudioBlock<'a> {
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
    pub input_left: &'a [f32],
    pub input_right: &'a [f32],
    pub midi: &'a [MidiEvent],
    /// Emergency all-sound-off before this block's MIDI events.
    pub panic: bool,
}

impl std::fmt::Debug for AudioBlock<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioBlock")
            .field("frames", &self.left.len())
            .field("midi", &self.midi)
            .field("panic", &self.panic)
            .finish()
    }
}

pub trait AudioProcessor: Send + 'static {
    /// Control thread, with all previous streams destroyed.
    fn prepare(&mut self, config: ProcessConfig) -> Result<(), String>;
    /// Audio thread: no allocation, locks, filesystem, network or GUI work.
    fn process(&mut self, block: AudioBlock<'_>);
    /// Control thread, with all streams destroyed. Release transient notes here.
    fn deactivate(&mut self);
}

/// State access must use shared parameter/UI state, not callback-owned DSP.
pub trait SessionState: Send + Sync + 'static {
    fn save(&self) -> Result<Vec<u8>, String>;
    /// Called on the control thread before a stream exists.
    fn restore(&self, bytes: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub app_id: String,
    pub state_dir: PathBuf,
    /// Test hosts can disable all device access; the editor still works.
    pub device_access: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Recovering,
    Error,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub config: Config,
    pub devices: Devices,
    pub phase: Phase,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub actual_sample_rate: u32,
    pub actual_buffer_frames: u32,
    pub xruns: u64,
    pub midi_overflows: u64,
    pub input_overflows: u64,
}

impl Snapshot {
    pub fn buffer_duration_ms(&self) -> Option<f64> {
        (self.actual_sample_rate > 0).then(|| {
            1000.0 * f64::from(self.actual_buffer_frames) / f64::from(self.actual_sample_rate)
        })
    }
}

pub(crate) enum Command {
    Apply(Config),
    Restore(Vec<u8>),
    Stop(String),
    Refresh,
    Save,
    Shutdown,
}

/// UI/control bridge. Snapshot locks are never acquired by an audio callback.
pub struct AppHandle {
    pub(crate) commands: Sender<Command>,
    pub(crate) snapshot: Mutex<Snapshot>,
    pub(crate) midi: Arc<MidiQueue>,
    pub(crate) metrics: Arc<runtime::Metrics>,
}

impl std::fmt::Debug for AppHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppHandle").finish_non_exhaustive()
    }
}

fn registry() -> &'static Mutex<Weak<AppHandle>> {
    static REGISTRY: OnceLock<Mutex<Weak<AppHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Weak::new()))
}

impl AppHandle {
    pub fn install_current(handle: &Arc<Self>) {
        *registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(handle);
    }
    pub fn current() -> Option<Arc<Self>> {
        registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut result = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.metrics.fill(&mut result);
        result.midi_overflows = self.midi.overflows();
        result
    }
    pub fn apply(&self, config: Config) -> Result<(), String> {
        self.submit(Command::Apply(config))
    }
    pub fn refresh(&self) -> Result<(), String> {
        self.submit(Command::Refresh)
    }
    pub fn save(&self) -> Result<(), String> {
        self.submit(Command::Save)
    }
    pub fn restore_state(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.submit(Command::Restore(bytes))
    }
    /// Report a processor fault from the UI/control thread and release streams.
    pub fn stop(&self, error: String) -> Result<(), String> {
        self.submit(Command::Stop(error))
    }
    pub fn panic(&self) {
        self.midi.panic();
    }
    pub fn midi(&self, event: MidiEvent) -> bool {
        self.midi.push(event)
    }
    fn submit(&self, command: Command) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|e| format!("Standalone command unavailable: {e}"))
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod allocations {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {static COUNT:Cell<Option<usize>>=const {Cell::new(None)};}
    struct Counter;
    #[global_allocator]
    static ALLOCATOR: Counter = Counter;
    // SAFETY: the allocator forwards every allocation/deallocation to System
    // unchanged; the optional thread-local counter records calls only.
    unsafe impl GlobalAlloc for Counter {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = COUNT.try_with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            // SAFETY: caller supplies the allocator's valid Layout.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: ptr/Layout belong to the System allocator above.
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let _ = COUNT.try_with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            // SAFETY: ptr/Layout/new size follow GlobalAlloc's contract.
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    pub fn count(f: impl FnOnce()) -> usize {
        COUNT.with(|count| count.set(Some(0)));
        f();
        COUNT.with(|count| count.replace(None).unwrap_or(0))
    }
}
