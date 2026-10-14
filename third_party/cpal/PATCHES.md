# CPAL 0.15.3: CoreAudio lifecycle backports

The crates.io 0.15.3 source corresponds to RustAudio/cpal
`ac6cbb2ba55e61665a35ab88ae136a83380d1354`, Apache-2.0 (LICENSE included).
The package manifest is crates.io's normalized manifest; examples and source are
included. Optional ASIO is not enabled; no ASIO SDK is bundled or used.

Two upstream corrections are backported to preserve the existing API and macOS 11
floor. No other backend, API or audio-format behavior is deliberately changed.

1. [02b454c, fix callback being called after dropping the stream](https://github.com/RustAudio/cpal/commit/02b454c02cc42910192739db03de7b384f50b9cf).
   The CoreAudio disconnection closure captures a Weak reference to StreamInner,
   instead of a strong Stream clone owned by that same inner object. Internal
   play/pause helpers follow upstream. Dropping an enumerated stream can therefore
   destroy its AudioUnit and its callback, returning the standalone processor.
2. [51c3b43, fix macOS non-default audio output](https://github.com/RustAudio/cpal/commit/51c3b43c54591203fe3edc9a31ff7595650f7103).
   Both supported/default configuration probes construct an input AudioUnit only
   for input scope; output probes construct an output AudioUnit.

These defects were found by independent standalone review. Native stream lifecycle
verification is recorded in the instrument's standalone evidence; a pointer-ownership
unit test alone does not establish CoreAudio listener removal/concurrent close safety.

Two compiler-only cleanups remove an unused CoreAudio `Rc` import and spell out
WASAPI's elided `MutexGuard` lifetime. They have no runtime effect.
