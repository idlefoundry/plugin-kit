use crate::devices::{Enumeration, MidiDiscovery, describe, enumerate};
use crate::routing::{CAPTURE_CAPACITY, InputResampler, MIDI_PER_BLOCK, MidiRouter, pack_frame};
use crate::{
    AppHandle, AppOptions, AudioBlock, AudioProcessor, Command, Config, Devices, MidiEvent,
    MidiQueue, Negotiated, Phase, ProcessConfig, SessionFile, SessionState, Snapshot, load_session,
    negotiate, save_session,
};
use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded};
use plugin_kit_queue::{AtomicQueue, QueuePush};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Metrics {
    sample_rate: AtomicU32,
    frames: AtomicU32,
    errors: AtomicU64,
    input_overflows: AtomicU64,
    failed: AtomicBool,
    shutdown: AtomicBool,
    ready: AtomicBool,
    pub(crate) save_requested: AtomicBool,
}
impl Metrics {
    pub(crate) fn fill(&self, s: &mut Snapshot) {
        s.actual_sample_rate = self.sample_rate.load(Ordering::Relaxed);
        s.actual_buffer_frames = self.frames.load(Ordering::Relaxed);
        s.xruns = self.errors.load(Ordering::Relaxed);
        s.input_overflows = self.input_overflows.load(Ordering::Relaxed);
    }
}

pub struct HostRuntime {
    handle: Arc<AppHandle>,
    worker: Option<JoinHandle<()>>,
    finished: Receiver<Result<(), String>>,
    ready: Receiver<()>,
}
impl std::fmt::Debug for HostRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostRuntime").finish_non_exhaustive()
    }
}
impl HostRuntime {
    pub fn handle(&self) -> Arc<AppHandle> {
        Arc::clone(&self.handle)
    }
    /// Wait only for shared session restoration and stopped-DSP initialization,
    /// before asking the editor for its persisted initial size. Hardware opening
    /// follows on the control thread and does not delay this handshake.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        if self.handle.metrics.ready.load(Ordering::Acquire) {
            return Ok(());
        }
        self.ready
            .recv_timeout(timeout)
            .map_err(|_| "Standalone session initialization did not finish in time.".to_string())
    }
    /// Bounded caller wait. A malfunctioning OS driver cannot be safely killed;
    /// report that case rather than waiting forever for a missing callback.
    pub fn shutdown(&mut self) -> Result<(), String> {
        if self.worker.is_none() {
            return Ok(());
        }
        self.handle.metrics.shutdown.store(true, Ordering::Release);
        let _ = self.handle.commands.try_send(Command::Shutdown);
        let result = match self.finished.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                self.worker.take();
                return Err("Standalone shutdown did not finish within two seconds; a driver or session operation is still waiting.".into());
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err("Standalone control thread terminated unexpectedly.".into())
            }
        };
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "Standalone control thread failed".to_string())?;
        }
        result
    }
}
impl Drop for HostRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub fn spawn(
    processor: Box<dyn AudioProcessor>,
    state: Option<Arc<dyn SessionState>>,
    options: AppOptions,
) -> Result<HostRuntime, String> {
    std::fs::create_dir_all(&options.state_dir)
        .map_err(|e| format!("Unable to open standalone session directory: {e}"))?;
    let instance_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.state_dir.join("instance.lock"))
        .map_err(|e| format!("Unable to lock standalone session: {e}"))?;
    instance_lock.try_lock().map_err(|e|format!("Another standalone instance is using this session directory (or the session cannot be locked): {e}"))?;
    let (tx, rx) = bounded(32);
    let (finished_tx, finished) = bounded(1);
    let (ready_tx, ready) = bounded(1);
    let handle = Arc::new(AppHandle {
        commands: tx,
        snapshot: Mutex::new(Snapshot {
            config: Config::initial(),
            devices: Devices::default(),
            phase: Phase::Stopped,
            error: None,
            warning: None,
            actual_sample_rate: 0,
            actual_buffer_frames: 0,
            xruns: 0,
            midi_overflows: 0,
            input_overflows: 0,
        }),
        midi: Arc::new(MidiQueue::new()),
        metrics: Arc::new(Metrics::default()),
    });
    let control_handle = Arc::clone(&handle);
    let worker = std::thread::Builder::new()
        .name(format!("{} standalone", options.app_id))
        .spawn(move || {
            let result = control(processor, state, options, control_handle, rx, ready_tx);
            drop(instance_lock);
            let _ = finished_tx.try_send(result);
        })
        .map_err(|e| e.to_string())?;
    Ok(HostRuntime {
        handle,
        worker: Some(worker),
        finished,
        ready,
    })
}

fn set_status(handle: &AppHandle, phase: Phase, error: Option<String>) {
    let mut snapshot = handle
        .snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    snapshot.phase = phase;
    snapshot.error = error;
}

struct ProcessorOwner {
    processor: Option<Box<dyn AudioProcessor>>,
    returned: Sender<Box<dyn AudioProcessor>>,
}
impl Drop for ProcessorOwner {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            let _ = self.returned.try_send(processor);
        }
    }
}

struct Callback {
    owner: ProcessorOwner,
    midi: Arc<MidiQueue>,
    router: MidiRouter,
    input: InputResampler,
    metrics: Arc<Metrics>,
    left: Vec<f32>,
    right: Vec<f32>,
    input_left: Vec<f32>,
    input_right: Vec<f32>,
    events: [MidiEvent; MIDI_PER_BLOCK],
    pending_releases: u64,
    channels: usize,
    selection: [usize; 2],
}
impl Callback {
    fn render<T>(&mut self, data: &mut [T])
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        data.fill(T::from_sample(0.0));
        if self.channels == 0 || !data.len().is_multiple_of(self.channels) {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.metrics.frames.store(
            (data.len() / self.channels).min(u32::MAX as usize) as u32,
            Ordering::Relaxed,
        );
        self.input.begin(data.len() / self.channels);
        let max = self.left.len();
        for output in data.chunks_mut(max * self.channels) {
            let frames = output.len() / self.channels;
            self.left[..frames].fill(0.0);
            self.right[..frames].fill(0.0);
            for frame in 0..frames {
                let (input, underflow) = self.input.sample();
                if underflow {
                    self.metrics.errors.fetch_add(1, Ordering::Relaxed);
                }
                self.input_left[frame] = input[0];
                self.input_right[frame] = input[1];
            }
            let panic = self.midi.take_panic();
            if panic {
                self.router.clear();
            }
            let mut count = 0;
            self.pending_releases |= self.midi.take_releases();
            if self.pending_releases != 0 {
                let source = self.pending_releases.trailing_zeros() as u16 + 1;
                let (emitted, done) = self.router.release_source_into(source, &mut self.events);
                count = emitted;
                if done {
                    self.pending_releases &= !(1u64 << (source - 1));
                }
            }
            for _ in 0..MIDI_PER_BLOCK {
                if self.events.len() - count < 128 {
                    break;
                }
                let Some((event, valid)) = self.midi.pop_raw() else {
                    break;
                };
                if !valid {
                    continue;
                }
                if event.source != 0 && self.pending_releases & (1u64 << (event.source - 1)) != 0 {
                    continue;
                }
                count += self.router.route_into(event, &mut self.events[count..]);
            }
            if let Some(processor) = self.owner.processor.as_mut() {
                processor.process(AudioBlock {
                    left: &mut self.left[..frames],
                    right: &mut self.right[..frames],
                    input_left: &self.input_left[..frames],
                    input_right: &self.input_right[..frames],
                    midi: &self.events[..count],
                    panic,
                });
            }
            for (frame, hardware) in output.chunks_exact_mut(self.channels).enumerate() {
                let left = self.left[frame];
                let right = self.right[frame];
                let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
                if self.selection[0] == self.selection[1] {
                    hardware[self.selection[0]] = T::from_sample(finite(left * 0.5 + right * 0.5));
                } else {
                    hardware[self.selection[0]] = T::from_sample(finite(left));
                    hardware[self.selection[1]] = T::from_sample(finite(right));
                }
            }
        }
    }
}

struct Active {
    output: cpal::Stream,
    input: Option<cpal::Stream>,
    returned: Receiver<Box<dyn AudioProcessor>>,
}
impl Active {
    fn stop(self) -> Result<Box<dyn AudioProcessor>, String> {
        // Destruction returns ownership even if the device never called us once.
        drop(self.output);
        drop(self.input);
        self.returned
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Audio callback did not return its processor during shutdown.".to_string())
    }
}

fn find_device(host: &cpal::Host, id: &crate::DeviceId) -> Result<cpal::Device, String> {
    let mut matches = host
        .devices()
        .map_err(|e| e.to_string())?
        .filter(|d| describe(d).is_ok_and(|i| i.id == *id));
    let found = matches
        .next()
        .ok_or_else(|| format!("Audio device '{}' disappeared before opening.", id.name))?;
    if matches.next().is_some() {
        return Err(format!("Audio device '{}' is ambiguous.", id.name));
    }
    Ok(found)
}

fn stream_config(
    device: &cpal::Device,
    input: bool,
    channels: u16,
    rate: u32,
    frames: u32,
) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let ranges: Vec<_> = if input {
        device
            .supported_input_configs()
            .map_err(|e| e.to_string())?
            .collect()
    } else {
        device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .collect()
    };
    let format = choose_format(ranges, channels, rate, frames)?;
    Ok((
        cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Fixed(frames),
        },
        format,
    ))
}

fn choose_format(
    ranges: Vec<cpal::SupportedStreamConfigRange>,
    channels: u16,
    rate: u32,
    frames: u32,
) -> Result<cpal::SampleFormat, String> {
    let range = ranges
        .into_iter()
        .filter(|c| {
            c.channels() == channels
                && c.min_sample_rate().0 <= rate
                && rate <= c.max_sample_rate().0
                && match c.buffer_size() {
                    cpal::SupportedBufferSize::Range { min, max } => {
                        *min <= frames && frames <= *max
                    }
                    cpal::SupportedBufferSize::Unknown => true,
                }
        })
        .min_by_key(|c| {
            if c.sample_format() == cpal::SampleFormat::F32 {
                0
            } else {
                1
            }
        })
        .ok_or("Device no longer supports selected stream configuration")?;
    Ok(range.sample_format())
}

fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<AtomicQueue>,
    selected: [usize; 2],
    metrics: Arc<Metrics>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let errors = Arc::clone(&metrics);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for frame in data.chunks_exact(channels) {
                    let finite = |v: T| {
                        let v = f32::from_sample(v);
                        if v.is_finite() { v } else { 0.0 }
                    };
                    if queue.force_push(pack_frame([
                        finite(frame[selected[0]]),
                        finite(frame[selected[1]]),
                    ])) != QueuePush::Inserted
                    {
                        metrics.input_overflows.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
            move |_| {
                errors.errors.fetch_add(1, Ordering::Relaxed);
                errors.failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn build_output<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut callback: Callback,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let errors = Arc::clone(&callback.metrics);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| callback.render(data),
            move |_| {
                errors.errors.fetch_add(1, Ordering::Relaxed);
                errors.failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| e.to_string())
}

macro_rules! sample_stream {
    ($format:expr,$build:ident,$($argument:expr),*) => {
        match $format {
            cpal::SampleFormat::I8=>$build::<i8>($($argument),*),cpal::SampleFormat::I16=>$build::<i16>($($argument),*),
            cpal::SampleFormat::I32=>$build::<i32>($($argument),*),cpal::SampleFormat::I64=>$build::<i64>($($argument),*),
            cpal::SampleFormat::U8=>$build::<u8>($($argument),*),cpal::SampleFormat::U16=>$build::<u16>($($argument),*),
            cpal::SampleFormat::U32=>$build::<u32>($($argument),*),cpal::SampleFormat::U64=>$build::<u64>($($argument),*),
            cpal::SampleFormat::F32=>$build::<f32>($($argument),*),cpal::SampleFormat::F64=>$build::<f64>($($argument),*),
            _=>Err("Unsupported audio sample format".into()),
        }
    }
}

type StartFailure = (String, Option<Box<dyn AudioProcessor>>);
fn start(
    mut processor: Box<dyn AudioProcessor>,
    host: &cpal::Host,
    config: &Negotiated,
    handle: &AppHandle,
) -> Result<Active, StartFailure> {
    let output = match find_device(host, &config.output) {
        Ok(d) => d,
        Err(e) => return Err((e, Some(processor))),
    };
    let (output_config, format) = match stream_config(
        &output,
        false,
        config.output_stream_channels,
        config.sample_rate,
        config.buffer_frames,
    ) {
        Ok(c) => c,
        Err(e) => return Err((e, Some(processor))),
    };
    let max = config.buffer_frames as usize;
    let reset_scope = processor.midi_reset_scope();
    if let Err(e) = processor.prepare(ProcessConfig {
        sample_rate: config.sample_rate as f32,
        max_buffer_frames: max,
    }) {
        return Err((e, Some(processor)));
    }
    let input_queue = Arc::new(AtomicQueue::new(CAPTURE_CAPACITY));
    let mut input_stream = None;
    if let Some(input) = &config.input {
        let result: Result<cpal::Stream, String> = (|| {
            let device = find_device(host, input)?;
            let (config_stream, format) = stream_config(
                &device,
                true,
                config.input_stream_channels,
                config.sample_rate,
                config.buffer_frames,
            )?;
            let stream = sample_stream!(
                format,
                build_input,
                &device,
                &config_stream,
                Arc::clone(&input_queue),
                config.config.input_channels.map(usize::from),
                Arc::clone(&handle.metrics)
            )?;
            Ok(stream)
        })();
        match result {
            Ok(stream) => input_stream = Some(stream),
            Err(e) => {
                processor.deactivate();
                return Err((
                    format!("External input could not start: {e}"),
                    Some(processor),
                ));
            }
        }
    }
    let (returned_tx, returned_rx) = bounded(1);
    let callback = Callback {
        owner: ProcessorOwner {
            processor: Some(processor),
            returned: returned_tx,
        },
        midi: Arc::clone(&handle.midi),
        router: MidiRouter::with_scope(config.config.midi_channel, reset_scope),
        input: InputResampler::new(input_queue, config.input.is_some(), max),
        metrics: Arc::clone(&handle.metrics),
        left: vec![0.0; max],
        right: vec![0.0; max],
        input_left: vec![0.0; max],
        input_right: vec![0.0; max],
        events: [MidiEvent {
            source: 0,
            data: [0; 3],
            len: 0,
        }; MIDI_PER_BLOCK],
        pending_releases: 0,
        channels: usize::from(config.output_stream_channels),
        selection: config.config.output_channels.map(usize::from),
    };
    let output_stream =
        match sample_stream!(format, build_output, &output, &output_config, callback) {
            Ok(stream) => stream,
            Err(e) => return Err((e, returned_rx.recv_timeout(Duration::from_secs(2)).ok())),
        };
    handle.metrics.failed.store(false, Ordering::Release);
    if let Some(input) = &input_stream
        && let Err(e) = input.play()
    {
        drop(output_stream);
        drop(input_stream);
        return Err((
            format!("External input could not start: {e}"),
            returned_rx.recv_timeout(Duration::from_secs(2)).ok(),
        ));
    }
    if let Err(e) = output_stream.play() {
        drop(output_stream);
        return Err((
            e.to_string(),
            returned_rx.recv_timeout(Duration::from_secs(2)).ok(),
        ));
    }
    handle
        .metrics
        .sample_rate
        .store(config.sample_rate, Ordering::Relaxed);
    handle
        .metrics
        .frames
        .store(config.buffer_frames, Ordering::Relaxed);
    Ok(Active {
        output: output_stream,
        input: input_stream,
        returned: returned_rx,
    })
}

struct MidiBinding {
    name: String,
    source: u16,
    _connection: midir::MidiInputConnection<()>,
}

fn connect_midi(
    config: &Config,
    app_id: &str,
    handle: &AppHandle,
) -> (Vec<MidiBinding>, Vec<String>) {
    let mut connections = Vec::new();
    let warnings = reconcile_midi(config, app_id, handle, &mut connections);
    (connections, warnings)
}

fn reconcile_midi(
    config: &Config,
    app_id: &str,
    handle: &AppHandle,
    connections: &mut Vec<MidiBinding>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let inventory = handle.snapshot().devices.midi_inputs;
    for index in (0..connections.len()).rev() {
        let binding = &connections[index];
        if !config.midi_inputs.contains(&binding.name)
            || inventory
                .iter()
                .filter(|name| *name == &binding.name)
                .count()
                != 1
        {
            let removed = connections.remove(index);
            let source = removed.source;
            drop(removed);
            handle.midi.release_source(source);
        }
    }
    for (index, name) in config.midi_inputs.iter().enumerate().take(64) {
        if connections.iter().any(|binding| binding.name == *name) {
            continue;
        }
        let result = (|| {
            let mut input = midir::MidiInput::new(app_id).map_err(|e| e.to_string())?;
            input.ignore(midir::Ignore::All);
            let ports = input.ports();
            let matches: Vec<_> = ports
                .iter()
                .filter(|p| input.port_name(p).is_ok_and(|n| n == *name))
                .collect();
            if matches.len() != 1 {
                return Err(format!("MIDI input '{name}' is unavailable or ambiguous."));
            }
            let queue = Arc::clone(&handle.midi);
            let source = index as u16 + 1;
            input
                .connect(
                    matches[0],
                    app_id,
                    move |_, bytes, _| {
                        if !(1..=3).contains(&bytes.len()) {
                            return;
                        }
                        let mut data = [0; 3];
                        data[..bytes.len()].copy_from_slice(bytes);
                        queue.push(MidiEvent {
                            source,
                            data,
                            len: bytes.len() as u8,
                        });
                    },
                    (),
                )
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(connection) => connections.push(MidiBinding {
                name: name.clone(),
                source: index as u16 + 1,
                _connection: connection,
            }),
            Err(error) => warnings.push(error),
        }
    }
    warnings
}

#[derive(PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    hash: u64,
}
struct PreservedSession {
    fingerprint: Option<FileFingerprint>,
}
fn fingerprint(path: &std::path::Path) -> Result<Option<FileFingerprint>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    Ok(Some(FileFingerprint {
        len,
        hash: hash.finish(),
    }))
}
impl PreservedSession {
    fn capture(options: &AppOptions) -> Self {
        Self {
            fingerprint: fingerprint(&options.state_dir.join("session.json"))
                .ok()
                .flatten(),
        }
    }
}

fn store(
    options: &AppOptions,
    state: Option<&Arc<dyn SessionState>>,
    config: &Config,
    preserved: &mut Option<PreservedSession>,
) -> Result<(), String> {
    if let Some(original) = preserved {
        let current = fingerprint(&options.state_dir.join("session.json"))?;
        if current.is_none() {
            *preserved = None;
        } else if current == original.fingerprint {
            return Err("The original unreadable session is preserved. Move it aside before saving a new session.".into());
        } else {
            return Err("A different file now occupies the preserved session path. Move it aside before saving a new session.".into());
        }
    }
    let bytes = state.map(|s| s.save()).transpose()?.unwrap_or_default();
    save_session(
        &options.state_dir.join("session.json"),
        &SessionFile::new(options.app_id.clone(), config.clone(), bytes),
    )
}

fn report_store(handle: &AppHandle, result: &Result<(), String>) {
    let mut snapshot = handle
        .snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match result {
        Err(error) => snapshot.warning = Some(error.clone()),
        Ok(()) => {
            if snapshot.warning.as_ref().is_some_and(|warning| {
                warning.to_lowercase().contains("session") && warning.contains("preserved")
            }) {
                snapshot.warning = None;
            }
        }
    }
}

struct Transition<T> {
    stream: Option<T>,
    processor: Option<Box<dyn AudioProcessor>>,
    config: Option<Config>,
    accepted: bool,
    error: Option<String>,
    warning: Option<String>,
}

/// The same transaction policy is used for real streams and deterministic fake
/// drivers. Failed opens return the processor; unavailable rollback devices must
/// never accidentally consume/drop that ownership.
fn transition<T>(
    processor: Box<dyn AudioProcessor>,
    candidate: &Negotiated,
    previous: Option<&Config>,
    devices: &Devices,
    mut open: impl FnMut(Box<dyn AudioProcessor>, &Negotiated) -> Result<T, StartFailure>,
) -> Transition<T> {
    let mut candidate = candidate.clone();
    let mut warning = None;
    let opened = match open(processor, &candidate) {
        Err((error, Some(processor))) if error.starts_with("External input could not start") => {
            warning = Some(format!("{error} Continuing with external input disabled."));
            candidate.input = None;
            candidate.input_stream_channels = 0;
            candidate.config.input = None;
            open(processor, &candidate)
        }
        result => result,
    };
    match opened {
        Ok(stream) => Transition {
            stream: Some(stream),
            processor: None,
            config: Some(candidate.config),
            accepted: true,
            error: None,
            warning,
        },
        Err((error, mut processor)) => {
            if let Some(processor) = processor.as_mut() {
                processor.deactivate();
            }
            if let Some(previous) = previous
                && let Ok(previous) = negotiate(previous, devices)
                && let Some(owned) = processor.take()
            {
                match open(owned, &previous) {
                    Ok(stream) => {
                        return Transition {
                            stream: Some(stream),
                            processor: None,
                            config: Some(previous.config),
                            accepted: false,
                            error: Some(format!("Unable to apply settings: {error}")),
                            warning,
                        };
                    }
                    Err((_, returned)) => processor = returned,
                }
            }
            if let Some(processor) = processor.as_mut() {
                processor.deactivate();
            }
            Transition {
                stream: None,
                processor,
                config: None,
                accepted: false,
                error: Some(format!("Unable to apply settings: {error}")),
                warning,
            }
        }
    }
}

fn should_retry_after_poll(active: bool, stopped_by_user: bool, pending: bool) -> bool {
    !active && !stopped_by_user && !pending
}

/// Resolve the default once, when an input actually appears. Retaining the
/// selected name prevents discovery order or an unplug from switching sources.
fn resolve_default_midi(config: &mut Config, devices: &Devices) -> bool {
    if !config.auto_midi || !config.midi_inputs.is_empty() {
        return false;
    }
    let Some(name) = devices.midi_inputs.iter().find(|name| {
        !name.trim().is_empty()
            && devices
                .midi_inputs
                .iter()
                .filter(|other| *other == *name)
                .take(2)
                .count()
                == 1
    }) else {
        return false;
    };
    config.midi_inputs.push(name.clone());
    true
}

#[derive(Default)]
struct MidiWarnings {
    empty: Option<String>,
    connection: Option<String>,
    reported: Option<String>,
}

impl MidiWarnings {
    fn inventory(&mut self, previous: usize, current: usize) {
        if current > 0 {
            self.empty = None;
        } else if previous > 0 && self.empty.is_none() {
            self.empty = Some(format!(
                "No MIDI inputs available (previously {previous}). Reconnect the controller or Refresh."
            ));
        }
    }

    fn connections(&mut self, warnings: Vec<String>) {
        self.connection = (!warnings.is_empty()).then(|| warnings.join(" "));
    }

    fn publish(&mut self, snapshot: &mut Snapshot) {
        // Session/audio warnings have priority. Clear or replace only a warning
        // this MIDI owner actually published, including after successful retry.
        if snapshot.warning.is_some() && snapshot.warning != self.reported {
            self.reported = None;
            return;
        }
        let warnings: Vec<_> = self
            .empty
            .iter()
            .chain(self.connection.iter())
            .cloned()
            .collect();
        self.reported = (!warnings.is_empty()).then(|| warnings.join(" "));
        snapshot.warning.clone_from(&self.reported);
    }
}

fn publish_inventory(
    snapshot: &mut Snapshot,
    midi_error: &mut Option<String>,
    warnings: &mut MidiWarnings,
    mut enumeration: Enumeration,
) -> bool {
    if midi_error.is_some() && snapshot.error == *midi_error {
        snapshot.error = None;
    }
    *midi_error = enumeration.midi_error.map(|error| {
        format!("MIDI discovery failed: {error} Previous inputs retained; Refresh retries.")
    });
    if midi_error.is_some() {
        enumeration
            .devices
            .midi_inputs
            .clone_from(&snapshot.devices.midi_inputs);
        ensure_midi_error(snapshot, midi_error);
    } else {
        warnings.inventory(
            snapshot.devices.midi_inputs.len(),
            enumeration.devices.midi_inputs.len(),
        );
    }
    let changed = snapshot.devices != enumeration.devices;
    snapshot.devices = enumeration.devices;
    warnings.publish(snapshot);
    changed
}

fn ensure_midi_error(snapshot: &mut Snapshot, error: &Option<String>) {
    if snapshot.error.is_none() {
        snapshot.error.clone_from(error);
    }
}

fn control(
    mut initial: Box<dyn AudioProcessor>,
    state: Option<Arc<dyn SessionState>>,
    options: AppOptions,
    handle: Arc<AppHandle>,
    commands: Receiver<Command>,
    ready: Sender<()>,
) -> Result<(), String> {
    let mut config = Config::initial();
    let mut preserve_bad = None;
    match load_session(&options.state_dir.join("session.json"), &options.app_id) {
        Ok(Some(session)) => {
            config = session.config;
            if let Some(state) = &state
                && let Err(e) = state.restore(&session.plugin_state)
            {
                preserve_bad = Some(PreservedSession::capture(&options));
                handle
                    .snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .warning = Some(e);
            }
        }
        Ok(None) => {}
        Err(e) => {
            preserve_bad = Some(PreservedSession::capture(&options));
            handle
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .warning = Some(e);
        }
    }
    // Initialize stopped DSP once so parameter editing/state methods have their
    // expected lifecycle even when no audio hardware is available.
    if let Err(e) = initial.prepare(ProcessConfig {
        sample_rate: 48000.0,
        max_buffer_frames: 256,
    }) {
        set_status(&handle, Phase::Error, Some(e));
    }
    initial.deactivate();
    handle.metrics.ready.store(true, Ordering::Release);
    let _ = ready.try_send(());
    let host = options.device_access.then(cpal::default_host);
    let mut processor = Some(initial);
    let mut active: Option<Active> = None;
    let mut midi = Vec::new();
    let mut successful: Option<Config> = None;
    let mut active_output: Option<crate::DeviceId> = None;
    let mut midi_discovery = MidiDiscovery::default();
    let mut midi_discovery_error = None;
    let mut midi_warnings = MidiWarnings::default();
    let mut last_poll = Instant::now() - Duration::from_secs(2);
    let mut last_save = Instant::now();
    let mut need_apply = options.device_access;
    let mut stopped_by_user = false;
    let mut candidate = config.clone();
    handle
        .snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .config = config.clone();
    loop {
        if handle.metrics.shutdown.load(Ordering::Acquire) {
            break;
        }
        let command = commands.recv_timeout(Duration::from_millis(100)).ok();
        let mut apply_reply = None;
        match command {
            Some(Command::Shutdown) => break,
            Some(Command::Apply(new)) => {
                candidate = new;
                need_apply = true;
                stopped_by_user = false;
            }
            Some(Command::ApplyTracked(new, reply)) => {
                candidate = new;
                need_apply = true;
                stopped_by_user = false;
                apply_reply = Some(reply);
            }
            Some(Command::Stop(error)) => {
                handle.midi.panic();
                midi.clear();
                midi_warnings.connections(Vec::new());
                if let Some(stream) = active.take() {
                    match stream.stop() {
                        Ok(mut p) => {
                            p.deactivate();
                            processor = Some(p);
                        }
                        Err(e) => set_status(&handle, Phase::Error, Some(e)),
                    }
                }
                need_apply = false;
                stopped_by_user = true;
                handle.metrics.sample_rate.store(0, Ordering::Relaxed);
                handle.metrics.frames.store(0, Ordering::Relaxed);
                set_status(&handle, Phase::Error, Some(error));
            }
            Some(Command::Refresh) => {
                last_poll = Instant::now() - Duration::from_secs(2);
            }
            Some(Command::Save) => {
                handle.metrics.save_requested.store(true, Ordering::Release);
            }
            Some(Command::Restore(bytes)) => {
                handle.midi.panic();
                midi.clear();
                midi_warnings.connections(Vec::new());
                if let Some(stream) = active.take() {
                    match stream.stop() {
                        Ok(mut p) => {
                            p.deactivate();
                            processor = Some(p);
                        }
                        Err(e) => set_status(&handle, Phase::Error, Some(e)),
                    }
                }
                if let Some(state) = &state {
                    match state.restore(&bytes) {
                        Ok(()) => {}
                        Err(e) => set_status(&handle, Phase::Error, Some(e)),
                    }
                }
                candidate = config.clone();
                need_apply = options.device_access;
            }
            None => {}
        }
        if last_poll.elapsed() >= Duration::from_secs(2) {
            last_poll = Instant::now();
            if let Some(host) = &host {
                match enumerate(host, &options.app_id, &mut midi_discovery) {
                    Ok(enumeration) => {
                        let changed = {
                            let mut snapshot = handle
                                .snapshot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            publish_inventory(
                                &mut snapshot,
                                &mut midi_discovery_error,
                                &mut midi_warnings,
                                enumeration,
                            )
                        };
                        // A server/driver can reconnect with exactly the same
                        // device identity. Retry after each successful poll while
                        // stopped, rather than requiring a device-list change.
                        if should_retry_after_poll(active.is_some(), stopped_by_user, need_apply) {
                            candidate = config.clone();
                            need_apply = true;
                        }
                        if changed && active.is_some() {
                            let devices = handle.snapshot().devices;
                            if !need_apply && resolve_default_midi(&mut config, &devices) {
                                successful = Some(config.clone());
                                handle
                                    .snapshot
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .config = config.clone();
                                handle.metrics.save_requested.store(true, Ordering::Release);
                            }
                            if !need_apply
                                && config.output.is_none()
                                && devices.default_output != active_output
                            {
                                candidate = config.clone();
                                need_apply = true;
                            } else if negotiate(&config, &devices).is_err() {
                                handle.metrics.failed.store(true, Ordering::Release);
                            }
                        }
                        // Healthy polling also retries a failed MIDI connection
                        // with unchanged identity. Existing bindings stay open.
                        if active.is_some() && !need_apply && midi_discovery_error.is_none() {
                            midi_warnings.connections(reconcile_midi(
                                &config,
                                &options.app_id,
                                &handle,
                                &mut midi,
                            ));
                        }
                    }
                    Err(e) => {
                        // ALSA can stop delivering callbacks when its server
                        // disappears without issuing CPAL's stream error callback.
                        if active.is_some() {
                            handle.metrics.failed.store(true, Ordering::Release);
                        }
                        set_status(&handle, Phase::Error, Some(e));
                    }
                }
            }
        }
        if handle.metrics.failed.swap(false, Ordering::AcqRel) {
            handle.midi.panic();
            midi.clear();
            midi_warnings.connections(Vec::new());
            if let Some(stream) = active.take() {
                match stream.stop() {
                    Ok(mut p) => {
                        p.deactivate();
                        processor = Some(p);
                    }
                    Err(e) => set_status(&handle, Phase::Error, Some(e)),
                }
            }
            set_status(
                &handle,
                Phase::Recovering,
                Some(
                    "Audio device stopped. Reconnect it or choose another device and Apply.".into(),
                ),
            );
            handle.metrics.sample_rate.store(0, Ordering::Relaxed);
            handle.metrics.frames.store(0, Ordering::Relaxed);
        }
        if need_apply {
            need_apply = false;
            let mut accepted = false;
            let devices = handle.snapshot().devices;
            resolve_default_midi(&mut candidate, &devices);
            let negotiated = negotiate(&candidate, &devices);
            if let (Some(host), Ok(negotiated)) = (&host, negotiated.as_ref()) {
                set_status(&handle, Phase::Starting, None);
                handle.midi.panic();
                midi.clear();
                midi_warnings.connections(Vec::new());
                if let Some(stream) = active.take() {
                    match stream.stop() {
                        Ok(mut p) => {
                            p.deactivate();
                            processor = Some(p);
                        }
                        Err(e) => set_status(&handle, Phase::Error, Some(e)),
                    }
                }
                if let Some(p) = processor.take() {
                    let result =
                        transition(p, negotiated, successful.as_ref(), &devices, |p, n| {
                            start(p, host, n, &handle)
                        });
                    active = result.stream;
                    active_output = active
                        .as_ref()
                        .and(result.config.as_ref())
                        .and_then(|c| c.output.clone().or_else(|| devices.default_output.clone()));
                    processor = result.processor;
                    if let Some(actual) = result.config {
                        config = actual;
                    }
                    if result.accepted {
                        successful = Some(config.clone());
                        accepted = true;
                    }
                    if active.is_some() {
                        let (connections, warnings) =
                            connect_midi(&config, &options.app_id, &handle);
                        midi = connections;
                        midi_warnings.connections(warnings);
                        {
                            let mut snapshot = handle
                                .snapshot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            snapshot.config = config.clone();
                            snapshot.phase = Phase::Running;
                            snapshot.error = result.error;
                            if let Some(warning) = result.warning {
                                snapshot.warning = Some(warning);
                            }
                        }
                        if result.accepted {
                            report_store(
                                &handle,
                                &store(&options, state.as_ref(), &config, &mut preserve_bad),
                            );
                        }
                    } else {
                        handle.metrics.sample_rate.store(0, Ordering::Relaxed);
                        handle.metrics.frames.store(0, Ordering::Relaxed);
                        set_status(&handle, Phase::Error, result.error);
                    }
                }
            } else if let Err(e) = negotiated {
                set_status(
                    &handle,
                    if active.is_some() {
                        Phase::Running
                    } else {
                        Phase::Error
                    },
                    Some(e),
                );
            }
            if let Some(reply) = apply_reply {
                let outcome = if accepted {
                    Ok(config.clone())
                } else {
                    Err(handle.snapshot().error.unwrap_or_else(|| {
                        "Settings could not be applied; no audio device is running.".into()
                    }))
                };
                // A dismissed UI drops its receiver; completion never blocks.
                let _ = reply.try_send(outcome);
            }
        }
        if last_save.elapsed() >= Duration::from_secs(5)
            || (handle.metrics.save_requested.load(Ordering::Acquire)
                && last_save.elapsed() >= Duration::from_millis(250))
        {
            last_save = Instant::now();
            handle
                .metrics
                .save_requested
                .store(false, Ordering::Release);
            let result = store(
                &options,
                state.as_ref(),
                successful.as_ref().unwrap_or(&config),
                &mut preserve_bad,
            );
            report_store(&handle, &result);
        }
        // Lifecycle operations may clear the ordinary status error. Keep an
        // unresolved discovery failure visible without replacing audio errors.
        {
            let mut snapshot = handle
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ensure_midi_error(&mut snapshot, &midi_discovery_error);
            midi_warnings.publish(&mut snapshot);
        }
    }
    handle.midi.panic();
    midi.clear();
    midi_warnings.connections(Vec::new());
    let mut shutdown_error = None;
    if let Some(stream) = active {
        match stream.stop() {
            Ok(mut p) => p.deactivate(),
            Err(e) => shutdown_error = Some(e),
        }
    }
    if let Some(mut p) = processor {
        p.deactivate();
    }
    let saved = store(
        &options,
        state.as_ref(),
        successful.as_ref().unwrap_or(&config),
        &mut preserve_bad,
    );
    report_store(&handle, &saved);
    if let Err(error) = saved {
        shutdown_error = Some(error);
    }
    set_status(&handle, Phase::Stopped, shutdown_error.clone());
    shutdown_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_inventory_snapshot() -> Snapshot {
        Snapshot {
            config: Config::initial(),
            devices: crate::devices::tests::inventory(2),
            phase: Phase::Running,
            error: None,
            warning: None,
            actual_sample_rate: 48000,
            actual_buffer_frames: 256,
            xruns: 0,
            midi_overflows: 0,
            input_overflows: 0,
        }
    }

    #[test]
    fn successful_midi_retry_clears_only_the_owned_unavailable_warning() {
        let mut snapshot = running_inventory_snapshot();
        let mut warnings = MidiWarnings::default();
        warnings.connections(vec!["MIDI input 'Keys' is unavailable.".into()]);
        warnings.publish(&mut snapshot);
        assert!(
            snapshot
                .warning
                .as_ref()
                .expect("connection warning")
                .contains("unavailable")
        );
        warnings.connections(Vec::new());
        warnings.publish(&mut snapshot);
        assert!(
            snapshot.warning.is_none(),
            "successful reconnect clears stale status"
        );

        warnings.connections(vec!["MIDI connection failed.".into()]);
        warnings.publish(&mut snapshot);
        snapshot.warning = Some("The original session was preserved.".into());
        warnings.publish(&mut snapshot);
        assert_eq!(
            snapshot.warning.as_deref(),
            Some("The original session was preserved.")
        );
        warnings.connections(Vec::new());
        warnings.publish(&mut snapshot);
        assert_eq!(
            snapshot.warning.as_deref(),
            Some("The original session was preserved.")
        );
        snapshot.warning = None;
        warnings.publish(&mut snapshot);
        assert!(
            snapshot.warning.is_none(),
            "no stale MIDI warning returns after unrelated recovery"
        );
    }

    #[test]
    fn successful_zero_reports_the_previous_count_once_without_retaining_ghost_ports() {
        let mut snapshot = running_inventory_snapshot();
        snapshot.devices.midi_inputs.push("Second input".into());
        let connected = snapshot.devices.clone();
        let mut empty = connected.clone();
        empty.midi_inputs.clear();
        let mut error = None;
        let mut warnings = MidiWarnings::default();
        assert!(publish_inventory(
            &mut snapshot,
            &mut error,
            &mut warnings,
            Enumeration {
                devices: empty.clone(),
                midi_error: None,
            }
        ));
        assert!(snapshot.devices.midi_inputs.is_empty());
        assert!(
            snapshot.error.is_none(),
            "successful zero is not an OS query error"
        );
        let notice = snapshot
            .warning
            .clone()
            .expect("empty inventory diagnostic");
        assert!(notice.contains("previously 2") && notice.contains("Refresh"));
        for _ in 0..5 {
            assert!(!publish_inventory(
                &mut snapshot,
                &mut error,
                &mut warnings,
                Enumeration {
                    devices: empty.clone(),
                    midi_error: None,
                }
            ));
            assert_eq!(snapshot.warning.as_deref(), Some(notice.as_str()));
            assert!(snapshot.devices.midi_inputs.is_empty());
        }
        publish_inventory(
            &mut snapshot,
            &mut error,
            &mut warnings,
            Enumeration {
                devices: connected,
                midi_error: None,
            },
        );
        assert_eq!(snapshot.devices.midi_inputs.len(), 2);
        assert!(
            snapshot.warning.is_none(),
            "returned inputs clear the owned zero notice"
        );
        assert_eq!(snapshot.phase, Phase::Running);
    }

    #[test]
    fn midi_discovery_errors_preserve_ports_and_running_audio_until_a_valid_refresh() {
        let mut snapshot = running_inventory_snapshot();
        snapshot.warning = Some("Original session warning".into());
        let config = snapshot.config.clone();
        let mut discovery_error = None;
        let mut midi_warnings = MidiWarnings::default();
        let mut discovered = crate::devices::tests::inventory(8);
        discovered.midi_inputs.clear();
        assert!(publish_inventory(
            &mut snapshot,
            &mut discovery_error,
            &mut midi_warnings,
            Enumeration {
                devices: discovered.clone(),
                midi_error: Some("Could not read MIDI input 2: name query failed".into()),
            }
        ));
        assert_eq!(snapshot.devices.midi_inputs, ["Keys"]);
        assert_eq!(
            snapshot.devices.audio, discovered.audio,
            "audio still refreshes"
        );
        assert_eq!(snapshot.phase, Phase::Running);
        assert_eq!(
            (snapshot.actual_sample_rate, snapshot.actual_buffer_frames),
            (48000, 256)
        );
        assert_eq!(snapshot.config, config);
        assert_eq!(snapshot.error, discovery_error);
        assert!(
            snapshot
                .error
                .as_ref()
                .expect("visible error")
                .contains("Refresh retries")
        );

        snapshot.error = None;
        ensure_midi_error(&mut snapshot, &discovery_error);
        assert_eq!(
            snapshot.error, discovery_error,
            "audio lifecycle cannot hide the MIDI error"
        );
        snapshot.error = Some("Audio apply failed".into());
        ensure_midi_error(&mut snapshot, &discovery_error);
        assert_eq!(snapshot.error.as_deref(), Some("Audio apply failed"));
        assert!(publish_inventory(
            &mut snapshot,
            &mut discovery_error,
            &mut midi_warnings,
            Enumeration {
                devices: discovered.clone(),
                midi_error: None,
            }
        ));
        assert!(
            snapshot.devices.midi_inputs.is_empty(),
            "valid zero is not a ghost inventory"
        );
        assert!(discovery_error.is_none());
        assert_eq!(snapshot.error.as_deref(), Some("Audio apply failed"));
        assert_eq!(
            snapshot.warning.as_deref(),
            Some("Original session warning")
        );

        snapshot.error = None;
        publish_inventory(
            &mut snapshot,
            &mut discovery_error,
            &mut midi_warnings,
            Enumeration {
                devices: discovered.clone(),
                midi_error: Some("Initialization failed".into()),
            },
        );
        assert!(snapshot.error.is_some());
        publish_inventory(
            &mut snapshot,
            &mut discovery_error,
            &mut midi_warnings,
            Enumeration {
                devices: discovered,
                midi_error: None,
            },
        );
        assert!(
            snapshot.error.is_none(),
            "recovery clears only its own error"
        );
        assert_eq!(snapshot.phase, Phase::Running);
    }

    #[test]
    fn reconnect_with_unchanged_device_identity_retries_but_user_stop_is_respected() {
        assert!(should_retry_after_poll(false, false, false));
        assert!(!should_retry_after_poll(true, false, false));
        assert!(!should_retry_after_poll(false, true, false));
        assert!(!should_retry_after_poll(false, false, true));
    }
    struct Fake;
    impl AudioProcessor for Fake {
        fn prepare(&mut self, _: ProcessConfig) -> Result<(), String> {
            Ok(())
        }
        fn process(&mut self, b: AudioBlock<'_>) {
            b.left.fill(1.0);
            b.right.fill(3.0);
        }
        fn deactivate(&mut self) {}
    }
    #[test]
    fn failed_candidate_rolls_back_without_saving_it() {
        let devices = crate::devices::tests::inventory(2);
        let previous = Config::initial();
        let mut candidate = previous.clone();
        candidate.sample_rate = 96000;
        let candidate = negotiate(&candidate, &devices).expect("candidate");
        let result = transition(
            Box::new(Fake),
            &candidate,
            Some(&previous),
            &devices,
            |processor, config| {
                if config.sample_rate == 96000 {
                    Err(("Device busy".into(), Some(processor)))
                } else {
                    Ok(processor)
                }
            },
        );
        assert!(result.stream.is_some());
        assert!(!result.accepted);
        assert_eq!(result.config.expect("rollback").sample_rate, 48000);
        assert!(result.error.expect("error").contains("Device busy"));
    }
    #[test]
    fn stream_format_selection_respects_the_negotiated_buffer_range() {
        let range = |format, min, max| {
            cpal::SupportedStreamConfigRange::new(
                2,
                cpal::SampleRate(48000),
                cpal::SampleRate(48000),
                cpal::SupportedBufferSize::Range { min, max },
                format,
            )
        };
        assert_eq!(
            choose_format(
                vec![
                    range(cpal::SampleFormat::F32, 32, 64),
                    range(cpal::SampleFormat::I16, 128, 512)
                ],
                2,
                48000,
                256
            )
            .expect("supported range"),
            cpal::SampleFormat::I16
        );
        assert!(
            choose_format(vec![range(cpal::SampleFormat::F32, 32, 64)], 2, 48000, 256).is_err()
        );
    }
    #[test]
    fn unavailable_rollback_retains_processor_for_later_recovery() {
        let devices = crate::devices::tests::inventory(2);
        let candidate = negotiate(&Config::initial(), &devices).expect("candidate");
        let mut previous = candidate.config.clone();
        let mut absent = candidate.output.clone();
        absent.name = "Unplugged".into();
        previous.output = Some(absent);
        let result: Transition<Box<dyn AudioProcessor>> = transition(
            Box::new(Fake),
            &candidate,
            Some(&previous),
            &devices,
            |processor, _| Err(("Open failed".into(), Some(processor))),
        );
        assert!(result.stream.is_none());
        assert!(result.processor.is_some());
        assert!(!result.accepted);
        let recovered = transition(
            result.processor.expect("retained"),
            &candidate,
            None,
            &devices,
            |processor, _| Ok(processor),
        );
        assert!(recovered.accepted);
        assert!(recovered.stream.is_some());
    }
    #[test]
    fn denied_input_keeps_output_available_and_reports_actual_configuration() {
        let devices = crate::devices::tests::inventory(2);
        let mut config = Config::initial();
        config.input = devices.default_output.clone();
        let candidate = negotiate(&config, &devices).expect("duplex");
        let result = transition(
            Box::new(Fake),
            &candidate,
            None,
            &devices,
            |processor, config| {
                if config.input.is_some() {
                    Err((
                        "External input could not start: permission denied".into(),
                        Some(processor),
                    ))
                } else {
                    Ok(processor)
                }
            },
        );
        assert!(result.accepted);
        assert!(result.stream.is_some());
        assert!(result.config.expect("actual").input.is_none());
        assert!(
            result
                .warning
                .expect("warning")
                .contains("permission denied")
        );
    }
    #[test]
    fn variable_callbacks_are_split_without_reallocation_and_processor_is_returned_on_drop() {
        let (tx, rx) = bounded(1);
        let metrics = Arc::new(Metrics::default());
        let mut callback = Callback {
            owner: ProcessorOwner {
                processor: Some(Box::new(Fake)),
                returned: tx,
            },
            midi: Arc::new(MidiQueue::new()),
            router: MidiRouter::new(None),
            input: InputResampler::new(Arc::new(AtomicQueue::new(8)), false, 3),
            metrics: Arc::clone(&metrics),
            left: vec![0.0; 3],
            right: vec![0.0; 3],
            input_left: vec![0.0; 3],
            input_right: vec![0.0; 3],
            events: [MidiEvent {
                source: 0,
                data: [0; 3],
                len: 0,
            }; MIDI_PER_BLOCK],
            pending_releases: 0,
            channels: 4,
            selection: [2, 3],
        };
        let left_ptr = callback.left.as_ptr();
        for frames in [1, 3, 4, 10, 257] {
            let mut output = vec![99.0; frames * 4];
            for note in 0..128 {
                assert!(callback.midi.push(MidiEvent {
                    source: 1,
                    data: [0x90, note, 100],
                    len: 3
                }));
            }
            assert_eq!(
                crate::allocations::count(|| callback.render(&mut output)),
                0
            );
            for frame in output.chunks(4) {
                assert_eq!(frame, [0.0, 0.0, 1.0, 3.0]);
            }
            assert_eq!(callback.left.as_ptr(), left_ptr);
        }
        drop(callback);
        assert!(rx.try_recv().is_ok());
    }
    #[test]
    fn tracked_applications_report_repeated_failures_for_their_own_request() {
        let dir = tempfile::tempdir().expect("temp");
        let mut runtime = spawn(
            Box::new(Fake),
            None,
            AppOptions {
                app_id: "TrackedApplyTest".into(),
                state_dir: dir.path().into(),
                device_access: false,
            },
        )
        .expect("spawn");
        runtime.wait_ready(Duration::from_secs(2)).expect("ready");
        let handle = runtime.handle();
        let first = handle
            .apply_tracked(Config::initial())
            .expect("first queued");
        let second = handle
            .apply_tracked(Config::initial())
            .expect("second queued");
        let first = first
            .result
            .recv_timeout(Duration::from_secs(2))
            .expect("first completion");
        let second = second
            .result
            .recv_timeout(Duration::from_secs(2))
            .expect("second completion");
        assert!(first.is_err());
        assert_eq!(
            first, second,
            "identical errors still complete both requests"
        );
        drop(
            handle
                .apply_tracked(Config::initial())
                .expect("dismissed request"),
        );
        runtime
            .shutdown()
            .expect("dropped receipt cannot block shutdown");
    }

    #[test]
    fn no_device_runtime_keeps_handle_and_shutdown_works_without_callbacks() {
        let dir = tempfile::tempdir().expect("temp");
        let mut runtime = spawn(
            Box::new(Fake),
            None,
            AppOptions {
                app_id: "Test".into(),
                state_dir: dir.path().into(),
                device_access: false,
            },
        )
        .expect("spawn");
        let handle = runtime.handle();
        runtime.wait_ready(Duration::from_secs(1)).expect("ready");
        runtime
            .wait_ready(Duration::from_secs(1))
            .expect("already ready");
        AppHandle::install_current(&handle);
        assert!(AppHandle::current().is_some());
        assert_eq!(handle.snapshot().phase, Phase::Stopped);
        handle.refresh().expect("refresh");
        runtime.shutdown().expect("shutdown");
        assert_eq!(handle.snapshot().phase, Phase::Stopped);
        assert!(dir.path().join("session.json").exists());
    }
    #[test]
    fn unreadable_session_is_preserved_and_processor_error_is_actionable() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"bad session").expect("bad session");
        let mut runtime = spawn(
            Box::new(Fake),
            None,
            AppOptions {
                app_id: "Test".into(),
                state_dir: dir.path().into(),
                device_access: false,
            },
        )
        .expect("spawn");
        let handle = runtime.handle();
        handle
            .stop("Processor failed; select another preset and Apply.".into())
            .expect("stop");
        let deadline = Instant::now() + Duration::from_secs(1);
        while handle.snapshot().phase != Phase::Error && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            handle.snapshot().error.as_deref(),
            Some("Processor failed; select another preset and Apply.")
        );
        assert!(
            runtime.shutdown().is_err(),
            "preserving the original prevented final session save"
        );
        assert_eq!(std::fs::read(&path).expect("preserved"), b"bad session");
        assert!(
            handle
                .snapshot()
                .warning
                .expect("warning")
                .contains("preserved")
        );
    }
    #[test]
    fn moving_bad_session_aside_resumes_saves_and_same_directory_instances_are_refused() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"bad original").expect("bad");
        let options = AppOptions {
            app_id: "Test".into(),
            state_dir: dir.path().into(),
            device_access: false,
        };
        let mut runtime = spawn(Box::new(Fake), None, options.clone()).expect("first");
        runtime.wait_ready(Duration::from_secs(1)).expect("ready");
        assert!(
            spawn(Box::new(Fake), None, options.clone()).is_err(),
            "second instance cannot clobber this session"
        );
        std::fs::rename(&path, dir.path().join("session.bad.json")).expect("move aside");
        for _ in 0..1024 {
            runtime.handle().save().expect("coalesced save");
        }
        runtime.shutdown().expect("close saves now resume");
        assert!(
            runtime.handle().snapshot().warning.is_none(),
            "stale preservation warning is cleared"
        );
        assert!(
            load_session(&path, "Test")
                .expect("valid saved session")
                .is_some()
        );
        assert_eq!(
            std::fs::read(dir.path().join("session.bad.json")).expect("preserved"),
            b"bad original"
        );
        let mut next = spawn(Box::new(Fake), None, options).expect("lock released");
        next.shutdown().expect("next shutdown");
    }
    #[test]
    fn final_save_errors_are_returned_and_replaced_bad_files_are_never_overwritten() {
        struct BadSave;
        impl SessionState for BadSave {
            fn save(&self) -> Result<Vec<u8>, String> {
                Err("save denied by test".into())
            }
            fn restore(&self, _: &[u8]) -> Result<(), String> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().expect("temp");
        let options = AppOptions {
            app_id: "Test".into(),
            state_dir: dir.path().into(),
            device_access: false,
        };
        let mut runtime =
            spawn(Box::new(Fake), Some(Arc::new(BadSave)), options.clone()).expect("spawn");
        assert!(
            runtime
                .shutdown()
                .expect_err("save failure returned")
                .contains("save denied by test")
        );
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"original bad").expect("bad original");
        let mut runtime = spawn(Box::new(Fake), None, options).expect("spawn");
        runtime.wait_ready(Duration::from_secs(1)).expect("ready");
        std::fs::write(&path, b"different bad").expect("changed file");
        assert!(
            runtime
                .shutdown()
                .expect_err("new file preserved")
                .contains("different file")
        );
        assert_eq!(std::fs::read(&path).expect("preserved"), b"different bad");
    }

    #[test]
    fn fresh_midi_default_waits_for_a_unique_input_and_keeps_its_endpoint() {
        let mut config = Config::initial();
        let mut devices = crate::devices::tests::inventory(2);
        devices.midi_inputs.clear();
        assert!(!resolve_default_midi(&mut config, &devices));
        assert!(config.auto_midi && config.midi_inputs.is_empty());

        devices.midi_inputs = vec![
            "Duplicate".into(),
            "First unique".into(),
            "Duplicate".into(),
            "Second unique".into(),
        ];
        assert!(resolve_default_midi(&mut config, &devices));
        assert_eq!(config.midi_inputs, ["First unique"]);
        assert!(config.auto_midi);
        devices.midi_inputs.reverse();
        assert!(!resolve_default_midi(&mut config, &devices));
        assert_eq!(config.midi_inputs, ["First unique"]);
        devices.midi_inputs = vec!["Different input".into()];
        assert!(!resolve_default_midi(&mut config, &devices));
        assert_eq!(
            config.midi_inputs,
            ["First unique"],
            "unplug cannot switch sources"
        );
    }

    #[test]
    fn explicit_midi_selection_or_disable_does_not_resolve_a_default() {
        let devices = crate::devices::tests::inventory(2);
        let mut disabled = Config::initial();
        disabled.auto_midi = false;
        assert!(!resolve_default_midi(&mut disabled, &devices));
        assert!(disabled.midi_inputs.is_empty());
        disabled.midi_inputs = vec!["Requested endpoint".into()];
        assert!(!resolve_default_midi(&mut disabled, &devices));
        assert_eq!(disabled.midi_inputs, ["Requested endpoint"]);

        let mut automatic = Config::initial();
        let mut ambiguous = devices;
        ambiguous.midi_inputs = vec![String::new(), "Same".into(), "Same".into()];
        assert!(!resolve_default_midi(&mut automatic, &ambiguous));
        assert!(automatic.midi_inputs.is_empty());
    }
}
