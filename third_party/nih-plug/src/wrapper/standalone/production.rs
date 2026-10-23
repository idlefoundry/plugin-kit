//! The production standalone adapter. Unlike the development launcher, the
//! processor has one exclusive owner and never waits for the GUI in a callback.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use baseview::{Event, EventStatus, Window, WindowHandler};
use crossbeam::queue::ArrayQueue;
use plugin_kit_standalone::{
    AppHandle, AppOptions, AudioBlock, AudioProcessor, MidiResetScope, ProcessConfig, SessionState,
};
use raw_window_handle::HasRawWindowHandle;

use crate::prelude::*;
use crate::wrapper::state::{self, ParamValue, PluginState};
use crate::wrapper::util::{process_wrapper, setup_logger};

const TASK_CAPACITY: usize = 256;

/// Single consumers for the audio thread's GUI and background task queues.
pub type AudioTaskReceivers<P> = (
    rtrb::Consumer<<P as Plugin>::BackgroundTask>,
    rtrb::Consumer<<P as Plugin>::BackgroundTask>,
);

struct Parameter {
    id: String,
    ptr: ParamPtr,
    dirty: AtomicBool,
    ui_dirty: AtomicBool,
}

/// Shared NIH parameter and persisted UI state. This never contains the DSP.
pub struct Session<P: Plugin> {
    params: Arc<dyn Params>,
    parameters: Vec<Parameter>,
    by_ptr: HashMap<ParamPtr, usize>,
    by_id: HashMap<String, ParamPtr>,
    transient_defaults: HashMap<String, ParamValue>,
    tasks: ArrayQueue<P::BackgroundTask>,
    background_tasks: ArrayQueue<P::BackgroundTask>,
    sample_rate: AtomicU32,
    latency: AtomicU32,
    error: AtomicBool,
    resize: AtomicBool,
    state_changed: AtomicBool,
}

impl<P: Plugin> Session<P> {
    fn new(params: Arc<dyn Params>, transient_ids: &[&str]) -> Arc<Self> {
        let map = params.param_map();
        let by_id: HashMap<_, _> = map.iter().map(|(id, ptr, _)| (id.clone(), *ptr)).collect();
        // SAFETY: every pointer came from params, whose Arc is retained here.
        let defaults = unsafe {
            state::serialize_object::<P>(params.clone(), by_id.iter().map(|(id, ptr)| (id, *ptr)))
        };
        Arc::new(Self {
            params,
            by_ptr: map
                .iter()
                .enumerate()
                .map(|(i, (_, ptr, _))| (*ptr, i))
                .collect(),
            by_id,
            parameters: map
                .into_iter()
                .map(|(id, ptr, _)| Parameter {
                    id,
                    ptr,
                    dirty: AtomicBool::new(false),
                    ui_dirty: AtomicBool::new(false),
                })
                .collect(),
            transient_defaults: transient_ids
                .iter()
                .filter_map(|id| {
                    defaults
                        .params
                        .get(*id)
                        .map(|value| ((*id).to_owned(), value.clone()))
                })
                .collect(),
            tasks: ArrayQueue::new(TASK_CAPACITY),
            background_tasks: ArrayQueue::new(TASK_CAPACITY),
            sample_rate: AtomicU32::new(48_000.0f32.to_bits()),
            latency: AtomicU32::new(0),
            error: AtomicBool::new(false),
            resize: AtomicBool::new(false),
            state_changed: AtomicBool::new(false),
        })
    }

    fn state(&self) -> PluginState {
        // SAFETY: params owns all of these pointers for this Session's lifetime.
        unsafe {
            state::serialize_object::<P>(
                self.params.clone(),
                self.by_id.iter().map(|(id, ptr)| (id, *ptr)),
            )
        }
    }

    pub fn set_parameter(&self, ptr: ParamPtr, normalized: f32) {
        let Some(&index) = self.by_ptr.get(&ptr) else {
            return;
        };
        if !normalized.is_finite() {
            return;
        }
        // The values are atomic. Updating the smoother is exclusively the
        // processor's responsibility, so editor changes also work while stopped.
        // SAFETY: the registered pointer's owner is retained in self.params.
        unsafe {
            ptr.set_normalized_value(normalized.clamp(0.0, 1.0));
        }
        self.parameters[index].dirty.store(true, Ordering::Release);
        self.parameters[index]
            .ui_dirty
            .store(true, Ordering::Release);
    }

    fn smoothers(&self, rate: f32, reset: bool) {
        for parameter in &self.parameters {
            if reset || parameter.dirty.swap(false, Ordering::AcqRel) {
                // SAFETY: pointer retained by params; only processor updates smoothers.
                unsafe {
                    parameter.ptr.update_smoother(rate, reset);
                }
            }
        }
    }

    fn task(&self, task: P::BackgroundTask) {
        let _ = self.tasks.push(task);
    }

    fn background(&self, task: P::BackgroundTask) {
        let _ = self.background_tasks.push(task);
    }

    /// A processing error is retained without formatting or logging in the callback.
    pub fn processing_failed(&self) -> bool {
        self.error.load(Ordering::Relaxed)
    }
}

impl<P: Plugin> SessionState for Session<P> {
    fn save(&self) -> Result<Vec<u8>, String> {
        let mut state = self.state();
        for (id, value) in &self.transient_defaults {
            state.params.insert(id.clone(), value.clone());
        }
        serde_json::to_vec(&state).map_err(|error| error.to_string())
    }

    fn restore(&self, bytes: &[u8]) -> Result<(), String> {
        let mut restored: PluginState = serde_json::from_slice(bytes)
            .map_err(|error| format!("The instrument session is invalid: {error}"))?;
        for (id, value) in &restored.fields {
            serde_json::from_str::<serde_json::Value>(value).map_err(|_| {
                format!("The instrument session contains an invalid saved field '{id}'.")
            })?;
        }
        // Reject invalid values before changing any shared parameters. Unknown
        // parameters from a newer compatible plugin version may be ignored.
        // Validate a migrated copy, then let deserialize_object apply the
        // instrument's compatibility migration exactly once to the original.
        let mut validated = restored.clone();
        P::filter_state(&mut validated);
        self.params
            .validate_fields(&validated.fields)
            .map_err(|error| {
                format!("The instrument session contains an invalid saved field: {error}")
            })?;
        for (id, value) in &validated.params {
            if !self.by_id.contains_key(id) {
                continue;
            }
            let ptr = self.by_id[id];
            let valid = match (ptr, value) {
                (ParamPtr::FloatParam(_), ParamValue::F32(value)) => value.is_finite(),
                (ParamPtr::IntParam(_), ParamValue::I32(_))
                | (ParamPtr::BoolParam(_), ParamValue::Bool(_)) => true,
                (ParamPtr::EnumParam(ptr), ParamValue::I32(index)) => {
                    // SAFETY: ptr is registered in params, retained by self.
                    *index >= 0 && (*index as usize) < unsafe { (*ptr).len() }
                }
                (ParamPtr::EnumParam(ptr), ParamValue::String(id)) => {
                    // SAFETY: as above, immutable enum metadata.
                    unsafe { (*ptr).has_id(id) }
                }
                _ => false,
            };
            if !valid {
                return Err(format!(
                    "The instrument session contains an invalid value for '{id}'."
                ));
            }
        }
        restored.params.retain(|id, _| self.by_id.contains_key(id));
        for (id, value) in &self.transient_defaults {
            restored.params.insert(id.clone(), value.clone());
        }
        // SAFETY: SessionState::restore runs after the old stream has been destroyed,
        // all pointers belong to the retained params Arc. prepare resets smoothers.
        let success = unsafe {
            state::deserialize_object::<P>(
                &mut restored,
                self.params.clone(),
                |id| self.by_id.get(id).copied(),
                None,
            )
        };
        if !success {
            return Err("The instrument session could not be restored.".into());
        }
        self.state_changed.store(true, Ordering::Release);
        Ok(())
    }
}

/// Adapter that owns the exact Plugin instance used by the editor and session.
pub struct Processor<P: Plugin> {
    plugin: P,
    midi_reset_scope: MidiResetScope,
    session: Arc<Session<P>>,
    layout: AudioIOLayout,
    main: Buffer<'static>,
    side: Buffer<'static>,
    input_left: Vec<f32>,
    input_right: Vec<f32>,
    rate: f32,
    initialized: bool,
    panic_token: Option<u64>,
    init_executor: Arc<Mutex<TaskExecutor<P>>>,
    // Each callback-owned producer has exactly one consumer on the GUI or
    // background thread. There is no CAS retry loop in these callback queues.
    gui_tasks: RefCell<rtrb::Producer<P::BackgroundTask>>,
    background_tasks: RefCell<rtrb::Producer<P::BackgroundTask>>,
    task_receivers: Option<AudioTaskReceivers<P>>,
}

impl<P: Plugin> Processor<P>
where
    P::BackgroundTask: Copy,
{
    pub fn new(mut plugin: P, transient_ids: &[&str]) -> Result<(Self, Arc<Session<P>>), String> {
        let layout = *P::AUDIO_IO_LAYOUTS
            .first()
            .ok_or("The instrument has no audio layout.")?;
        if layout.main_input_channels.is_some()
            || layout.main_output_channels.map(|n| n.get()).unwrap_or(0) != 2
            || layout.aux_input_ports.len() > 1
            || layout.aux_input_ports.first().is_some_and(|n| n.get() > 2)
            || !layout.aux_output_ports.is_empty()
        {
            return Err("This standalone adapter requires stereo output and at most one stereo auxiliary input.".into());
        }
        let session = Session::new(plugin.params(), transient_ids);
        let init_executor = Arc::new(Mutex::new(plugin.task_executor()));
        let mut main = Buffer::default();
        let mut side = Buffer::default();
        let (gui_tasks, gui_receiver) = rtrb::RingBuffer::new(TASK_CAPACITY);
        let (background_tasks, background_receiver) = rtrb::RingBuffer::new(TASK_CAPACITY);
        // Reserve only while no stream exists. Callback bindings retain capacity.
        unsafe {
            main.set_slices(0, |s| s.reserve(2));
            side.set_slices(0, |s| s.reserve(2));
        }
        Ok((
            Self {
                plugin,
                midi_reset_scope: MidiResetScope::Channel,
                session: session.clone(),
                layout,
                main,
                side,
                input_left: Vec::new(),
                input_right: Vec::new(),
                rate: 48_000.0,
                initialized: false,
                panic_token: None,
                init_executor,
                gui_tasks: RefCell::new(gui_tasks),
                background_tasks: RefCell::new(background_tasks),
                task_receivers: Some((gui_receiver, background_receiver)),
            },
            session,
        ))
    }

    /// Preserve an instrument's deliberately global controller-reset behavior.
    pub fn with_midi_reset_scope(mut self, scope: MidiResetScope) -> Self {
        self.midi_reset_scope = scope;
        self
    }

    /// Transfer the audio task consumers to the host before moving this processor
    /// into a stream. The first consumer belongs to the GUI thread, the second
    /// to its background task worker. Consumers must not be duplicated.
    pub fn take_audio_task_receivers(&mut self) -> Option<AudioTaskReceivers<P>> {
        self.task_receivers.take()
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        self.plugin.editor(AsyncExecutor {
            execute_background: Arc::new({
                let session = self.session.clone();
                move |task| session.background(task)
            }),
            execute_gui: Arc::new({
                let session = self.session.clone();
                move |task| session.task(task)
            }),
        })
    }
}

// Buffer's reference vector is empty at each boundary. Rebinding only an empty
// vector makes its capacity reusable without retaining any callback references.
fn rebind_empty<'a, 'b>(buffer: Buffer<'a>) -> Buffer<'b> {
    debug_assert_eq!(buffer.channels(), 0);
    // SAFETY: there are no references in the vector and Buffer stores no other
    // lifetime-dependent data. All live references are cleared before rebinding.
    unsafe { std::mem::transmute(buffer) }
}

impl<P: Plugin> AudioProcessor for Processor<P>
where
    P::BackgroundTask: Copy,
{
    fn midi_reset_scope(&self) -> MidiResetScope {
        self.midi_reset_scope
    }

    fn prepare(&mut self, config: ProcessConfig) -> Result<(), String> {
        if !config.sample_rate.is_finite()
            || config.sample_rate <= 0.0
            || config.max_buffer_frames == 0
            || config.max_buffer_frames > u32::MAX as usize
        {
            return Err("The audio processing configuration is invalid.".into());
        }
        self.rate = config.sample_rate;
        self.input_left.resize(config.max_buffer_frames, 0.0);
        self.input_right.resize(config.max_buffer_frames, 0.0);
        self.session
            .sample_rate
            .store(self.rate.to_bits(), Ordering::Relaxed);
        self.session.smoothers(self.rate, true);
        let buffer_config = BufferConfig {
            sample_rate: self.rate,
            min_buffer_size: None,
            max_buffer_size: config.max_buffer_frames as u32,
            process_mode: ProcessMode::Realtime,
        };
        if !self.plugin.initialize(
            &self.layout,
            &buffer_config,
            &mut Initializing {
                session: &self.session,
                executor: &self.init_executor,
            },
        ) {
            return Err("The instrument rejected the audio configuration.".into());
        }
        self.plugin.reset();
        self.initialized = true;
        self.session.error.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn panic_token(&mut self, token: Option<u64>) {
        self.panic_token = token;
    }

    fn process(&mut self, block: AudioBlock<'_>) {
        // Consume even on an invalid block, and never retain metadata across direct calls.
        let panic_token = self.panic_token.take();
        process_wrapper(|| {
            let frames = block.left.len();
            if !self.initialized || block.right.len() != frames || frames > self.input_left.len() {
                block.left.fill(0.0);
                block.right.fill(0.0);
                return;
            }
            self.session.smoothers(self.rate, false);
            for (to, from) in self.input_left[..frames].iter_mut().zip(
                block
                    .input_left
                    .iter()
                    .copied()
                    .chain(std::iter::repeat(0.0)),
            ) {
                *to = if from.is_finite() { from } else { 0.0 };
            }
            for (to, from) in self.input_right[..frames].iter_mut().zip(
                block
                    .input_right
                    .iter()
                    .copied()
                    .chain(std::iter::repeat(0.0)),
            ) {
                *to = if from.is_finite() { from } else { 0.0 };
            }
            let mut main = rebind_empty(std::mem::take(&mut self.main));
            let mut side = rebind_empty(std::mem::take(&mut self.side));
            // SAFETY: each slice is exactly frames long, distinct, and alive until
            // process returns. Both vectors were reserved off the callback.
            unsafe {
                main.set_slices(frames, |s| {
                    s.push(block.left);
                    s.push(block.right);
                });
                side.set_slices(frames, |s| {
                    s.push(&mut self.input_left[..frames]);
                    if self
                        .layout
                        .aux_input_ports
                        .first()
                        .is_some_and(|n| n.get() == 2)
                    {
                        s.push(&mut self.input_right[..frames]);
                    }
                });
            }
            let mut sides = [side];
            let side_count = usize::from(!self.layout.aux_input_ports.is_empty());
            // SAFETY: NIH's AuxiliaryBuffers ties the lifetime of its buffer
            // array to its channel references. Project the unique local array
            // for this call only; nobody uses sides until process has returned,
            // and the plugin API cannot retain the borrowed buffer array.
            let inputs = unsafe { std::slice::from_raw_parts_mut(sides.as_mut_ptr(), side_count) };
            let mut context = Processing {
                session: &self.session,
                midi: block.midi.iter(),
                panic_channel: block.panic.then_some(0),
                panic_token: block.panic.then_some(panic_token).flatten(),
                transport: Transport::new(self.rate),
                gui_tasks: &self.gui_tasks,
                background_tasks: &self.background_tasks,
            };
            if matches!(
                self.plugin.process(
                    &mut main,
                    &mut AuxiliaryBuffers {
                        inputs,
                        outputs: &mut []
                    },
                    &mut context
                ),
                ProcessStatus::Error(_)
            ) {
                self.session.error.store(true, Ordering::Relaxed);
                for output in main.as_slice() {
                    output.fill(0.0);
                }
            }
            // SAFETY: clearing drops references only; the callback buffers are
            // still alive, and no processor retains any pointer to their data.
            unsafe {
                main.set_slices(0, Vec::clear);
                sides[0].set_slices(0, Vec::clear);
            }
            self.main = rebind_empty(main);
            self.side = rebind_empty(std::mem::take(&mut sides[0]));
        });
    }

    fn deactivate(&mut self) {
        if self.initialized {
            self.plugin.reset();
            self.plugin.deactivate();
            self.initialized = false;
        }
    }
}

struct Initializing<'a, P: Plugin> {
    session: &'a Session<P>,
    executor: &'a Mutex<TaskExecutor<P>>,
}
impl<P: Plugin> InitContext<P> for Initializing<'_, P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }
    fn execute(&self, task: P::BackgroundTask) {
        (self
            .executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))(task);
    }
    fn set_latency_samples(&self, samples: u32) {
        self.session.latency.store(samples, Ordering::Relaxed);
    }
    fn set_current_voice_capacity(&self, _capacity: u32) {}
}

struct Processing<'a, P: Plugin> {
    session: &'a Session<P>,
    midi: std::slice::Iter<'a, plugin_kit_standalone::MidiEvent>,
    panic_channel: Option<u8>,
    panic_token: Option<u64>,
    transport: Transport,
    gui_tasks: &'a RefCell<rtrb::Producer<P::BackgroundTask>>,
    background_tasks: &'a RefCell<rtrb::Producer<P::BackgroundTask>>,
}
impl<P: Plugin> ProcessContext<P> for Processing<'_, P>
where
    P::BackgroundTask: Copy,
{
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }
    fn take_standalone_panic(&mut self) -> bool {
        self.panic_channel.take().is_some()
    }
    fn standalone_panic_token(&self) -> Option<u64> {
        self.panic_token
    }
    fn execute_background(&self, task: P::BackgroundTask) {
        if let Ok(mut producer) = self.background_tasks.try_borrow_mut() {
            let _ = producer.push(task);
        }
    }
    fn execute_gui(&self, task: P::BackgroundTask) {
        if let Ok(mut producer) = self.gui_tasks.try_borrow_mut() {
            let _ = producer.push(task);
        }
    }
    fn transport(&self) -> &Transport {
        &self.transport
    }
    fn process_mode(&self) -> ProcessMode {
        ProcessMode::Realtime
    }
    fn next_event(&mut self) -> Option<PluginNoteEvent<P>> {
        if let Some(channel) = self.panic_channel {
            self.panic_channel = (channel < 31).then_some(channel + 1);
            return Some(NoteEvent::MidiCC {
                timing: 0,
                channel: channel / 2,
                cc: if channel % 2 == 0 { 120 } else { 121 },
                value: 0.0,
            });
        }
        self.midi.find_map(|event| {
            NoteEvent::from_midi(0, &event.data[..usize::from(event.len).min(3)]).ok()
        })
    }
    fn send_event(&mut self, _event: PluginNoteEvent<P>) {}
    fn set_latency_samples(&self, samples: u32) {
        self.session.latency.store(samples, Ordering::Relaxed);
    }
    fn set_current_voice_capacity(&self, _capacity: u32) {}
    fn set_parameter_normalized(&mut self, ptr: ParamPtr, normalized: f32, _timing: u32) -> bool {
        self.session.set_parameter(ptr, normalized);
        if self.session.by_ptr.contains_key(&ptr) {
            // SAFETY: known parameter; this call is on the processor thread.
            unsafe {
                ptr.update_smoother(self.transport.sample_rate, false);
            }
        }
        false
    }
}

struct Context<P: Plugin> {
    session: Arc<Session<P>>,
    app: Arc<AppHandle>,
}
impl<P: Plugin> GuiContext for Context<P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }
    fn request_resize(&self) -> bool {
        self.session.resize.store(true, Ordering::Release);
        true
    }
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        self.session.set_parameter(param, normalized);
    }
    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        self.session.state()
    }
    fn set_state(&self, state: PluginState) {
        if let Ok(bytes) = serde_json::to_vec(&state) {
            let _ = self.app.restore_state(bytes);
        }
    }
}

struct AppWindow<P: Plugin> {
    _editor_handle: Box<dyn Any>,
    editor: Arc<Mutex<Box<dyn Editor>>>,
    session: Arc<Session<P>>,
    executor: Arc<Mutex<TaskExecutor<P>>>,
    app: Arc<AppHandle>,
    audio_tasks: rtrb::Consumer<P::BackgroundTask>,
}
impl<P: Plugin> WindowHandler for AppWindow<P> {
    fn on_frame(&mut self, window: &mut Window) {
        // Quitting, as the editor asked (`app_window::request_quit`): the window closed as its
        // close button closes it, and the app shut down as then, once `open_blocking` returns.
        if app_window::quit_requested() {
            window.close();
            return;
        }
        if self.session.error.swap(false, Ordering::AcqRel) {
            let _ = self.app.stop(
                "The instrument could not process audio. Apply audio settings to restart it."
                    .into(),
            );
        }
        for parameter in &self.session.parameters {
            if parameter.ui_dirty.swap(false, Ordering::AcqRel) {
                // SAFETY: pointer belongs to the retained params Arc.
                let value = unsafe { parameter.ptr.unmodulated_normalized_value() };
                self.editor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .param_value_changed(&parameter.id, value);
            }
        }
        for _ in 0..TASK_CAPACITY {
            match self.audio_tasks.pop() {
                Ok(task) => (self
                    .executor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner))(
                    task
                ),
                Err(_) => break,
            }
        }
        for _ in 0..TASK_CAPACITY {
            match self.session.tasks.pop() {
                Some(task) => (self
                    .executor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner))(
                    task
                ),
                None => break,
            }
        }
        if self.session.resize.swap(false, Ordering::AcqRel) {
            let size = self
                .editor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .size();
            app_window::fit(window, size);
        }
        if self.session.state_changed.swap(false, Ordering::AcqRel) {
            self.editor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .param_values_changed();
        }
    }
    fn on_event(&mut self, window: &mut Window, event: Event) -> EventStatus {
        if app_window::refits(&event) {
            self.session.resize.store(true, Ordering::Release);
        }
        // (The window's own keys, where the editor's has not the keyboard: on X11, with the
        // pointer outside the editor's.)
        if let Event::Keyboard(key) = &event {
            if app_window::quits(key) {
                window.close();
                return EventStatus::Captured;
            }
        }
        EventStatus::Ignored
    }
}

/// Launch using the instrument's normal editor. Device failure never closes it.
pub fn run<P: Plugin>(options: AppOptions) -> Result<(), String>
where
    P::BackgroundTask: Copy,
{
    run_with_transient_parameters::<P>(options, &[])
}

/// Exclude live performance controls from restored app sessions while retaining
/// the plugin's ordinary state representation and preset semantics.
pub fn run_with_transient_parameters<P: Plugin>(
    options: AppOptions,
    transient_ids: &[&str],
) -> Result<(), String>
where
    P::BackgroundTask: Copy,
{
    run_with_midi_reset_scope::<P>(options, transient_ids, MidiResetScope::Channel)
}

/// Choose the controller reset policy explicitly for omni instruments.
pub fn run_with_midi_reset_scope<P: Plugin>(
    options: AppOptions,
    transient_ids: &[&str],
    midi_reset_scope: MidiResetScope,
) -> Result<(), String>
where
    P::BackgroundTask: Copy,
{
    setup_logger();
    let (mut processor, session) = Processor::new(P::default(), transient_ids)?;
    processor.midi_reset_scope = midi_reset_scope;
    let editor = processor
        .editor()
        .ok_or("The standalone instrument requires an editor.")?;
    let executor = processor.init_executor.clone();
    let background_executor = processor.init_executor.clone();
    let (audio_gui_tasks, mut audio_background_tasks) = processor
        .take_audio_task_receivers()
        .ok_or("Standalone task receivers are unavailable.")?;
    let mut runtime =
        plugin_kit_standalone::spawn(Box::new(processor), Some(session.clone()), options)?;
    // Session restoration precedes the editor's first size/view read. This
    // handshake does not wait for hardware discovery or device opening.
    runtime.wait_ready(Duration::from_secs(2))?;
    let terminate = Arc::new(AtomicBool::new(false));
    let (worker_finished, worker_completion) = crossbeam::channel::bounded(1);
    let worker = {
        let session = session.clone();
        let terminate = terminate.clone();
        thread::Builder::new()
            .name(format!("{} standalone tasks", P::NAME))
            .spawn(move || {
                while !terminate.load(Ordering::Acquire) {
                    for _ in 0..TASK_CAPACITY {
                        match audio_background_tasks.pop() {
                            Ok(task) => (background_executor
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner))(
                                task
                            ),
                            Err(_) => break,
                        }
                    }
                    for _ in 0..TASK_CAPACITY {
                        match session.background_tasks.pop() {
                            Some(task) => (background_executor
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner))(
                                task
                            ),
                            None => break,
                        }
                    }
                    thread::park_timeout(Duration::from_millis(10));
                }
                let _ = worker_finished.try_send(());
            })
            .map_err(|error| format!("Could not start the standalone task worker: {error}"))?
    };
    let app = runtime.handle();
    AppHandle::install_current(&app);
    let context: Arc<dyn GuiContext> = Arc::new(Context {
        session: session.clone(),
        app: app.clone(),
    });
    let options = app_window::options(P::NAME.to_owned(), editor.as_ref(), app_window::scale());
    let editor = Arc::new(Mutex::new(editor));
    Window::open_blocking(options, move |window| {
        let parent = match window.raw_window_handle() {
            raw_window_handle::RawWindowHandle::Xlib(handle) => {
                ParentWindowHandle::X11Window(handle.window as u32)
            }
            raw_window_handle::RawWindowHandle::Xcb(handle) => {
                ParentWindowHandle::X11Window(handle.window)
            }
            raw_window_handle::RawWindowHandle::AppKit(handle) => {
                ParentWindowHandle::AppKitNsView(handle.ns_view)
            }
            raw_window_handle::RawWindowHandle::Win32(handle) => {
                ParentWindowHandle::Win32Hwnd(handle.hwnd)
            }
            _ => panic!("The standalone window backend is unsupported."),
        };
        let handle = editor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .spawn(parent, context);
        AppWindow {
            _editor_handle: handle,
            editor,
            session,
            executor,
            app,
            audio_tasks: audio_gui_tasks,
        }
    });
    let closing = Instant::now();
    let result = runtime.shutdown();
    terminate.store(true, Ordering::Release);
    worker.thread().unpark();
    if worker_completion
        .recv_timeout(Duration::from_secs(2).saturating_sub(closing.elapsed()))
        .is_err()
    {
        return Err("The standalone task worker did not stop within two seconds.".into());
    }
    worker
        .join()
        .map_err(|_| "The standalone task worker failed.".to_owned())?;
    result
}

/// The app's window and its editor's scale (the CA-72's `docs/decisions.md` R-WIN-STANDALONE).
/// On Windows baseview made the process aware of each monitor's scale only once its first
/// window was open, and scaled that window by its monitor's scale while the editor in it, told
/// no scale, drew at 1: at 125 % and more the window opened a quarter or more larger than its
/// editor, black beyond it (the CA-72's issue 64). Now the process is aware of each monitor's
/// scale before anything is measured, and the window and the editor share the scale of the
/// monitor it opens on. baseview's change 8 (`third_party/baseview/PATCHES.md`) leaves the
/// window no frame to drag and no maximise box, which the editor does not follow; and on
/// Windows a resize from outside (Windows scales a window whose monitor's scale changes) is
/// undone at the next frame, [`refits`].
///
/// And its quitting (R-QUIT): every way of quitting the app closes its window, as its close
/// button does, so that the app shuts down as then, its session saved, once the window's
/// `open_blocking` returns. On macOS the application's menus are baseview's (its change 11:
/// Quit ⌘Q, the Dock's Quit and logging out close the window); on Windows Alt+F4 is the
/// system's (baseview leaves Alt's keys to `DefWindowProcW`, which closes the window); on
/// Linux Ctrl+Q is the app's own, [`quits`], which the editor gives the window
/// ([`request_quit`]) before any use of its own.
pub mod app_window {
    use std::sync::atomic::{AtomicBool, Ordering};

    use baseview::{Event, Size, Window, WindowEvent, WindowOpenOptions, WindowScalePolicy};
    use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

    use crate::editor::Editor;

    static QUIT: AtomicBool = AtomicBool::new(false);

    /// Whether `key` is this system's shortcut for quitting an application, where the app's
    /// windows are to act on it: Ctrl+Q on Linux (none on macOS, whose ⌘Q is the main menu's,
    /// taken before any view has it, or on Windows, whose Alt+F4 is the system's).
    pub fn quits(key: &KeyboardEvent) -> bool {
        quits_on(std::env::consts::OS, key)
    }

    /// [`quits`] on `os` (as `std::env::consts::OS` names it): Ctrl+Q pressed on Linux, Shift
    /// or not, with no other modifier, the Q of the keyboard's layout.
    pub fn quits_on(os: &str, key: &KeyboardEvent) -> bool {
        let others = Modifiers::ALT | Modifiers::ALT_GRAPH | Modifiers::META | Modifiers::SUPER;
        os == "linux"
            && key.state == KeyState::Down
            && key.modifiers.contains(Modifiers::CONTROL)
            && !key.modifiers.intersects(others)
            && matches!(&key.key, Key::Character(c) if c.eq_ignore_ascii_case("q"))
    }

    /// Ask the app's window to close, as its close button does, at its next frame: the app's
    /// editor's [`quits`] key, which reaches the editor's window and not the app's.
    pub fn request_quit() {
        QUIT.store(true, Ordering::Release);
    }

    /// Whether quitting has been asked for since this was last asked (once).
    pub fn quit_requested() -> bool {
        QUIT.swap(false, Ordering::AcqRel)
    }

    /// The scale the window opens at (the primary monitor's, where baseview opens it), the
    /// process made aware of each monitor's scale first. None where the window's own scale
    /// policy is kept (macOS, Linux), or where Windows does not say.
    #[cfg(target_os = "windows")]
    pub fn scale() -> Option<f32> {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
        use windows::Win32::UI::HiDpi::{
            GetDpiForMonitor, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, MDT_EFFECTIVE_DPI,
        };
        // SAFETY: plain calls with values of their own types. The awareness is baseview's,
        // which it sets after its first window; it fails harmlessly if the process has one.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let (mut x, mut y) = (0u32, 0u32);
            GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).ok()?;
            (x > 0).then(|| x as f32 / 96.0)
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn scale() -> Option<f32> {
        None
    }

    /// The window made its editor's `size` (logical pixels) at its scale.
    pub fn fit(window: &mut Window, (width, height): (u32, u32)) {
        window.resize(Size::new(f64::from(width), f64::from(height)));
    }

    /// Whether `event` asks for the window to be fitted to its editor again: on Windows, any
    /// resize, the window's own (fitted already, which changes nothing) or one from outside
    /// (a monitor's scale changed while the window was open: Windows scaled the window and left
    /// the editor, which cannot change its scale once open). Not elsewhere, where a window
    /// manager may keep a window at a size of its own, which the window would fight.
    pub fn refits(event: &Event) -> bool {
        cfg!(target_os = "windows") && matches!(event, Event::Window(WindowEvent::Resized(_)))
    }

    /// The window's options for `editor`: `scale` told the editor before it is first asked its
    /// size, and the window at the same scale where it takes it (else the system's, as before).
    pub fn options(title: String, editor: &dyn Editor, scale: Option<f32>) -> WindowOpenOptions {
        let scale = scale.filter(|&s| editor.set_scale_factor(s));
        let (width, height) = editor.size();
        WindowOpenOptions {
            title,
            size: Size::new(f64::from(width), f64::from(height)),
            scale: scale.map_or(WindowScalePolicy::SystemScaleFactor, |s| {
                WindowScalePolicy::ScaleFactor(f64::from(s))
            }),
        }
    }
}

#[cfg(test)]
#[path = "production_tests.rs"]
mod tests;
