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

16. **A shipped standalone adapter shares the exact instrument and editor.**
    `standalone-production` exports `wrapper::production`, independently of the
    development launcher's JACK dependency. The exclusively owned processor is
    moved into `plugin-kit-standalone` streams and reclaimed on their destruction.
    Shared parameter values can be edited while stopped; smoothers are changed
    only by the processor. Sessions validate migrated copies before restoring
    the original once; enum validation adds `EnumParamInner::has_id`. Optional
    auxiliary input uses the normal plugin path. Callback task mailboxes are
    preallocated SPSC queues and require `Copy` payloads, so exhaustion cannot
    invoke a task destructor. The single task executor is shared only on control
    threads. An omni instrument explicitly opts into global controller resets.

    Typed session preflight adds `PersistentField::validate` and derived
    `Params::validate_fields`, including nested/prefixed/array fields and Arc
    forwarding. Validation follows migration and precedes every state setter;
    wrong-type fields cannot partially mutate a standalone session.
    `ProcessContext::take_standalone_panic` defaults to false in hosted contexts.
    The production context offers one boundary reset to opting-in instruments;
    other instruments keep synthetic CC120/121. CA-72 coalesces that boundary
    with its UI panic so fresh local notes survive either callback ordering.

17. **A host's answer to a resize reaches the editor, however late.** VST3's
    `IPlugFrame::resizeView()` can refuse, and on Linux the wrapper queues the request onto
    the host's `IRunLoop`, so `GuiContext::request_resize()` returned `true` before the host
    had answered, and a later refusal left the host's frame and the editor at different
    sizes. The frame now captures the editor's request id and size
    (`Editor::resize_request()`, by default `(0, size())`) before calling the host, and reports
    the host's answer through `Editor::resize_result()` on the host's GUI thread, including a
    request queued onto the run loop (`src/wrapper/vst3/view.rs`; `inner.rs` no longer asserts
    on a refusal, a valid answer). `GuiContext::resize_response_will_follow()` says whether the
    wrapper sends these answers (VST3's: `true`; others `false`), and VST3's `request_resize()`
    returns whether the request was queued, so a request it could not queue is answered `false`
    there and then (`src/wrapper/vst3/context.rs`). An editor that overrides
    neither hook behaves as before. The CA-72's change 14 (its `docs/decisions.md` R72).
    `crates/plugin-kit-learn/tests/vst3_host.rs` answers the view's request at once both ways
    and, on Linux, delivers a refusal through the actual run-loop socket.

From the CA-72's 0.2.6 to 0.2.8 (2026-10-18 and 19; the kit's `docs/decisions.md`
K-CA72-0.2.8), its changes 15 and 16, here 18 and 19:

18. **A remote wrapper can opt out of native-window occlusion** (the CA-72's
    R-AU-REMOTE-VISIBILITY). The additive `GuiContext::native_window_occlusion_reliable()`
    defaults to true. CLAP queries the private presence-only host capability
    `com.idlefoundry.native-window-occlusion-unreliable/1` once during initialization,
    caches the answer per instance, and exposes it through the GUI context. The AUv2
    wrapper advertises this capability (`../clap-wrapper/PATCHES.md`, change 7) because a
    remotely presented editor's local NSWindow need not describe its visible proxy.
    `PluginApi` remains `Clap`; ordinary CLAP hosts, VST3 and both standalone adapters
    retain the default. An editor that skips presenting while its window is occluded asks
    this first and keeps presenting where it is false. `src/wrapper/clap/visibility_tests.rs`
    (the CA-72's) runs the actual CLAP initialization and context against a host with and
    without the marker (`cargo test --manifest-path third_party/nih-plug/Cargo.toml --lib
    visibility_tests`, in the kit's CI); with the marker ignored, two of its three fail.

19. **A VST3 host's brightness arrives as brightness** (the CA-72's R-MPE, its issue 81,
    `af0b1f3`). `NoteExpressionController::translate_event`
    (`src/wrapper/vst3/note_expressions.rs`) turned `kBrightnessTypeID` into
    `NoteEvent::PolyExpression` and `kExpressionTypeID` into `PolyBrightness`, the opposite of
    its own `translate_event_reverse` and of the CLAP wrapper, so a VST3 host's MPE timbre
    (brightness) reached a plugin as expression. They now map each to its own. The module's
    own test, `brightness_and_expression_each_map_to_their_own` (`cargo test --manifest-path
    third_party/nih-plug/Cargo.toml --features vst3 --lib note_expressions`, in the kit's CI),
    fails with the two mapped as before.

Three more from the CA-72's 0.2.2 to 0.2.8 that changed the production standalone (16) without
a number of their own there (`docs/decisions.md` K-CA72-0.2.8):

20. **A standalone panic's token reaches the instrument with its block.** The kit's
    `plugin-kit-standalone` captures a tagged PANIC's token with the block's panic
    (`AudioProcessor::panic_token`, K-PANIC-TOKEN), but this adapter dropped it, so an
    instrument could not tell which of its editor's PANIC requests a block's reset answered.
    The adapter now keeps the token given for the next block and offers it, with that block's
    panic only, through the additive `ProcessContext::standalone_panic_token()` (None in every
    other context); it is taken at each block, a rejected one too, so it never reaches a later
    block. An instrument reads it before `take_standalone_panic()`. The CA-72's e43d39c
    (2026-10-14), which it tests through its own instrument;
    `src/wrapper/standalone/production_tests.rs` here drives the adapter as the runtime does
    (`--features standalone-production --lib production`), and fails without the token passed.

21. **The standalones' windows: the kit's baseview, and on Windows one scale for the window
    and its editor** (the CA-72's R-WIN-STANDALONE, its issue 64, `8109b28`, 2026-10-18).
    Both standalone adapters opened their window with upstream's baseview at `579130e` from
    git, with `opengl`, so an application's window had none of `../baseview`'s changes (its
    change 9: no frame to drag and no maximise box), and a plug-in built two baseviews. They
    now take `../baseview` by path, without `opengl` (`gl_config` is gone from both), the same
    copy a plug-in's editor opens in. And on Windows baseview made the process aware of each
    monitor's scale only once its first window was open, and scaled that window by its
    monitor's scale while the editor in it, told no scale, drew at 1: at 125 % and more the
    window opened a quarter or more larger than its editor, black beyond it. Now
    `app_window::scale()` makes the process aware of each monitor's scale and reads the
    primary monitor's (where the window opens) before anything is measured;
    `app_window::options()` tells the editor that scale before first asking its size and opens
    the window at it (else at the system's, as before); and a resize from outside (Windows
    scales a window whose monitor's scale changes) is undone at the next frame on Windows
    (`refits`, `fit`). macOS and Linux keep the system's scale policy. The manifest takes
    `windows`' `Win32_UI_HiDpi`. `src/wrapper/standalone/production_tests.rs`:
    `the_apps_window_opens_at_the_scale_its_editor_was_told_first` and
    `a_resize_refits_the_apps_window_on_windows_alone`; the CA-72's test of its own window at
    five scales stays with it.

22. **Every way of quitting the production standalone closes its window** (the CA-72's
    R-QUIT, `45d3104`, 2026-10-19), as its close button does, so that
    `run_with_midi_reset_scope` returns and shuts the app down as then (its session saved,
    audio and MIDI stopped), and its caller's own way out follows. On macOS the menus and the
    quit Apple event are baseview's change 11; on Windows Alt+F4 is the system's, which
    baseview leaves to `DefWindowProcW`. On Linux `app_window::quits` is Ctrl+Q: the app's
    window closes at it (`window.close()`, as `WM_DELETE_WINDOW` closes it), and an editor that
    has the keys (X11 gives them to the window under the pointer) asks for it with
    `app_window::request_quit`, which the window acts on at its next frame. The manifest takes
    `keyboard-types` for the key, as baseview gives it, with the `standalone-production`
    feature. `src/wrapper/standalone/production_tests.rs`:
    `ctrl_q_quits_on_linux_and_no_other_key_does` (the CA-72's) and
    `a_request_to_quit_is_seen_once`. The CA-72's tests that its editor hands Ctrl+Q over
    before typing has it stay with it.

23. **Offline work runs in the live wrappers' floating point mode** (the CA-72's "Detached
    offline rendering", its R-REFERENCE-AUDIO, 2026-10-21). `util::with_flush_to_zero` runs a
    closure under the wrappers' own scoped flush-to-zero guard (`wrapper::util::ScopedFtz`,
    which `process_wrapper` takes), restoring the calling thread's mode afterwards, so that a
    plug-in's render made with no host (an agent's, a test's) gives the samples a host's
    `process` call would. It leaves out `process_wrapper`'s `assert_no_alloc`: an offline
    renderer allocates its output. The live wrappers' processing is unchanged. `src/util.rs`:
    `offline_work_is_flushed_to_zero_and_the_threads_mode_put_back` (a subnormal result flushed
    inside, nested too, and the thread's mode as it was outside).

To move to a newer upstream commit, copy its `Cargo.toml`, `LICENSE`, `README.md`, `src`
and `nih_plug_derive` here and apply the twenty-three changes again, unless upstream has fixed them.
Then update the commit above, and `nih_plug_xtask`'s `rev` in each plug-in's workspace `Cargo.toml`.
