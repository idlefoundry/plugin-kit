# clap-wrapper, patched

The Audio Unit's wrapper (the CA-72's `docs/decisions.md` R30): its AUv2 wrapper, built by
`scripts/auv2.sh` around the CLAP bundle, is the component's own code.

[free-audio/clap-wrapper](https://github.com/free-audio/clap-wrapper), tag `v0.16.0`, commit
`1cca996e96f29ab2be7ae9f8cfe532bbc92e1dd6` (MIT, `LICENSE`), the project's latest:
`CMakeLists.txt`, `LICENSE`, `README.md`, `cmake/`, `include/`, `libs/` and `src/`; its tests
and CI are left out. Of `libs/`, the AUv2 wrapper compiles `fmt/` (the {fmt} library's
headers, MIT, its licence in `fmt/fmt/format.h`); `psl/` is the VST3 wrapper's. The SDKs it
builds against are vendored beside it, unmodified, at the versions its own
`cmake/base_sdks.cmake` fetches: `../clap` (CLAP 1.2.6) and `../AudioUnitSDK`
(AudioUnitSDK 1.1.0).

As copied, before the changes below, the tree's SHA-256 (every file but this one, sorted by
path, as
`find . -type f ! -name PATCHES.md -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256`
from this directory) was
`8a2ba5677ba806cee610fef7247df7787fc34455f07608c9181eedd76026b8ae`.

pluginval (strictness 10) aborted the host in about one run in four, in its parameter
thread-safety test, with the heap corrupted, and failed its state restoration test in about
one in eight. Four changes, in `src/wrapasauv2.cpp` and
`src/detail/auv2/auv2_base_classes.h`, fix these and the parameters' order:

1. **Parameter changes from any thread.** `SetParameter()` added the host's change to the
   process adapter's event list at once. A host calls it from any thread: its main thread
   while the audio thread renders (a knob of the host's own moved during playback), the
   audio thread, or while another thread initializes the unit, which clears and reserves
   that same list (pluginval's test does that). The list is a plain vector, so two threads
   could write it together: events lost or garbled, or the heap corrupted. Now the change
   waits in a list of the wrapper's own, under a lock (`_pendingParameters`); `Render()`
   takes the waiting changes, trying the lock and never waiting on it (if another thread
   holds it for the moment, the changes go with the next block), and gives them to the
   process adapter on the audio thread. A change made before the unit is initialized now
   reaches the plugin with its first block rather than being dropped. `RestoreState()`
   drops the changes still waiting, which the state supersedes.
2. **A saved state has the host's latest changes.** `SaveState()` asked the plugin for its
   state while the host's changes since the last block were still waiting for the next one
   (`ProcessAdapter::flush()` is declared upstream but was never written). Now, if any are
   waiting, it hands them to the plugin first with `clap_plugin_params.flush()`, holding a
   lock that `Render()` holds throughout, so that no block is processed meanwhile. The
   host is then most likely not rendering (or the audio thread would have taken them); if
   it is, `SaveState()` waits for the block's end, and the next block for the flush.
   CLAP reserves `flush()` for the audio thread while the plugin is active, as it is here
   from the unit's initialization on; nih-plug's (`third_party/nih-plug`) only sets the
   parameters, and with the lock nothing else runs the plugin meanwhile. The output list
   is passed as null, which nih-plug takes to mean none: the editor's own changes stay
   queued for the next block and reach the host from there.
3. **The host reads the restored values at once.** After `RestoreState()` loaded a state,
   the unit's own copy of the parameters' values, which the host reads, kept the earlier
   ones until the plugin's request to rescan them (`CLAP_PARAM_RESCAN_VALUES`, which nih-plug
   makes after every state load) came round on a later idle. A host reading them in
   between, as pluginval does, took the stale values for the state's. `RestoreState()` now
   rescans them itself, at once, after the load.
4. **The parameters in the plugin's order.** `GetParameterList()` gave the plugin's own
   order only when the plugin gives an AUv2 order of its own (clap-wrapper's
   `CLAP_PLUGIN_AUV2_PARAM_ORDERING` extension, which nih-plug does not have), and
   otherwise the SDK's, which sorts them by id: for the CA-72, nih-plug's hashes of the
   parameters' string ids, so the hosts' lists (Logic's and GarageBand's controls, the
   automation menus) were in no order. It now gives the order `setupParameters()` lists
   them in, the plugin's (the CA-72's, the panel's), under the lock that guards that list.

To update: copy the same paths from a newer tag (and the SDKs at the versions it then
fetches), apply these changes again if upstream has not made its own, build the component
(`scripts/auv2.sh`), validate it (`scripts/validate.sh`: auval and pluginval, several runs),
play it in GarageBand or Logic, and record the tag, commit and hash here.
