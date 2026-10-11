# nih-plug, patched

The `nih_plug` crate and its derive macros from
[robbert-vdh/nih-plug](https://github.com/robbert-vdh/nih-plug) at commit
`de421011f41a6d10fc8c7a6084e4f4dee0143683` (ISC, `LICENSE`; its VST3 bindings are
GPL-3.0). The manifest's `[workspace]` and `[profile]` tables are removed, and two of
upstream's own compiler warnings are allowed. The examples, the GUI adapters and the other
plug-ins are left out.

The commit is the project's latest. It fails four of clap-validator 0.4.1's tests, and so
do its own example plug-ins (`sine`, `gain`). These two changes, both in
`src/wrapper/clap/wrapper.rs`, fix them:

1. **A corrupt state no longer aborts the host.** `ext_state_load` reserved the length
   the stream's first eight bytes claim before reading any of it. With random bytes that
   is about 10^18 bytes, and the failed allocation aborts the process (clap-validator's
   `state-invalid-random`). The buffer now grows as the data arrives, 64 KiB at a time,
   so a short or corrupt stream fails the load instead.
2. **The host is told the values changed after a state load.** `set_state_inner` now also
   asks the host to rescan the parameters' values (`CLAP_PARAM_RESCAN_VALUES`). Without
   that, a host may keep showing the values from before the load
   (`state-reproducibility-basic`, `-binary` and `-buffered`).

A third change, in `src/wrapper/vst3/wrapper.rs`, comes from Steinberg's VST3 validator
(SDK 3.8.1):

3. **A bus arrangement is matched against the right buses.** `set_bus_arrangements`
   took a layout's first auxiliary bus to be the host's bus 1 when the layout has no main
   bus, and bus 0 when it has one: the wrong way round (the rest of the file has it
   right). For an instrument with a side chain it read past the end of the host's
   one-element array, and so refused every arrangement the host asked for (the
   validator's mono test: "Mono Input-SpeakerArrangement is not supported").

Five more came from a review of the plug-in before its release (2026-10-03; the CA-72's
`docs/decisions.md` R18):

4. **A VST3 host's refusal to resize reaches the editor.** `request_resize` always
   returned `true`, whatever the host answered, so an editor could not tell that its
   window had not grown (the CA-72's presets' drawer, opening below the panel, went
   unseen while it held the keyboard). On the GUI thread, where the host answers at once
   (macOS, Windows), it now returns the host's answer; elsewhere (Linux, where baseview's
   window has a thread of its own) it still schedules the request and returns `true`.
   `WrapperView::request_resize` (`src/wrapper/vst3/view.rs`) no longer asserts that the
   host agreed, which stopped a debug build at a refusal.
5. **Auxiliary buses are bounded by the host's own count.** In `process`, the VST3 wrapper
   bounded the auxiliary input buses by the host's number of output buses, and both
   wrappers compared with `>`, so a bus at the host's count was read one past the end of its
   array (`src/wrapper/vst3/wrapper.rs`: inputs by `num_inputs`, both with `>=`;
   `src/wrapper/clap/wrapper.rs`: both with `>=`).
6. **A side chain's missing channels are as long as the block.** Channels the host did not
   supply (too few of them, or no pointers) were zeroed at whatever length an earlier block
   left them, which may be shorter than this block's, and a plug-in reading them by the
   block's samples panicked. They are now resized to the block first, within the room
   reserved for them (`src/wrapper/util/buffer_management.rs`).
7. **VST3's buffer configuration carries the processing mode just set.**
   `setup_processing` stored it with the mode from before the call, so `initialize` saw the
   previous mode: realtime for an offline render, offline for playback after one
   (`src/wrapper/vst3/wrapper.rs`).
8. **The processing mode in `process`.** `ProcessContext::process_mode()`
   (`src/context/process.rs`; the VST3, CLAP and standalone contexts). CLAP's `render`
   extension may change the mode while the plug-in is active, and the plug-in is not
   initialized again for it, so `BufferConfig::process_mode` alone may be out of date.

One more came from a report: the CA-72 crashed in Sandyne, a DAW built with JUCE (2026-10-06;
the CA-72's `docs/decisions.md` R31):

9. **A channel the host gives as a null pointer is not read.** A VST3 host may give a null
   pointer for a channel, and JUCE's hosts do for every channel of a bus they deactivated,
   with the bus's full channel count (`HostBufferMapper::associateBufferTo`); setting up an
   instrument with no inputs deactivates its side chain. `create_buffers` read every input
   channel through its pointer, so an instrument with a side chain read from address 0 in
   its first block, an access violation. A null input channel is now read as silence, and a
   null output channel is backed by scratch storage, allocated with the rest and discarded,
   so that every channel the plug-in sees is as long as the block
   (`src/wrapper/util/buffer_management.rs`, with a test of each kind of channel).

Two more came with the CA-72's MIDI Learn (2026-10-06; `docs/decisions.md` R34), which sets a
parameter from the audio thread when a learned MIDI controller moves. Upstream left that as a
`TODO` in `ProcessContext` (`set_parameter`), and the plugin must not use the editor's
`ParamSetter` there (its host calls belong on the GUI thread) nor keep a value of its own that the
host, the editor and the saved state would not see:

10. **`ProcessContext::set_parameter_normalized(param, normalized, timing)`**
    (`src/context/process.rs`). The value is set at once, as each wrapper already sets the
    host's automation during a process call (its smoother updated, the editor told), so the
    plugin reads it from there on; and the host is told as its format has it:
    - **CLAP:** a `CLAP_EVENT_PARAM_VALUE` output event at the change's time, flagged
      `CLAP_EVENT_DONT_RECORD`, no gesture: `clap/ext/params.h`'s "Turning a knob via plugin's
      internal MIDI mapping", where recording both the MIDI and the parameter would conflict
      (`src/wrapper/clap/context.rs`; `Wrapper::set_own_parameter` and `handle_out_events` in
      `src/wrapper/clap/wrapper.rs`, after the editor's changes, in time order). Nothing more:
      Bitwig Studio 5.2.7 does not apply such an event to its own display (without the flag it
      did, and made an undo step of each), but a values rescan after each block was not the
      cure, since clap-wrapper's Audio Unit takes any rescan for a new parameter list.
    - **VST3:** a point at the change's sample offset in the process call's
      `outputParameterChanges`, the processor's way to tell the host and its controller of a
      change (`src/wrapper/vst3/context.rs`, `inner.rs`; drained after each run in
      `src/wrapper/vst3/wrapper.rs`). VST3 has no flag against recording: Steinberg notes that
      some hosts write automation for these, and Ableton Live 12.2.7 does while its automation
      is armed.
    - **The standalone:** set at once; no host to tell (`src/wrapper/standalone/`).

    Each wrapper reserves room for 1024 such changes a process call when it is made
    (`OWN_PARAM_CHANGES`, `src/wrapper/util.rs`), so nothing is allocated, locked or waited for
    on the audio thread; past it a change is set but not told, and the call returns `false`.
    And **at one sample, the host's parameter changes come before the events the plugin reads**,
    whatever the host's order, so that a controller setting a parameter follows the automation of
    it at that sample rather than winning or losing by chance: in VST3 the process call's events
    are sorted by (time, parameter change first) (MIDI CCs arrive as parameter changes there too,
    in the host's queue order); in CLAP, `handle_in_events_until` splits the run before an event
    that a parameter change follows at its sample (`split_before`).
11. **`TestProcessContext`** (`src/context/process.rs`): a `ProcessContext` with no host, for a
    plugin's own tests: the events pushed, a change made through change 10 set as a wrapper sets
    it and kept as a host would be told it, and `automate()` setting a parameter as a host's
    automation is set between runs. It touches only the parameters of the `Params` it is made
    with. (A plugin cannot set a parameter outside nih-plug otherwise: its setters are
    crate-private.)

Two more came from clap-validator's `param-fuzz-modulation` on Windows, which crashed the
MC-79 (an access violation, sometimes a fail-fast abort) and never the CA-72 (the MC-79's
`docs/decisions.md` R13):

12. **No background task holds an instance the host has destroyed.** The background thread
    upgrades its weak reference to the instance for as long as a task runs; the CLAP
    wrapper's `destroy` (and the VST3 wrapper's `Drop`) dropped its own reference anyway, so
    the instance outlived it, and was dropped on the background thread once the task
    returned, after the host had unloaded the library: on Windows, where unloading unmaps
    it, the thread ran on into unmapped code. Each instance's tasks now pass a gate
    (`src/event_loop/background_thread.rs`): as the instance goes it closes the gate, after
    which none of its tasks runs or takes the instance, and waits (up to ten seconds) for one
    already running (`close`, `EventLoop::close_background`). It waits only for that, not
    for every other holder: a VST3 host releasing the component before the editor's view
    (which holds the instance too) is not held up.
13. **A task for an instance already gone does not stop the shared background thread.** The
    thread returned on such a task, though every instance in the process shares it; the next
    instance to go then failed to send it `Shutdown` and panicked in its drop, inside the
    host's call (`src/event_loop/background_thread.rs`). It now skips the task.

One more came from a report: the CA-72's switches and buttons did nothing in Cubase 15 on
Windows 11 (2026-10-07; the CA-72's `docs/decisions.md` R36, its change 12; K4 here):

14. **An edit made in the editor while the host processes audio is held, not left to the
    host.** While the host was processing, `raw_set_parameter_normalized` left the value
    alone, for the host to send the edit back to the processor, so that a value never changes
    in the middle of a process call; until then `IEditController::getParamNormalized()`
    reported the value from before the edit. Cubase 15 reads that value within `endEdit()` and
    sends it to the processor at the next process call, after the edit. So a click, a whole
    gesture between two process calls, set nothing, and a drag let go between two calls ended
    one move short. A host that does not send the edit back at all (upstream's `FIXME` names
    REAPER, which stops calling a plugin it bypasses) left it unset. The edit is now held, the
    latest for each parameter (`held_edits`, `src/wrapper/vst3/inner.rs`): the controller
    reports it, the next process call sets it at its start, before the host's own changes at
    its first sample, and `setProcessing(false)` sets it if no call will
    (`src/wrapper/vst3/context.rs`, `wrapper.rs`). Values still change only between process
    calls, and nothing is allocated, locked or waited for on the audio thread: a flag, and an
    atomic for each parameter. `crates/plugin-kit-learn/tests/vst3_host.rs` drives the wrapper
    through its factory as Cubase 15 does; four of its six tests fail without this.
15. **The keys a host gives a VST3 plugin's view reach its editor.** Cubase keeps the keyboard
    from a plugin's own window and gives the plugin its keys through `IPlugView::onKeyDown()`
    and `onKeyUp()`, using those the plugin does not take as its own key commands (Escape closes
    the plugin's window). The wrapper answered `kNotImplemented`, so no key ever reached the
    editor, and no typed field (a search, a name, a value typed into a readout) worked in
    Cubase. They are now offered to it (`Editor::on_host_key()`, `HostKey`: VST3's character,
    `VirtualKeyCodes` code, `KeyModifier` flags, down or up; `src/editor.rs`), and the host is
    told `kResultTrue` if the editor took the key, else `kResultFalse`
    (`src/wrapper/vst3/view.rs`). An editor that does not override it takes none, as before.
    The CA-72's change 13 (its `docs/decisions.md` R-KEYS). `crates/plugin-kit-learn/tests/
    vst3_host.rs` gives the view keys as Cubase does.

To move to a newer upstream commit, copy its `Cargo.toml`, `LICENSE`, `README.md`, `src`
and `nih_plug_derive` here and apply the fifteen changes again, unless upstream has fixed them.
Then update the commit above, and `nih_plug_xtask`'s `rev` in each plug-in's workspace `Cargo.toml`.
