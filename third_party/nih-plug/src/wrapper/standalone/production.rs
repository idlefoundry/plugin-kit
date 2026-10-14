//! The production standalone adapter. Unlike the development launcher, the
//! processor has one exclusive owner and never waits for the GUI in a callback.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use baseview::{
    Event, EventStatus, Size, Window, WindowHandler, WindowOpenOptions, WindowScalePolicy,
};
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

    fn process(&mut self, block: AudioBlock<'_>) {
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
            let (width, height) = self
                .editor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .size();
            window.resize(Size::new(f64::from(width), f64::from(height)));
        }
        if self.session.state_changed.swap(false, Ordering::AcqRel) {
            self.editor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .param_values_changed();
        }
    }
    fn on_event(&mut self, _window: &mut Window, _event: Event) -> EventStatus {
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
    let editor = Arc::new(Mutex::new(editor));
    let (width, height) = editor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .size();
    Window::open_blocking(
        WindowOpenOptions {
            title: P::NAME.to_owned(),
            size: Size::new(f64::from(width), f64::from(height)),
            scale: WindowScalePolicy::SystemScaleFactor,
            gl_config: None,
        },
        move |window| {
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
        },
    );
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
