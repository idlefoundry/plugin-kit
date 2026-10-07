use atomic_refcell::AtomicRefMut;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use vst3_sys::vst::IComponentHandler;

use crate::prelude::{
    GuiContext, InitContext, ParamPtr, PluginApi, PluginNoteEvent, PluginState, ProcessContext,
    ProcessMode, Transport, Vst3Plugin,
};

use super::inner::{Task, WrapperInner};
use crate::event_loop::EventLoop;
use crate::wrapper::util::OwnParamChange;

/// An [`InitContext`] implementation for the wrapper.
///
/// # Note
///
/// Requests to change the latency are only sent when this object is dropped. Otherwise there's the
/// risk that the host will immediately deactivate/reactivate the plugin while still in the init
/// call. Reentrannt function calls are difficult to handle in Rust without forcing everything to
/// use interior mutability, so this will have to do for now. This does mean that `Plugin` mutex
/// lock has to be dropped before this object.
pub(crate) struct WrapperInitContext<'a, P: Vst3Plugin> {
    pub(super) inner: &'a WrapperInner<P>,
    pub(super) pending_requests: PendingInitContextRequests,
}

/// Any requests that should be sent out when the [`WrapperInitContext`] is dropped. See that
/// struct's docstring for mroe information.
#[derive(Debug, Default)]
pub(crate) struct PendingInitContextRequests {
    /// The value of the last `.set_latency_samples()` call.
    latency_changed: Cell<Option<u32>>,
}

/// A [`ProcessContext`] implementation for the wrapper. This is a separate object so it can hold on
/// to lock guards for event queues. Otherwise reading these events would require constant
/// unnecessary atomic operations to lock the uncontested locks.
pub(crate) struct WrapperProcessContext<'a, P: Vst3Plugin> {
    pub(super) inner: &'a WrapperInner<P>,
    pub(super) input_events_guard: AtomicRefMut<'a, VecDeque<PluginNoteEvent<P>>>,
    pub(super) output_events_guard: AtomicRefMut<'a, VecDeque<PluginNoteEvent<P>>>,
    /// The parameters the plugin sets itself, for the host's output parameter changes, and
    /// whether the host gave a queue for them (PATCHES.md, change 10).
    pub(super) own_param_changes_guard: AtomicRefMut<'a, Vec<OwnParamChange>>,
    pub(super) host_listens: bool,
    pub(super) transport: Transport,
}

/// A [`GuiContext`] implementation for the wrapper. This is passed to the plugin in
/// [`Editor::spawn()`][crate::prelude::Editor::spawn()] so it can interact with the rest of the plugin and
/// with the host for things like setting parameters.
pub(crate) struct WrapperGuiContext<P: Vst3Plugin> {
    pub(super) inner: Arc<WrapperInner<P>>,
    #[cfg(debug_assertions)]
    pub(super) param_gesture_checker:
        atomic_refcell::AtomicRefCell<crate::wrapper::util::context_checks::ParamGestureChecker>,
}

impl<P: Vst3Plugin> Drop for WrapperInitContext<'_, P> {
    fn drop(&mut self) {
        if let Some(samples) = self.pending_requests.latency_changed.take() {
            self.inner.set_latency_samples(samples)
        }
    }
}

impl<P: Vst3Plugin> InitContext<P> for WrapperInitContext<'_, P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }

    fn execute(&self, task: P::BackgroundTask) {
        (self.inner.task_executor.lock())(task);
    }

    fn set_latency_samples(&self, samples: u32) {
        // See this struct's docstring
        self.pending_requests.latency_changed.set(Some(samples));
    }

    fn set_current_voice_capacity(&self, _capacity: u32) {
        // This is only supported by CLAP
    }
}

impl<P: Vst3Plugin> ProcessContext<P> for WrapperProcessContext<'_, P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }

    fn execute_background(&self, task: P::BackgroundTask) {
        let task_posted = self.inner.schedule_background(Task::PluginTask(task));
        nih_debug_assert!(task_posted, "The task queue is full, dropping task...");
    }

    fn execute_gui(&self, task: P::BackgroundTask) {
        let task_posted = self.inner.schedule_gui(Task::PluginTask(task));
        nih_debug_assert!(task_posted, "The task queue is full, dropping task...");
    }

    #[inline]
    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn process_mode(&self) -> ProcessMode {
        self.inner.current_process_mode.load()
    }

    fn next_event(&mut self) -> Option<PluginNoteEvent<P>> {
        self.input_events_guard.pop_front()
    }

    fn send_event(&mut self, event: PluginNoteEvent<P>) {
        self.output_events_guard.push_back(event);
    }

    fn set_latency_samples(&self, samples: u32) {
        self.inner.set_latency_samples(samples)
    }

    fn set_current_voice_capacity(&self, _capacity: u32) {
        // This is only supported by CLAP
    }

    fn set_parameter_normalized(&mut self, param: ParamPtr, normalized: f32, timing: u32) -> bool {
        let Some(&hash) = self.inner.param_ptr_to_hash.get(&param) else {
            nih_debug_assert_failure!("Unknown parameter: {:?}", param);
            return false;
        };
        // As the host's automation is set during a process call: the value, its smoother, and
        // the editor told.
        let sample_rate = self
            .inner
            .current_buffer_config
            .load()
            .map(|c| c.sample_rate);
        self.inner
            .set_normalized_value_by_hash(hash, normalized, sample_rate);
        let changes = &mut self.own_param_changes_guard;
        if !self.host_listens || changes.len() >= changes.capacity() {
            return false;
        }
        changes.push(OwnParamChange {
            hash,
            timing,
            // SAFETY: the pointer is one of the plugin's own parameters (`param_ptr_to_hash`),
            // which live as long as the wrapper.
            normalized: unsafe { param.unmodulated_normalized_value() },
        });
        true
    }
}

impl<P: Vst3Plugin> GuiContext for WrapperGuiContext<P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }

    fn request_resize(&self) -> bool {
        // On the GUI thread the host answers at once: its answer is returned, so an editor can
        // show itself another way where the host will not resize it (PATCHES.md, change 4).
        // Elsewhere (Linux, where baseview runs the editor on a thread of its own) the request
        // waits for the GUI thread, and its answer is not known.
        let on_gui_thread = self
            .inner
            .event_loop
            .borrow()
            .as_ref()
            .is_some_and(|event_loop| event_loop.is_main_thread());
        if on_gui_thread {
            return match &*self.inner.plug_view.read() {
                // SAFETY: this is the GUI thread, which `request_resize()` needs.
                Some(plug_view) => unsafe { plug_view.request_resize() },
                None => false,
            };
        }

        let task_posted = self.inner.schedule_gui(Task::RequestResize);
        nih_debug_assert!(task_posted, "The task queue is full, dropping task...");

        // TODO: We don't handle resize request failures right now. In practice this should however
        //       not happen.
        true
    }

    // All of these functions are supposed to be called from the main thread, so we'll put some
    // trust in the caller and assume that this is indeed the case
    unsafe fn raw_begin_set_parameter(&self, param: ParamPtr) {
        match &*self.inner.component_handler.borrow() {
            Some(handler) => match self.inner.param_ptr_to_hash.get(&param) {
                Some(hash) => {
                    handler.begin_edit(*hash);
                }
                None => nih_debug_assert_failure!("Unknown parameter: {:?}", param),
            },
            None => nih_debug_assert_failure!("Component handler not yet set"),
        }

        #[cfg(debug_assertions)]
        match self.inner.param_id_from_ptr(param) {
            Some(param_id) => self
                .param_gesture_checker
                .borrow_mut()
                .begin_set_parameter(param_id),
            None => nih_debug_assert_failure!(
                "raw_begin_set_parameter() called with an unknown ParamPtr"
            ),
        }
    }

    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        match &*self.inner.component_handler.borrow() {
            Some(handler) => match self.inner.param_ptr_to_hash.get(&param) {
                Some(hash) => {
                    // While the host processes audio, the value is not changed here, so that it
                    // never changes in the middle of a process call: the edit is held, and the
                    // next process call sets it at its start (PATCHES.md, change 14). Until then
                    // it is the value the controller reports, since a host may read it back as
                    // the gesture ends and send that to the processor: Cubase 15 does, within
                    // `endEdit()`, and the value from before the edit undid it. Upstream left the
                    // value to the host's echo of the edit, which a host that stops processing
                    // without saying so (REAPER, bypassing the plugin) never sends.
                    if self.inner.is_processing.load(Ordering::SeqCst) {
                        self.inner.hold_edit(*hash, normalized);
                    } else {
                        self.inner.set_normalized_value_by_hash(
                            *hash,
                            normalized,
                            self.inner
                                .current_buffer_config
                                .load()
                                .map(|c| c.sample_rate),
                        );
                    }

                    handler.perform_edit(*hash, normalized as f64);
                }
                None => nih_debug_assert_failure!("Unknown parameter: {:?}", param),
            },
            None => nih_debug_assert_failure!("Component handler not yet set"),
        }

        #[cfg(debug_assertions)]
        match self.inner.param_id_from_ptr(param) {
            Some(param_id) => self
                .param_gesture_checker
                .borrow_mut()
                .set_parameter(param_id),
            None => {
                nih_debug_assert_failure!("raw_set_parameter() called with an unknown ParamPtr")
            }
        }
    }

    unsafe fn raw_end_set_parameter(&self, param: ParamPtr) {
        match &*self.inner.component_handler.borrow() {
            Some(handler) => match self.inner.param_ptr_to_hash.get(&param) {
                Some(hash) => {
                    handler.end_edit(*hash);
                }
                None => nih_debug_assert_failure!("Unknown parameter: {:?}", param),
            },
            None => nih_debug_assert_failure!("Component handler not yet set"),
        }

        #[cfg(debug_assertions)]
        match self.inner.param_id_from_ptr(param) {
            Some(param_id) => self
                .param_gesture_checker
                .borrow_mut()
                .end_set_parameter(param_id),
            None => {
                nih_debug_assert_failure!("raw_end_set_parameter() called with an unknown ParamPtr")
            }
        }
    }

    fn get_state(&self) -> PluginState {
        self.inner.get_state_object()
    }

    fn set_state(&self, state: PluginState) {
        self.inner.set_state_object_from_gui(state)
    }
}
