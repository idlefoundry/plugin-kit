//! A context passed during the process function.

use std::collections::VecDeque;

use super::PluginApi;
use crate::prelude::{ParamPtr, Plugin, PluginNoteEvent, ProcessMode};

/// Contains both context data and callbacks the plugin can use during processing. Most notably this
/// is how a plugin sends and receives note events, gets transport information, and accesses
/// sidechain inputs and auxiliary outputs. This is passed to the plugin during as part of
/// [`Plugin::process()`][crate::plugin::Plugin::process()].
//
// # Safety
//
// The implementing wrapper needs to be able to handle concurrent requests, and it should perform
// the actual callback within [MainThreadQueue::schedule_gui].
pub trait ProcessContext<P: Plugin> {
    /// Get the current plugin API.
    fn plugin_api(&self) -> PluginApi;

    /// Execute a task on a background thread using `[Plugin::task_executor]`. This allows you to
    /// defer expensive tasks for later without blocking either the process function or the GUI
    /// thread. As long as creating the `task` is realtime-safe, this operation is too.
    ///
    /// # Note
    ///
    /// Scheduling the same task multiple times will cause those duplicate tasks to pile up. Try to
    /// either prevent this from happening, or check whether the task still needs to be completed in
    /// your task executor.
    fn execute_background(&self, task: P::BackgroundTask);

    /// Execute a task on a background thread using `[Plugin::task_executor]`. As long as creating
    /// the `task` is realtime-safe, this operation is too.
    ///
    /// # Note
    ///
    /// Scheduling the same task multiple times will cause those duplicate tasks to pile up. Try to
    /// either prevent this from happening, or check whether the task still needs to be completed in
    /// your task executor.
    fn execute_gui(&self, task: P::BackgroundTask);

    /// Get information about the current transport position and status.
    fn transport(&self) -> &Transport;

    /// The current processing mode. Unlike [`BufferConfig::process_mode`], this follows a mode the
    /// host sets while the plugin is active (CLAP's `render` extension may be used at any time,
    /// and the plugin is not reinitialized for it).
    ///
    /// [`BufferConfig::process_mode`]: crate::prelude::BufferConfig::process_mode
    fn process_mode(&self) -> ProcessMode;

    /// Returns the next note event, if there is one. Use
    /// [`NoteEvent::timing()`][crate::prelude::NoteEvent::timing()] to get the event's timing
    /// within the buffer. Only available when
    /// [`Plugin::MIDI_INPUT`][crate::prelude::Plugin::MIDI_INPUT] is set.
    ///
    /// # Usage
    ///
    /// You will likely want to use this with a loop, since there may be zero, one, or more events
    /// for a sample:
    ///
    /// ```ignore
    /// let mut next_event = context.next_event();
    /// for (sample_id, channel_samples) in buffer.iter_samples().enumerate() {
    ///     while let Some(event) = next_event {
    ///         if event.timing() != sample_id as u32 {
    ///             break;
    ///         }
    ///
    ///         match event {
    ///             NoteEvent::NoteOn { note, velocity, .. } => { ... },
    ///             NoteEvent::NoteOff { note, .. } if note == 69 => { ... },
    ///             NoteEvent::PolyPressure { note, pressure, .. } { ... },
    ///             _ => (),
    ///         }
    ///
    ///         next_event = context.next_event();
    ///     }
    ///
    ///     // Do something with `channel_samples`...
    /// }
    ///
    /// ProcessStatus::Normal
    /// ```
    fn next_event(&mut self) -> Option<PluginNoteEvent<P>>;

    /// Send an event to the host. Only available when
    /// [`Plugin::MIDI_OUTPUT`][crate::prelude::Plugin::MIDI_INPUT] is set. Will not do anything
    /// otherwise.
    fn send_event(&mut self, event: PluginNoteEvent<P>);

    /// Update the current latency of the plugin. If the plugin is currently processing audio, then
    /// this may cause audio playback to be restarted.
    fn set_latency_samples(&self, samples: u32);

    /// Set the current voice **capacity** for this plugin (so not the number of currently active
    /// voices). This may only be called if
    /// [`ClapPlugin::CLAP_POLY_MODULATION_CONFIG`][crate::prelude::ClapPlugin::CLAP_POLY_MODULATION_CONFIG]
    /// is set. `capacity` must be between 1 and the configured maximum capacity. Changing this at
    /// runtime allows the host to better optimize polyphonic modulation, or to switch to strictly
    /// monophonic modulation when dropping the capacity down to 1.
    fn set_current_voice_capacity(&self, capacity: u32);

    /// Set a parameter from the audio thread as a change of the plugin's own (a MIDI controller
    /// it has learned, say), `timing` samples into the block given to this `process()` call. The
    /// value is set at once, as the host's automation sets it during processing, so the plugin
    /// reads it from here on and the editor is told; and the host is told as its format has it:
    ///
    /// - CLAP: a `CLAP_EVENT_PARAM_VALUE` output event at that time, flagged
    ///   `CLAP_EVENT_DONT_RECORD` (what changed it, a MIDI message say, is what the host records:
    ///   recording the parameter as well would conflict with it, as `clap/ext/params.h` says).
    /// - VST3: a point in the process call's output parameter changes, at that sample offset.
    ///   VST3 has no flag against recording: a host that writes automation for these may record
    ///   it.
    /// - The standalone: no host to tell.
    ///
    /// No gesture is begun or ended. Realtime-safe: nothing is allocated, locked or waited for.
    /// Returns whether the host will be told: false when the parameter is not the plugin's (then
    /// nothing is set), the host gave no queue for such changes, or this block already holds as
    /// many as it can tell (the value is set either way).
    ///
    /// Not upstream's: the CA-72's `third_party/nih-plug/PATCHES.md`, change 10.
    fn set_parameter_normalized(&mut self, param: ParamPtr, normalized: f32, timing: u32) -> bool;
}

/// Information about the plugin's transport. Depending on the plugin API and the host not all
/// fields may be available.
#[derive(Debug)]
pub struct Transport {
    /// Whether the transport is currently running.
    pub playing: bool,
    /// Whether recording is enabled in the project.
    pub recording: bool,
    /// Whether the pre-roll is currently active, if the plugin API reports this information.
    pub preroll_active: Option<bool>,

    /// The sample rate in Hertz. Also passed in
    /// [`Plugin::initialize()`][crate::prelude::Plugin::initialize()], so if you need this then you
    /// can also store that value.
    pub sample_rate: f32,
    /// The project's tempo in beats per minute.
    pub tempo: Option<f64>,
    /// The time signature's numerator.
    pub time_sig_numerator: Option<i32>,
    /// The time signature's denominator.
    pub time_sig_denominator: Option<i32>,

    // XXX: VST3 also has a continuous time in samples that ignores loops, but we can't reconstruct
    //      something similar in CLAP so it may be best to just ignore that so you can't rely on it
    /// The position in the song in samples. Can be used to calculate the time in seconds if needed.
    pub(crate) pos_samples: Option<i64>,
    /// The position in the song in seconds. Can be used to calculate the time in samples if needed.
    pub(crate) pos_seconds: Option<f64>,
    /// The position in the song in quarter notes. Can be calculated from the time in seconds and
    /// the tempo if needed.
    pub(crate) pos_beats: Option<f64>,
    /// The last bar's start position in beats. Can be calculated from the beat position and time
    /// signature if needed.
    pub(crate) bar_start_pos_beats: Option<f64>,
    /// The number of the bar at `bar_start_pos_beats`. This starts at 0 for the very first bar at
    /// the start of the song. Can be calculated from the beat position and time signature if
    /// needed.
    pub(crate) bar_number: Option<i32>,

    /// The loop range in samples, if the loop is active and this information is available. None of
    /// the plugin API docs mention whether this is exclusive or inclusive, but just assume that the
    /// end is exclusive. Can be calculated from the other loop range information if needed.
    pub(crate) loop_range_samples: Option<(i64, i64)>,
    /// The loop range in seconds, if the loop is active and this information is available. None of
    /// the plugin API docs mention whether this is exclusive or inclusive, but just assume that the
    /// end is exclusive. Can be calculated from the other loop range information if needed.
    pub(crate) loop_range_seconds: Option<(f64, f64)>,
    /// The loop range in quarter notes, if the loop is active and this information is available.
    /// None of the plugin API docs mention whether this is exclusive or inclusive, but just assume
    /// that the end is exclusive. Can be calculated from the other loop range information if
    /// needed.
    pub(crate) loop_range_beats: Option<(f64, f64)>,
}

impl Transport {
    /// Initialize the transport struct without any information.
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            playing: false,
            recording: false,
            preroll_active: None,

            sample_rate,
            tempo: None,
            time_sig_numerator: None,
            time_sig_denominator: None,

            pos_samples: None,
            pos_seconds: None,
            pos_beats: None,
            bar_start_pos_beats: None,
            bar_number: None,

            loop_range_samples: None,
            loop_range_seconds: None,
            loop_range_beats: None,
        }
    }

    /// The position in the song in samples. Will be calculated from other information if needed.
    pub fn pos_samples(&self) -> Option<i64> {
        match (
            self.pos_samples,
            self.pos_seconds,
            self.pos_beats,
            self.tempo,
        ) {
            (Some(pos_samples), _, _, _) => Some(pos_samples),
            (_, Some(pos_seconds), _, _) => {
                Some((pos_seconds * self.sample_rate as f64).round() as i64)
            }
            (_, _, Some(pos_beats), Some(tempo)) => {
                Some((pos_beats / tempo * 60.0 * self.sample_rate as f64).round() as i64)
            }
            (_, _, _, _) => None,
        }
    }

    /// The position in the song in seconds. Can be used to calculate the time in samples if needed.
    pub fn pos_seconds(&self) -> Option<f64> {
        match (
            self.pos_samples,
            self.pos_seconds,
            self.pos_beats,
            self.tempo,
        ) {
            (_, Some(pos_seconds), _, _) => Some(pos_seconds),
            (Some(pos_samples), _, _, _) => Some(pos_samples as f64 / self.sample_rate as f64),
            (_, _, Some(pos_beats), Some(tempo)) => Some(pos_beats / tempo * 60.0),
            (_, _, _, _) => None,
        }
    }

    /// The position in the song in quarter notes. Will be calculated from other information if
    /// needed.
    pub fn pos_beats(&self) -> Option<f64> {
        match (
            self.pos_samples,
            self.pos_seconds,
            self.pos_beats,
            self.tempo,
        ) {
            (_, _, Some(pos_beats), _) => Some(pos_beats),
            (_, Some(pos_seconds), _, Some(tempo)) => Some(pos_seconds / 60.0 * tempo),
            (Some(pos_samples), _, _, Some(tempo)) => {
                Some(pos_samples as f64 / self.sample_rate as f64 / 60.0 * tempo)
            }
            (_, _, _, _) => None,
        }
    }

    /// The last bar's start position in beats. Will be calculated from other information if needed.
    pub fn bar_start_pos_beats(&self) -> Option<f64> {
        if self.bar_start_pos_beats.is_some() {
            return self.bar_start_pos_beats;
        }

        match (
            self.time_sig_numerator,
            self.time_sig_denominator,
            self.pos_beats(),
        ) {
            (Some(time_sig_numerator), Some(time_sig_denominator), Some(pos_beats)) => {
                let quarter_note_bar_length =
                    time_sig_numerator as f64 / time_sig_denominator as f64 * 4.0;
                Some((pos_beats / quarter_note_bar_length).floor() * quarter_note_bar_length)
            }
            (_, _, _) => None,
        }
    }

    /// The number of the bar at `bar_start_pos_beats`. This starts at 0 for the very first bar at
    /// the start of the song. Will be calculated from other information if needed.
    pub fn bar_number(&self) -> Option<i32> {
        if self.bar_number.is_some() {
            return self.bar_number;
        }

        match (
            self.time_sig_numerator,
            self.time_sig_denominator,
            self.pos_beats(),
        ) {
            (Some(time_sig_numerator), Some(time_sig_denominator), Some(pos_beats)) => {
                let quarter_note_bar_length =
                    time_sig_numerator as f64 / time_sig_denominator as f64 * 4.0;
                Some((pos_beats / quarter_note_bar_length).floor() as i32)
            }
            (_, _, _) => None,
        }
    }

    /// The loop range in samples, if the loop is active and this information is available. None of
    /// the plugin API docs mention whether this is exclusive or inclusive, but just assume that the
    /// end is exclusive. Will be calculated from other information if needed.
    pub fn loop_range_samples(&self) -> Option<(i64, i64)> {
        match (
            self.loop_range_samples,
            self.loop_range_seconds,
            self.loop_range_beats,
            self.tempo,
        ) {
            (Some(loop_range_samples), _, _, _) => Some(loop_range_samples),
            (_, Some((start_seconds, end_seconds)), _, _) => Some((
                ((start_seconds * self.sample_rate as f64).round() as i64),
                ((end_seconds * self.sample_rate as f64).round() as i64),
            )),
            (_, _, Some((start_beats, end_beats)), Some(tempo)) => Some((
                (start_beats / tempo * 60.0 * self.sample_rate as f64).round() as i64,
                (end_beats / tempo * 60.0 * self.sample_rate as f64).round() as i64,
            )),
            (_, _, _, _) => None,
        }
    }

    /// The loop range in seconds, if the loop is active and this information is available. None of
    /// the plugin API docs mention whether this is exclusive or inclusive, but just assume that the
    /// end is exclusive. Will be calculated from other information if needed.
    pub fn loop_range_seconds(&self) -> Option<(f64, f64)> {
        match (
            self.loop_range_samples,
            self.loop_range_seconds,
            self.loop_range_beats,
            self.tempo,
        ) {
            (_, Some(loop_range_seconds), _, _) => Some(loop_range_seconds),
            (Some((start_samples, end_samples)), _, _, _) => Some((
                start_samples as f64 / self.sample_rate as f64,
                end_samples as f64 / self.sample_rate as f64,
            )),
            (_, _, Some((start_beats, end_beats)), Some(tempo)) => {
                Some((start_beats / tempo * 60.0, end_beats / tempo * 60.0))
            }
            (_, _, _, _) => None,
        }
    }

    /// The loop range in quarter notes, if the loop is active and this information is available.
    /// None of the plugin API docs mention whether this is exclusive or inclusive, but just assume
    /// that the end is exclusive. Will be calculated from other information if needed.
    pub fn loop_range_beats(&self) -> Option<(f64, f64)> {
        match (
            self.loop_range_samples,
            self.loop_range_seconds,
            self.loop_range_beats,
            self.tempo,
        ) {
            (_, _, Some(loop_range_beats), _) => Some(loop_range_beats),
            (_, Some((start_seconds, end_seconds)), _, Some(tempo)) => {
                Some((start_seconds / 60.0 * tempo, end_seconds / 60.0 * tempo))
            }
            (Some((start_samples, end_samples)), _, _, Some(tempo)) => Some((
                start_samples as f64 / self.sample_rate as f64 / 60.0 * tempo,
                end_samples as f64 / self.sample_rate as f64 / 60.0 * tempo,
            )),
            (_, _, _, _) => None,
        }
    }
}

/// A [`ProcessContext`] with no host, for a plugin's own tests: the events are the ones pushed,
/// in order; the transport is stopped and the processing mode fixed; a change the plugin makes
/// itself ([`ProcessContext::set_parameter_normalized`]) is set as a wrapper sets it (the value
/// and its smoother) and kept in [`reported`][Self::reported], as a host would be told it; and
/// [`automate()`][Self::automate()] sets a parameter as a host's automation is set between the
/// runs a wrapper splits a block into. Only the parameters of the [`Params`] it is made with are
/// touched.
///
/// Not upstream's: the CA-72's `third_party/nih-plug/PATCHES.md`, change 11.
pub struct TestProcessContext<P: Plugin> {
    /// Keeps the parameters `known` points into alive.
    _params: std::sync::Arc<dyn crate::params::Params>,
    known: Vec<ParamPtr>,
    events: VecDeque<PluginNoteEvent<P>>,
    transport: Transport,
    process_mode: ProcessMode,
    /// The changes the plugin made itself, as the host would be told them: the parameter, its
    /// normalized value as set, and when in the block.
    pub reported: Vec<(ParamPtr, f32, u32)>,
}

impl<P: Plugin> std::fmt::Debug for TestProcessContext<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestProcessContext")
            .field("events", &self.events.len())
            .field("reported", &self.reported)
            .finish()
    }
}

impl<P: Plugin> TestProcessContext<P> {
    /// A context for a plugin whose parameters are `params`, at `sample_rate`.
    pub fn new(
        params: std::sync::Arc<dyn crate::params::Params>,
        sample_rate: f32,
        process_mode: ProcessMode,
    ) -> Self {
        let known = params.param_map().into_iter().map(|(_, p, _)| p).collect();
        Self {
            _params: params,
            known,
            events: VecDeque::new(),
            transport: Transport::new(sample_rate),
            process_mode,
            reported: Vec::new(),
        }
    }

    /// An event for the next `process()` call (or the rest of this one), after those pushed.
    pub fn push_event(&mut self, event: PluginNoteEvent<P>) {
        self.events.push_back(event);
    }

    /// Sets `param` to `normalized` as a host's automation is set: between `process()` calls, as
    /// a wrapper does at an automation point it has split the block at. Returns whether `param`
    /// is one of the plugin's.
    pub fn automate(&self, param: ParamPtr, normalized: f32) -> bool {
        if !self.known.contains(&param) {
            return false;
        }
        // SAFETY: `param` is one of `_params`' own, which this context keeps alive.
        unsafe {
            param.set_normalized_value(normalized);
            param.update_smoother(self.transport.sample_rate, false);
        }
        true
    }
}

impl<P: Plugin> ProcessContext<P> for TestProcessContext<P> {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }

    fn execute_background(&self, _task: P::BackgroundTask) {}

    fn execute_gui(&self, _task: P::BackgroundTask) {}

    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn process_mode(&self) -> ProcessMode {
        self.process_mode
    }

    fn next_event(&mut self) -> Option<PluginNoteEvent<P>> {
        self.events.pop_front()
    }

    fn send_event(&mut self, _event: PluginNoteEvent<P>) {}

    fn set_latency_samples(&self, _samples: u32) {}

    fn set_current_voice_capacity(&self, _capacity: u32) {}

    fn set_parameter_normalized(&mut self, param: ParamPtr, normalized: f32, timing: u32) -> bool {
        if !self.automate(param, normalized) {
            return false;
        }
        // SAFETY: as in `automate()`.
        let value = unsafe { param.unmodulated_normalized_value() };
        self.reported.push((param, value, timing));
        true
    }
}
