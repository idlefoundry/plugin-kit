# CPAL 0.15.3: CoreAudio lifecycle backports

The crates.io 0.15.3 source corresponds to RustAudio/cpal
`ac6cbb2ba55e61665a35ab88ae136a83380d1354`, Apache-2.0 (LICENSE included).
The package manifest is crates.io's normalized manifest; examples and source are
included. Optional ASIO is not enabled; no ASIO SDK is bundled or used.

Two upstream corrections are backported to preserve the existing API and macOS 11
floor, and one change of our own (3) is made. No other backend, API or audio-format behavior is deliberately changed.

1. [02b454c, fix callback being called after dropping the stream](https://github.com/RustAudio/cpal/commit/02b454c02cc42910192739db03de7b384f50b9cf).
   The CoreAudio disconnection closure captures a Weak reference to StreamInner,
   instead of a strong Stream clone owned by that same inner object. Internal
   play/pause helpers follow upstream. Dropping an enumerated stream can therefore
   destroy its AudioUnit and its callback, returning the standalone processor.
2. [51c3b43, fix macOS non-default audio output](https://github.com/RustAudio/cpal/commit/51c3b43c54591203fe3edc9a31ff7595650f7103).
   Both supported/default configuration probes construct an input AudioUnit only
   for input scope; output probes construct an output AudioUnit.

3. Idle Foundry's own change (2026-10-19, CA-72 issue 112): the supported and
   default configuration probes read the device's
   `kAudioDevicePropertyBufferFrameSizeRange` as a property, instead of making a
   HAL AudioUnit and setting its `CurrentDevice` to read the same range. Setting
   the device creates an IO proc on it: for an input probe, that is what asked
   for microphone access at every launch, and for five virtual drivers on the
   owner's Mac (Steam Streaming, Virtual Desktop, Immersed) the call never
   returned, so listing never finished. Listing now reads properties only; only
   building a stream makes an AudioUnit. `cpal::coreaudio_audio_units_made()`
   (hidden) counts the AudioUnits made, so a test can prove that listing makes
   none. Upstream cpal still makes the AudioUnit in these probes.

These defects were found by independent standalone review. Native stream lifecycle
verification is recorded in the instrument's standalone evidence; a pointer-ownership
unit test alone does not establish CoreAudio listener removal/concurrent close safety.

Two compiler-only cleanups remove an unused CoreAudio `Rc` import and spell out
WASAPI's elided `MutexGuard` lifetime. They have no runtime effect.
