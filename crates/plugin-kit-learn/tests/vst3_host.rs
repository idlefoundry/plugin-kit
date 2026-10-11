//! The kit's VST3 wrapper (`third_party/nih-plug/PATCHES.md`, change 14; K4), driven through
//! its own factory as Cubase 15 drives it (the CA-72's `docs/decisions.md` R36). An edit made
//! in the editor while the host processes audio reaches the processor whatever the host does
//! with it: Cubase 15 reads the controller's value within `endEdit()` and sends that to the
//! processor at the next process call (before change 14 it read the value from before the
//! edit, so a click on a switch did nothing, and a quick drag ended one move short); a host
//! that sends nothing back still has the edit set at the next process call; the host's own
//! change at that call's first sample follows the edit; and an edit made just before
//! processing stops is kept.
//!
//! And a key the host gives the editor's view (`IPlugView::onKeyDown()`, change 15: Cubase keeps
//! the keyboard from a plug-in's window and gives it its keys this way) reaches the editor, the
//! host told whether the editor took it (the CA-72's `docs/decisions.md` R-KEYS).
//!
//! It is here because this is the kit's one crate on nih-plug, with the VST3 wrapper switched
//! on for its tests alone. The plug-in is a probe: its editor opens no window but keeps the
//! context the wrapper gives it, the one a plug-in's editor sets its parameters through, and
//! its processor notes the values it sees. Everything runs on the test's thread, the host's
//! main thread and audio thread taking turns as Cubase's do between process calls.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::num::NonZeroU32;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use nih_plug::wrapper::vst3::vst3_sys;
use vst3_sys::base::{IPluginBase, IPluginFactory, kResultFalse, kResultOk, tresult};
use vst3_sys::gui::IPlugView;
use vst3_sys::utils::{SharedVstPtr, StaticVstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, IAudioProcessor, IComponent, IComponentHandler, IEditController,
    IParamValueQueue, IParameterChanges, ProcessData, ProcessModes, ProcessSetup,
    SymbolicSampleSizes,
};
use vst3_sys::{ComInterface, IID, VST3, VstPtr};
// The name the `VST3` attribute's expansion uses.
use vst3_sys as vst3_com;

const RATE: f64 = 48_000.0;
const BLOCK: usize = 64;

/// The platform name `IPlugView::attached()` is given, as a host on this system gives it.
#[cfg(target_os = "windows")]
const PLATFORM: &CStr = c"HWND";
#[cfg(target_os = "macos")]
const PLATFORM: &CStr = c"NSView";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const PLATFORM: &CStr = c"X11EmbedWindowID";

/// A parameter's VST3 id: nih-plug's hash of its string id (`wrapper::util::hash_param_id`).
fn id(param: &str) -> u32 {
    let mut h: u32 = 0;
    for b in param.bytes() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(b));
    }
    h & !(1 << 31)
}

thread_local! {
    /// The parameters of the probe made on this thread, as its editor and processor share them.
    static PARAMS: RefCell<Option<Arc<ProbeParams>>> = const { RefCell::new(None) };
    /// The context the wrapper gave the probe's editor as the host attached it.
    static CONTEXT: RefCell<Option<Arc<dyn GuiContext>>> = const { RefCell::new(None) };
    /// The values the probe's processor last saw: SWITCH and LEVEL.
    static SEEN: Cell<Option<(bool, f32)>> = const { Cell::new(None) };
    /// The keys the host gave the probe's editor's view, and whether its editor takes them.
    static KEYS: RefCell<Vec<nih_plug::editor::HostKey>> = const { RefCell::new(Vec::new()) };
    static TAKES_KEYS: Cell<bool> = const { Cell::new(false) };
}

#[derive(Params)]
struct ProbeParams {
    /// A switch, as the CA-72's rockers and buttons are (on by default).
    #[id = "switch"]
    switch: BoolParam,
    /// A knob, unsmoothed (0.5 by default).
    #[id = "level"]
    level: FloatParam,
}

struct Probe {
    params: Arc<ProbeParams>,
}

impl Default for Probe {
    fn default() -> Self {
        let params = Arc::new(ProbeParams {
            switch: BoolParam::new("Switch", true),
            level: FloatParam::new("Level", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 }),
        });
        PARAMS.with(|p| *p.borrow_mut() = Some(params.clone()));
        Self { params }
    }
}

impl Plugin for Probe {
    const NAME: &'static str = "Probe";
    const VENDOR: &'static str = "Idle Foundry";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0.0.0";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    // As the CA-72's, the CA-74's and the MC-79's.
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(ProbeEditor))
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        SEEN.with(|s| {
            s.set(Some((
                self.params.switch.value(),
                self.params.level.value(),
            )))
        });
        for channel in buffer.as_slice() {
            channel.fill(0.0);
        }
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for Probe {
    const VST3_CLASS_ID: [u8; 16] = *b"IdleFoundryProbe";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument];
}

// Its factory, `GetPluginFactory()`, as a host finds it.
nih_export_vst3!(Probe);

/// An editor without a window: it keeps the context it is given.
struct ProbeEditor;

impl Editor for ProbeEditor {
    fn spawn(
        &self,
        _parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn Any + Send> {
        CONTEXT.with(|c| *c.borrow_mut() = Some(context));
        Box::new(())
    }
    fn size(&self) -> (u32, u32) {
        (100, 100)
    }
    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }
    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
    fn on_host_key(&self, key: nih_plug::editor::HostKey) -> bool {
        KEYS.with(|k| k.borrow_mut().push(key));
        TAKES_KEYS.with(Cell::get)
    }
}

/// What the host does with the editor's edits.
#[derive(Clone, Copy, PartialEq)]
enum Host {
    /// Cubase 15 (Windows 11, 2026-10-07): it sends each `performEdit()` to the processor at the
    /// next process call, and within `endEdit()` it reads the controller's value
    /// (`getParamNormalized()`) and sends that instead: one change a parameter a call, its last.
    Cubase,
    /// It sends nothing back.
    Silent,
}

/// The host's record of the editor's edits.
#[derive(Default)]
struct Ledger {
    /// The parameter changes for the next process call, at its first sample: the last value
    /// for each parameter.
    next: Vec<(u32, f64)>,
    /// The values the host read within `endEdit()`.
    read_back: Vec<(u32, f64)>,
}

impl Ledger {
    fn send(&mut self, id: u32, value: f64) {
        match self.next.iter_mut().find(|(i, _)| *i == id) {
            Some(change) => change.1 = value,
            None => self.next.push((id, value)),
        }
    }
}

/// The host's `IComponentHandler`, which the wrapper calls with the editor's edits.
#[VST3(implements(IComponentHandler))]
struct Handler {
    host: Host,
    /// The instance's `IEditController`, unowned: the instance outlives its handler's use.
    controller: usize,
    ledger: Arc<Mutex<Ledger>>,
}

impl IComponentHandler for Handler {
    unsafe fn begin_edit(&self, _id: u32) -> tresult {
        kResultOk
    }

    unsafe fn perform_edit(&self, id: u32, value: f64) -> tresult {
        if self.host == Host::Cubase {
            self.ledger.lock().unwrap().send(id, value);
        }
        kResultOk
    }

    unsafe fn end_edit(&self, id: u32) -> tresult {
        if self.host == Host::Cubase {
            // SAFETY: the instance's controller, alive while it edits (`controller`).
            let controller =
                unsafe { VstPtr::<dyn IEditController>::shared(self.controller as *mut _) }
                    .unwrap();
            // SAFETY: a call on the controller's thread, as the host makes it.
            let value = unsafe { controller.get_param_normalized(id) };
            let mut ledger = self.ledger.lock().unwrap();
            ledger.read_back.push((id, value));
            ledger.send(id, value);
        }
        kResultOk
    }

    unsafe fn restart_component(&self, _flags: i32) -> tresult {
        kResultOk
    }
}

/// One parameter's change at the first sample of a process call.
#[VST3(implements(IParamValueQueue))]
struct Queue {
    id: u32,
    value: f64,
}

impl IParamValueQueue for Queue {
    unsafe fn get_parameter_id(&self) -> u32 {
        self.id
    }

    unsafe fn get_point_count(&self) -> i32 {
        1
    }

    unsafe fn get_point(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> tresult {
        if index != 0 {
            return kResultFalse;
        }
        // SAFETY: the plug-in's pointers to its own locals, for this call.
        unsafe {
            *sample_offset = 0;
            *value = self.value;
        }
        kResultOk
    }

    unsafe fn add_point(&self, _offset: i32, _value: f64, _index: *mut i32) -> tresult {
        kResultFalse
    }
}

/// A process call's parameter changes.
#[VST3(implements(IParameterChanges))]
struct Changes {
    queues: Vec<VstPtr<dyn IParamValueQueue>>,
}

impl IParameterChanges for Changes {
    unsafe fn get_parameter_count(&self) -> i32 {
        self.queues.len() as i32
    }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        let queue = usize::try_from(index).ok().and_then(|i| self.queues.get(i));
        // SAFETY: a pointer `StaticVstPtr` wraps unowned, alive with this object.
        unsafe { pointer(queue.map_or(null_mut(), |q| q.as_ptr().cast())) }
    }

    unsafe fn add_parameter_data(
        &self,
        _id: *const u32,
        _index: *mut i32,
    ) -> StaticVstPtr<dyn IParamValueQueue> {
        // SAFETY: a null pointer.
        unsafe { pointer(null_mut()) }
    }
}

/// An interface pointer as `SharedVstPtr` or `StaticVstPtr`, each a transparent raw pointer.
///
/// # Safety
///
/// `P` is one of those two, for the interface `p` points to (or null).
unsafe fn pointer<P>(p: *mut c_void) -> P {
    assert_eq!(size_of::<P>(), size_of::<*mut c_void>());
    // SAFETY: the same representation (above, and the caller's promise of the type).
    unsafe { std::mem::transmute_copy(&p) }
}

/// One of the host's COM objects as its first (only) interface, owned by the pointer returned.
fn com<I: ComInterface + ?Sized, T>(object: Box<T>) -> VstPtr<I> {
    // SAFETY: a `VST3` object's first field is its first interface's vtable pointer, and it
    // begins with one reference, which the pointer takes.
    unsafe { VstPtr::owned(Box::into_raw(object).cast()) }.unwrap()
}

/// A probe instance in a host: made, set up, processing, its editor attached.
struct Instance {
    component: VstPtr<dyn IComponent>,
    controller: VstPtr<dyn IEditController>,
    processor: VstPtr<dyn IAudioProcessor>,
    view: VstPtr<dyn IPlugView>,
    _handler: VstPtr<dyn IComponentHandler>,
    ledger: Arc<Mutex<Ledger>>,
    params: Arc<ProbeParams>,
    context: Arc<dyn GuiContext>,
}

impl Instance {
    fn new(host: Host) -> Self {
        // SAFETY: the factory `nih_export_vst3!` made in this test, owned by the caller.
        let factory =
            unsafe { VstPtr::<dyn IPluginFactory>::owned(GetPluginFactory().cast()) }.unwrap();
        let cid = IID {
            data: Probe::PLATFORM_VST3_CLASS_ID,
        };
        let mut object = null_mut();
        // SAFETY: the factory's class, as `IComponent`, into a local (each call below likewise is
        // the host's call with valid pointers to locals, on the thread a host makes it).
        unsafe {
            assert_eq!(
                factory.create_instance(&cid, &<dyn IComponent as ComInterface>::IID, &mut object),
                kResultOk
            );
        }
        // SAFETY: the reference `create_instance` returned.
        let component = unsafe { VstPtr::<dyn IComponent>::owned(object.cast()) }.unwrap();
        let controller: VstPtr<dyn IEditController> = component.cast().unwrap();
        let processor: VstPtr<dyn IAudioProcessor> = component.cast().unwrap();
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let handler: VstPtr<dyn IComponentHandler> = com(Handler::allocate(
            host,
            controller.as_ptr() as usize,
            ledger.clone(),
        ));
        let setup = ProcessSetup {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: BLOCK as i32,
            sample_rate: RATE,
        };
        let view;
        // SAFETY: as above.
        unsafe {
            assert_eq!(component.initialize(null_mut()), kResultOk);
            let shared: SharedVstPtr<dyn IComponentHandler> = pointer(handler.as_ptr().cast());
            assert_eq!(controller.set_component_handler(shared), kResultOk);
            assert_eq!(processor.setup_processing(&setup), kResultOk);
            assert_eq!(component.set_active(1), kResultOk);
            assert_eq!(processor.set_processing(1), kResultOk);
            view =
                VstPtr::<dyn IPlugView>::owned(controller.create_view(c"editor".as_ptr()).cast())
                    .unwrap();
            // The probe's editor never uses its parent.
            let parent = std::ptr::NonNull::<c_void>::dangling().as_ptr();
            assert_eq!(view.attached(parent, PLATFORM.as_ptr()), kResultOk);
        }
        let params = PARAMS.with(|p| p.borrow_mut().take()).unwrap();
        let context = CONTEXT.with(|c| c.borrow_mut().take()).unwrap();
        let instance = Self {
            component,
            controller,
            processor,
            view,
            _handler: handler,
            ledger,
            params,
            context,
        };
        instance.process();
        assert_eq!(instance.seen(), (true, 0.5));
        instance
    }

    /// A process call of one block, with the parameter changes the host has for it.
    fn process(&self) {
        let next = std::mem::take(&mut self.ledger.lock().unwrap().next);
        let changes: Option<VstPtr<dyn IParameterChanges>> = (!next.is_empty()).then(|| {
            com(Changes::allocate(
                next.iter()
                    .map(|&(id, value)| com(Queue::allocate(id, value)))
                    .collect(),
            ))
        });
        let mut left = [0.0f32; BLOCK];
        let mut right = [0.0f32; BLOCK];
        let mut channels: [*mut c_void; 2] = [left.as_mut_ptr().cast(), right.as_mut_ptr().cast()];
        let mut output = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: channels.as_mut_ptr(),
        };
        // SAFETY: as in `new` (null for what the host does not give).
        unsafe {
            let mut data = ProcessData {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
                num_samples: BLOCK as i32,
                num_inputs: 0,
                num_outputs: 1,
                inputs: null_mut(),
                outputs: &mut output,
                input_param_changes: pointer(
                    changes.as_ref().map_or(null_mut(), |c| c.as_ptr().cast()),
                ),
                output_param_changes: pointer(null_mut()),
                input_events: pointer(null_mut()),
                output_events: pointer(null_mut()),
                context: null_mut(),
            };
            assert_eq!(self.processor.process(&mut data), kResultOk);
        }
    }

    /// What the processor saw in the last process call: SWITCH and LEVEL.
    fn seen(&self) -> (bool, f32) {
        SEEN.with(Cell::get).unwrap()
    }

    /// The value the controller reports to the host.
    fn reported(&self, param: &str) -> f64 {
        // SAFETY: as in `new`.
        unsafe { self.controller.get_param_normalized(id(param)) }
    }

    /// A click on the switch in the editor: a whole gesture between two process calls.
    fn click(&self, on: bool) {
        let setter = ParamSetter::new(self.context.as_ref());
        setter.begin_set_parameter(&self.params.switch);
        setter.set_parameter(&self.params.switch, on);
        setter.end_set_parameter(&self.params.switch);
    }

    fn read_back(&self) -> Vec<(u32, f64)> {
        std::mem::take(&mut self.ledger.lock().unwrap().read_back)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: as in `new`.
        unsafe {
            self.view.removed();
            self.processor.set_processing(0);
            self.component.set_active(0);
            self.component.terminate();
        }
    }
}

/// Cubase 15's click on a switch: the host reads the edit back as the gesture ends, and the
/// processor then has it, and keeps it. Before change 14 the read gave the value from before the
/// edit, which the host sent on, and the switch never moved.
#[test]
fn a_click_survives_a_host_that_reads_the_value_back_as_the_gesture_ends() {
    let i = Instance::new(Host::Cubase);
    i.click(false);
    assert_eq!(i.read_back(), [(id("switch"), 0.0)]);
    assert_eq!(i.reported("switch"), 0.0);
    i.process();
    assert_eq!(i.seen(), (false, 0.5));
    i.process();
    assert_eq!(i.seen(), (false, 0.5));
    assert_eq!(i.reported("switch"), 0.0);

    i.click(true);
    assert_eq!(i.read_back(), [(id("switch"), 1.0)]);
    i.process();
    assert_eq!(i.seen(), (true, 0.5));
}

/// A quick drag of a knob in Cubase 15: a process call between its moves, and the last move and
/// the release before the next one. The host's read as the gesture ends gives the last move, not
/// the move before it that the last process call set, and the knob stays where it was let go.
#[test]
fn a_drag_ends_where_it_was_let_go_in_a_host_that_reads_the_value_back() {
    let i = Instance::new(Host::Cubase);
    let setter = ParamSetter::new(i.context.as_ref());
    setter.begin_set_parameter(&i.params.level);
    setter.set_parameter(&i.params.level, 0.25);
    i.process();
    assert_eq!(i.seen(), (true, 0.25));
    setter.set_parameter(&i.params.level, 0.75);
    setter.end_set_parameter(&i.params.level);
    assert_eq!(i.read_back(), [(id("level"), 0.75)]);
    i.process();
    assert_eq!(i.seen(), (true, 0.75));
}

/// A host that sends nothing back (REAPER, its plug-in bypassed, stops calling it without saying
/// so; this one goes on calling): the edit is set at the next process call all the same.
#[test]
fn an_edit_reaches_the_processor_from_a_host_that_sends_nothing_back() {
    let i = Instance::new(Host::Silent);
    i.click(false);
    assert_eq!(i.reported("switch"), 0.0);
    i.process();
    assert_eq!(i.seen(), (false, 0.5));
    assert_eq!(i.reported("switch"), 0.0);
}

/// The host's own change at the first sample of the next process call (its automation) follows
/// the editor's edit made before the call, as it would had the host sent the edit itself.
#[test]
fn the_hosts_own_change_at_the_first_sample_follows_the_edit() {
    let i = Instance::new(Host::Silent);
    let setter = ParamSetter::new(i.context.as_ref());
    setter.begin_set_parameter(&i.params.level);
    setter.set_parameter(&i.params.level, 0.25);
    setter.end_set_parameter(&i.params.level);
    i.ledger.lock().unwrap().send(id("level"), 0.75);
    i.process();
    assert_eq!(i.seen(), (true, 0.75));
    i.process();
    assert_eq!(i.seen(), (true, 0.75));
    assert_eq!(i.reported("level"), 0.75);
}

/// An edit made just before the host stops processing, with no process call after it, is set
/// as processing stops.
#[test]
fn an_edit_made_as_processing_stops_is_kept() {
    let i = Instance::new(Host::Silent);
    i.click(false);
    // SAFETY: as in `Instance::new`.
    unsafe { assert_eq!(i.processor.set_processing(0), kResultOk) };
    assert!(!i.params.switch.value());
    assert_eq!(i.reported("switch"), 0.0);
}

/// With the host not processing, an edit is set at once, as before change 14.
#[test]
fn an_edit_while_not_processing_is_set_at_once() {
    let i = Instance::new(Host::Silent);
    // SAFETY: as in `Instance::new`.
    unsafe { assert_eq!(i.processor.set_processing(0), kResultOk) };
    i.click(false);
    assert!(!i.params.switch.value());
    assert_eq!(i.reported("switch"), 0.0);
}

/// A key the host gives the view (change 15; the CA-72's R-KEYS) reaches the editor with its
/// character, VST3 code, modifiers and state, down or up; the host is told it was taken only if
/// the editor took it (else the key is the host's: its key command).
#[test]
fn a_key_the_host_gives_the_view_reaches_the_editor() {
    use nih_plug::editor::HostKey;
    let i = Instance::new(Host::Cubase);
    TAKES_KEYS.with(|t| t.set(false));
    // SAFETY: the view the instance made, on the host's main thread.
    unsafe { assert_eq!(i.view.on_key_down('8' as i16, 0, 0), kResultFalse) };
    TAKES_KEYS.with(|t| t.set(true));
    // SAFETY: as above.
    unsafe {
        assert_eq!(i.view.on_key_down('8' as i16, 0, 1), kResultOk);
        assert_eq!(i.view.on_key_up(13, 4, 0), kResultOk);
    }
    let key = |character, key_code, modifiers, down| HostKey {
        character,
        key_code,
        modifiers,
        down,
    };
    assert_eq!(
        KEYS.with(|k| std::mem::take(&mut *k.borrow_mut())),
        vec![
            key(Some('8'), 0, 0, true),
            key(Some('8'), 0, 1, true),
            key(Some('\r'), 4, 0, false),
        ]
    );
}
