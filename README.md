# plugin-kit

The code Idle Foundry's circuit-derived plug-ins share: the [CA-72](https://github.com/idlefoundry/ca-72)
and the plug-ins that follow it. Each plug-in models its own instrument's circuit; what they
have in common, from measuring a render to the patched plug-in framework, lives here once,
so that a fix made for one reaches them all. GPL-3.0-or-later, as the plug-ins are.

**Status:** 0.3.0: the first pieces (`docs/decisions.md` K1), one ngspice runner for every lab
(K2), and the update check, MIDI Learn and a patched softbuffer from the CA-72 (K3). The plug-ins
move onto it one at a time; until one has, it keeps its own copies.

## What is here

| Path | What |
|---|---|
| `crates/plugin-kit-analysis` | Measurements of rendered audio for the labs and the tests: loudness (ITU-R BS.1770-4), sample and true peak, a spectrum summary, onsets, a pitch track, A/B comparison of renders |
| `crates/plugin-kit-spice` | ngspice 47 as the offline circuit reference: runs netlists in batch mode, reads their rawfiles, fails on anything ngspice reports as an error; on Linux, macOS and Windows; at most four runs on a machine at once, at the lowest priority |
| `crates/plugin-kit-rt` | Real-time threads: promotion to the audio threads' scheduling (macOS time constraint, Linux `SCHED_FIFO`, Windows MMCSS), CPU pinning, a watchdog that demotes a starving thread, a backoff for waits |
| `crates/plugin-kit-update` | The update check, made when the user asks: the latest release on GitHub through the system's curl (HTTPS only, 20 s, 1 MB, nothing sent but the request), read by the editor each frame with no thread waiting, and the installer for the system opened in the browser, from the repository's releases only |
| `crates/plugin-kit-learn` | MIDI Learn's core: the 16 × 128 table of atomics the audio thread reads, a plug-in's list of learnable parameters, reserved controllers, absolute 7-bit values on an exact channel, jump takeover, one controller a control and one control a controller, the table saved by parameter id with the host's project, the glide of a learned knob |
| `crates/plugin-kit-materials` | The worn skins' materials: pictures placed and resampled at a scale, a display's glass, a lit button's light on the surface round it and its translucent cap lit from inside, dots filled as one shape, a dot-matrix display's lettering (K5) |
| `crates/plugin-kit-stereo` | The stereo and DRIVE's AUTO GAIN: a constant-power pan law, where SCATTER places the voices (EVEN, EDGES, CENTER) and DOUBLE its pairs, the places' glide and the trims of voices summed; AUTO GAIN's curve measured off the audio thread for each sound, K-weighted, read by the audio thread without a lock and saved with the session (K6) |
| `third_party/nih-plug` | The plug-in framework, at upstream's commit with thirteen changes (`PATCHES.md`) |
| `third_party/baseview` | The editor's windows, with three changes for Windows hosts (`PATCHES.md`) |
| `third_party/softbuffer` | The editor's pixels, at the 0.4.8 release with one change for macOS hosts that load several plug-ins into one process (`PATCHES.md`) |
| `third_party/clap-wrapper`, `clap`, `AudioUnitSDK` | The Audio Unit, built from a plug-in's CLAP (`clap-wrapper/PATCHES.md`, `VENDORED.md`) |

## Using it

A plug-in takes the crates from this repository at one commit, by name, in its workspace's
`Cargo.toml`:

```toml
[workspace.dependencies]
plugin-kit-analysis = { git = "https://github.com/idlefoundry/plugin-kit", rev = "<commit>" }
plugin-kit-rt = { git = "https://github.com/idlefoundry/plugin-kit", rev = "<commit>" }
plugin-kit-spice = { git = "https://github.com/idlefoundry/plugin-kit", rev = "<commit>" }
nih_plug = { git = "https://github.com/idlefoundry/plugin-kit", rev = "<commit>" }
```

and `baseview`, `plugin-kit-update` and `plugin-kit-learn` the same way in its plug-in crate,
every one at the same commit. softbuffer replaces crates.io's for the whole build:

```toml
[patch.crates-io]
softbuffer = { git = "https://github.com/idlefoundry/plugin-kit", rev = "<commit>" }
```

A dependency's profiles are ignored, so a plug-in sets
`[profile.dev.package.plugin-kit-analysis] opt-level = 3` itself. Its release
archives the sources of its git dependencies, this repository among them, as the GPL asks.

Tests that need ngspice skip without it unless `REQUIRE_NGSPICE` is set; `NGSPICE` names the
binary when it is not found by itself (`crates/plugin-kit-spice`).

## Changing it

A fix to anything here is made here, never in a plug-in's copy, then each plug-in moves its
`rev` to the new commit and shows that its sound has not changed: every factory preset
rendered sample for sample as before (its `preset_render` test). A change that alters a
plug-in's samples needs its reason and a measurement, in that plug-in's records too.

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --no-fail-fast`

The vendored crates are not members of this workspace; the plug-ins build and test them
(`plugin-kit-learn` builds nih-plug's core here, without its wrappers' features).

## Credits and licences

- nih-plug, by Robbert van der Helm (ISC; its VST3 bindings GPL-3.0).
- baseview, by the RustAudio contributors (MIT or Apache-2.0).
- softbuffer, by the rust-windowing contributors (MIT or Apache-2.0).
- clap-wrapper, by the Free Audio contributors (MIT); the CLAP headers (MIT); Apple's
  AudioUnitSDK (Apache-2.0).

Each keeps its licence in its folder. Everything else here is GPL-3.0-or-later
(`LICENSE`), Copyright © 2026 Idle Foundry Ltd.
