# Standalone host services

`plugin-kit-standalone` provides audio, MIDI, device settings and session storage for
an instrument's own desktop application. The first adopter is the CA-72. Its
`docs/standalone-spec.md` holds the product and acceptance contract. This crate does
not create the editor or change the circuit: the instrument supplies an
`AudioProcessor` and a shared `SessionState` adapter.

## Integration

1. Implement `AudioProcessor::prepare` and `deactivate` on the control thread, and
   `process(AudioBlock)` on the audio thread. Processing receives planar stereo
   output, sanitized optional external input and bounded MIDI messages. Prepare
   allocates all buffers and handles a new rate/maximum processing block.
2. Implement `SessionState` using shared parameter/editor state. `save` must not
   touch callback-owned DSP. `restore` runs with all streams destroyed. Exclude
   held notes, transient wheel positions and active gestures.
3. Call `spawn`, install `AppHandle::install_current(&runtime.handle())`, and call
   `runtime.wait_ready(Duration::from_secs(2))` before asking the editor for its
   persisted size. Cold DSP preparation, device discovery and opening follow on
   the control thread; readiness guarantees restored shared state, not warm DSP.
4. Only a standalone editor context should read `AppHandle::current()`. Settings
   read `snapshot()` and send `apply`, `refresh`, `save`, `restore_state` or `stop`.
   The audio callback never acquires the snapshot mutex.
5. Send local playing events with `MidiEvent::source = 0` and call `panic()` for an
   emergency release. Hardware inputs receive independent identities 1–64. The
   router merges devices within each channel/pitch and preserves the first emitted
   source identity in the final release. MIDI channels remain separate. MIDI Learn
   sees physical CC channels unchanged. SysEx, system/clock messages and MIDI
   output are outside this instrument host's scope.
   `AudioProcessor::midi_reset_scope()` defaults to `MidiResetScope::Channel`,
   translating CC120/123 into source/channel note releases. An omni instrument
   such as CA-72 explicitly chooses `Global` to preserve its global controllers.
   Source-specific mode releases owned notes; it does not implement an instrument's
   source-specific release-tail choke. Device unplug uses separate source releases
   regardless of controller scope and invalidates its queued event generations.
6. Close the editor and call `runtime.shutdown()`. The caller waits at most two
   seconds and receives an error if the OS driver does not finish or its final
   session save fails. Do not silently
   discard that result in the application.

## Ownership and callback work

The output callback owns `Box<dyn AudioProcessor>` exclusively. Destroying its
stream destroys the callback owner, which returns the Box through a bounded
channel. This also works when no callback has ever arrived or building/starting
a stream fails. There is no shared mutable DSP cell, processor mutex or spinning
audio lock. Processing splits unexpectedly large device callbacks into the
prepared maximum block without growing any buffer.

Input frames and MIDI use `plugin-kit-queue`'s packed atomic slots. Producers try
one gate acquisition, store the complete packet, then publish tail. Consumers try
one head advance; they never spin while a preempted producer has not published.
Contended MIDI producers fail closed and request emergency release. Processing drains
at most 256 MIDI messages per block. Queue exhaustion requests a global emergency
release and discards at most the 2048-message capacity before further processing.
Source generations advance before an emergency release so a preempted producer
cannot publish a stale note afterward. This avoids stranded notes when a note-off was dropped. Callback errors set
atomics only; device work and error display happen through control/UI snapshots.

Selection uses hardware channel indices. Mono input duplicates one channel;
stereo input takes two selected channels. Non-finite input becomes silence. Mono
output is `(L+R)/2`; every unused hardware output stays zero. Floats from a broken
processor are sanitized before device sample conversion.

Capture and playback may be separate endpoints at a common nominal rate, including
Windows' capture/render devices. A 65536-frame capture reservoir primes two device
periods before input is audible. Consumption adapts to its fill using linear
interpolation, bounded to a 1% rate correction. Equal clocks copy samples exactly.
This adds capture buffering latency; buffer duration is not a roundtrip estimate.
Capture starts after output-stream construction. Startup/overflow backlog is
discarded in bounded batches: at most `min(4 × callback frames, 4096)` during
priming plus one frame per silent output frame. Recovery plays recent capture
instead of reading old audio while slowly correcting a large backlog.

## Configuration, sessions and recovery

Capability negotiation rejects impossible channels/rates/buffer limits before
stopping a working stream. Opening remains the final capability check. Failed
opens return the processor, then try the previous successful settings. Missing
rollback hardware leaves an editable stopped instance retaining the processor.
Input opening/permission failures try the output configuration with external
input disabled and report that effective configuration. Missing MIDI ports are
warnings; other ports and audio remain usable.

Device inventories refresh every two seconds, on workers the control thread
never waits on: one lists audio devices, one MIDI ports, and each publishes on
its own. A device that keeps the audio worker waiting longer than 1.5 seconds is
left out and named in `Devices::unresponsive` and the status ("Steam Streaming
Speakers isn't responding."); the round starts again on a new worker, which skips
that device until its call returns, so a blocked device holds one thread however
long it blocks. Settings are first applied once both inventories have answered.
Listing reads capabilities only: on macOS, CoreAudio properties (stream
configuration, nominal and available sample rates, buffer frame size range), with
no AudioUnit or IO proc, so it never asks for microphone access. That is asked
only when an external input is chosen and opened. On macOS the MIDI worker runs
its run loop for an instant before each poll: CoreMIDI announces new and removed
endpoints there, and without it a controller plugged in after launch never
appeared. Selected identities must match one
device exactly; absent or duplicate identities cannot silently select a different
interface. Device loss releases notes, tears down the streams and keeps the
editor alive. A successful inventory poll retries a stopped configuration even
when the reconnected device has exactly the same identity. Failed enumeration
also tears down an active stream: an ALSA server may disappear without CPAL
issuing its stream error callback.
MIDI inventory changes retain surviving connections and release only lost sources.
Processor faults submitted through `stop` require explicit Apply before retry.

The versioned `session.json` contains the app identity, last successful settings
and adapter state. Saves use a same-directory temporary file, sync and atomic
replacement; Unix also syncs the directory. Saves occur after accepted changes,
on request, every five seconds and at close. Malformed, unsupported or mismatched
files remain untouched; saving stays disabled with a warning until the user moves
that file aside. A failed candidate is never saved.
The original file's fingerprint is checked again before saving. Moving it aside
while the app runs resumes saves; a replacement file is preserved. Save requests
coalesce behind a 250 ms gate. A `File::try_lock` on `instance.lock` makes one app
instance own each state directory; another instance is refused before loading or
writing that session. Different state directories remain independent.
Control-thread save calls are serialized within the process so Windows cannot
reject a competing atomic replacement while the preceding writer still holds
the destination open.

## Support limits to retain in product documentation

- CPAL 0.15.3 is retained to preserve the existing macOS baseline. Backends are
  the native default: CoreAudio, WASAPI and ALSA. ASIO and a production JACK
  routing backend are not implemented here. ALSA may expose system server bridge
  devices. Native PipeWire is not claimed.
- CPAL 0.15 does not expose stable OS device identifiers. Identity consists of
  name and input/output channel capabilities, and only unique matches reconnect.
  Two identical same-name devices are deliberately ambiguous.
- The capture reservoir was verified with simulated clocks differing by ±500 ppm
  for 60 seconds at 48 kHz, with 128- and 256-frame callbacks, without overflow or
  underflow after priming. Driver scheduling bursts, the frequency response of
  linear interpolation and real capture latency still require hardware checks.
- A callback period is reported as buffer duration, not measured end-to-end
  latency. The `xruns` snapshot member counts reported stream errors; CPAL 0.15
  does not expose a reliable cross-platform underrun counter.
- OS driver calls for enumeration, stream construction/destruction and MIDI
  disconnect cannot be forcibly interrupted safely. Enumeration is bounded by
  leaving a blocked worker behind (at most eight at once); opening the chosen
  device still happens on the control thread. The caller's shutdown wait
  is bounded, but a malfunctioning driver may leave a detached control thread
  until the OS operation returns. Normal shutdown does not wait for a callback.
- Hardware playback, input, permission, hotplug and driver behavior still need
  installed-app verification on each OS. Deterministic tests are not that proof.

## Evidence

On Linux, without opening audio or MIDI hardware:

```
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c 3 cargo test -p plugin-kit-queue -p plugin-kit-standalone
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c 3 cargo clippy -p plugin-kit-queue -p plugin-kit-standalone --all-targets -- -D warnings
cargo fmt -p plugin-kit-queue -p plugin-kit-standalone -- --check
```

Twenty-nine standalone tests and two queue tests cover identity ambiguity/missing devices, capability negotiation,
mono/multichannel routing, NaN/Inf input, overlapping source/channel ownership,
velocity-zero note-offs, MIDI channel filtering, concurrent producers, overflow
recovery, malformed/unsupported session preservation, interrupted/competing atomic
writes, failed apply/rollback/recovery, input denial preserving output, startup
readiness, no-device shutdown and independent capture clocks. Variable-callback
tests count allocations on the calling thread with MIDI traffic queued: the
callback allocates zero times.
Review regressions cover controller scope, accepted source identity, bounded large
source releases, stale packets from preempted producers, moved/replaced bad session
files, final-save errors, instance ownership, coalesced saves, range-specific sample
format selection and recent-input recovery after a one-second capture backlog.

### Isolated Linux callbacks

`python scripts/standalone-virtual/verify.py` is an explicit integration fixture,
requiring installed PipeWire, PipeWire ALSA, ALSA development files and Python3.
It builds only the probe with four low-priority jobs into its own temporary
target directory. It starts a private server socket/config and contains no udev,
hardware/device factory or session manager. `ALSA_CONFIG_PATH` redirects both
the default PCM and hardware control name hints to that private server. MIDI
sequencer access is absent from the private ALSA configuration. The graph has
only a dummy driver, null sink and virtual null source; the fixture inspects the
graph and rejects device objects/hardware properties. Known left/right playback
is linked through the sink monitor and source into capture. Cleanup signals only
child processes it created; logs/config/evidence remain in `/tmp`.

The integration run verifies live playback, correctly ordered nonzero stereo
capture, Apply at 48kHz/128 frames, 44.1kHz/256 frames and 48kHz/256 frames,
private-server disconnection and automatic recovery, preservation of the same
processor, and close under two seconds with exactly one processor Drop. The
2026-10-13 run recovered without Apply, allocated the processor once, and closed
in 1 ms or less. Requested periods remain preferences: this PipeWire/ALSA graph
delivered 64 frames at 48 kHz and 117/118 frames at 44.1 kHz; the snapshot reports those
observed sizes. This tests actual Linux backend callbacks without claiming
physical interface latency, hardware hotplug, microphone permission behavior,
or Windows/macOS driver verification.

The isolation follows primary documentation for [PipeWire configuration](https://docs.pipewire.org/page_man_pipewire_conf_5.html),
[runtime/remote selection](https://pipewire.pages.freedesktop.org/pipewire/page_man_pipewire_1.html),
[the PipeWire ALSA PCM](https://github.com/PipeWire/pipewire/blob/master/pipewire-alsa/conf/50-pipewire.conf),
and [ALSA_CONFIG_PATH](https://www.alsa-project.org/wiki/LibEnvVars).
