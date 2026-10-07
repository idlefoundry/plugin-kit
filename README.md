# plugin-kit

The code Idle Foundry's circuit-derived plug-ins share: the [CA-72](https://github.com/idlefoundry/ca-72)
and the plug-ins that follow it. Each plug-in models its own instrument's circuit; what they
have in common, from measuring a render to the patched plug-in framework, lives here once,
so that a fix made for one reaches them all. GPL-3.0-or-later, as the plug-ins are.

**Status:** 0.2.0: the first pieces (`docs/decisions.md` K1) and one ngspice runner for every lab (K2). The plug-ins move onto it one
at a time; until one has, it keeps its own copies.

## What is here

| Path | What |
|---|---|
| `crates/plugin-kit-analysis` | Measurements of rendered audio for the labs and the tests: loudness (ITU-R BS.1770-4), sample and true peak, a spectrum summary, onsets, a pitch track, A/B comparison of renders |
| `crates/plugin-kit-spice` | ngspice 47 as the offline circuit reference: runs netlists in batch mode, reads their rawfiles, fails on anything ngspice reports as an error; on Linux, macOS and Windows; at most four runs on a machine at once, at the lowest priority |
| `crates/plugin-kit-rt` | Real-time threads: promotion to the audio threads' scheduling (macOS time constraint, Linux `SCHED_FIFO`, Windows MMCSS), CPU pinning, a watchdog that demotes a starving thread, a backoff for waits |
| `third_party/nih-plug` | The plug-in framework, at upstream's commit with thirteen changes (`PATCHES.md`) |
| `third_party/baseview` | The editor's windows, with three changes for Windows hosts (`PATCHES.md`) |
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

and `baseview` the same way in its plug-in crate. A dependency's profiles are ignored, so a
plug-in sets `[profile.dev.package.plugin-kit-analysis] opt-level = 3` itself. Its release
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

The vendored crates are not members of this workspace; the plug-ins build and test them.

## Credits and licences

- nih-plug, by Robbert van der Helm (ISC; its VST3 bindings GPL-3.0).
- baseview, by the RustAudio contributors (MIT or Apache-2.0).
- clap-wrapper, by the Free Audio contributors (MIT); the CLAP headers (MIT); Apple's
  AudioUnitSDK (Apache-2.0).

Each keeps its licence in its folder. Everything else here is GPL-3.0-or-later
(`LICENSE`), Copyright © 2026 Idle Foundry Ltd.
