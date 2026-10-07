# Decisions

The decisions behind the kit, in the order they were made. "The owner" is the person who
commissions the work; an agent decision is one the owner has not separately approved. Records
are numbered K1, K2 and so on, apart from each plug-in's own R records.

## K1. One kit for what the plug-ins share

**Owner decision, 2026-10-06.** Shown that the CA-72, the CA-74 and the MC-79 each carried
their own copies of the same code, near-identical in three crates and diverging in the rest,
and that a fix made in one reached the others only when someone copied it, the owner decided
that the shared code becomes one crate. They agreed the plan: a public repository under
GPL-3.0-or-later, `idlefoundry/plugin-kit` (a working name), taken by each plug-in at a
commit; the pieces moved in order, starting with the analysis, the ngspice runner, the
real-time threads and one patched copy of the plug-in framework and the editor's windows;
the CA-74 and the MC-79 moved first, on branches merged on the owner's word, and the CA-72
last, in a release of its own.

**Agent decisions, 2026-10-06** (not separately approved):

- **The crates keep their split and take the kit's name:** `plugin-kit-analysis`,
  `plugin-kit-spice`, `plugin-kit-rt`, so that a plug-in's lab, which runs ngspice, and its
  plug-in, which never does, take only what each needs.
- **Analysis:** the three copies were the same but for their doc comments; this is the
  CA-72's.
- **ngspice:** the three copies differed in their environment variables and in one rule. The
  variables are now `NGSPICE` (the binary) and `REQUIRE_NGSPICE` (a missing or other ngspice
  fails a test), in place of `CA72_`, `CA74_` and `MC79_NGSPICE` and their `_REQUIRE_`
  forms, so that one setting on the reference machine and in CI serves every plug-in. The
  rule kept is the stricter: a test runs only on ngspice 47, the version the references were
  made with (the CA-72's and the CA-74's; the MC-79's copy ran on any version).
- **Real-time threads:** the CA-72's, with the CA-74's `Backoff` (spin, yield, then sleep;
  on Windows it yields, since a sleep there lasts a timer tick) and the MC-79's `demote` on
  macOS, which sets the standard policy where the others did nothing (the MC-79's R13). The
  watchdog's and canary's threads are named `plugin-kit-rt-watchdog` and
  `plugin-kit-rt-canary`. A plug-in that takes this moves from the others' behaviour only in
  that demote.
- **nih-plug:** a three-way merge of the copies on their common base, the CA-74's (changes
  1 to 8): the CA-72's 9 to 11 (null channels, a parameter set from the audio thread, a test
  context) and the MC-79's two for the shared background thread, numbered 12 and 13 here. The
  code merged without a conflict; `PATCHES.md` lists all thirteen.
- **baseview, clap-wrapper, the CLAP headers and the AudioUnitSDK:** the CA-72's copies as
  they are, with their records. baseview's three changes need the editor's part to take
  effect: drawing its frame again on `WindowEvent::Damaged`, and `Window::set_wants_keys`
  while it holds the keyboard.
- **A plug-in takes the kit by git dependency at a commit,** not as a submodule: Cargo finds
  each crate by name in the repository, the commit is in the plug-in's `Cargo.lock`, and the
  release's source archive (the CA-72's `scripts/git-sources.sh`) carries it for the GPL.

**Evidence (the Mac, 2026-10-06):** rustc 1.97.1; `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets -- -D warnings` and `cargo test --workspace` pass (14 tests; the
ngspice tests skip, ngspice not being installed there).

**Evidence (GitHub CI, 2026-10-07 UTC, `dbd0545`):** fmt, clippy and the tests green on macOS 15,
Windows 2025 and Ubuntu 22.04; on Linux with ngspice 47 built from its checked source and
`REQUIRE_NGSPICE=1`, so the runner's tests ran rather than skipped.

**Evidence (the plug-ins on `dbd0545`, 2026-10-06):** the CA-74 (its R11) and the MC-79 (its
R14), switched on their `plugin-kit` branches, rendered every factory preset the same to the bit
as their `main` (26 and 20 presets, `preset_render`), and passed fmt, clippy (with and without
`--all-features`) and their tests on the Mac, on the Linux reference machine with
`REQUIRE_NGSPICE=1`, and on Windows with MSVC, where each plug-in's DLL imports only the
system's libraries (the C runtime linked in).

## K2. One ngspice runner for every lab, the TR-808's improvements in it

**Owner decision, 2026-10-06.** Told that the TR-808's copy of the runner had become the best of
the four (it runs ngspice on Windows and keeps the shared machine's rules in its own code), the
owner agreed that its improvements come into the kit, that the TR-808 then takes the kit's
runner in place of its copy, and that the other plug-ins gain them by moving to the new commit.

**Agent decisions, 2026-10-06** (not separately approved):
- **From the TR-808's runner** (its decisions D3 and D32), into `plugin-kit-spice`: on
  Windows, the official package's console build (`C:\Spice64\bin\ngspice_con.exe`, which
  opens no window); every run at the lowest priority (`nice -n 19`; on Windows below normal,
  without a console window); every run holding `~/.cache/daw-timing.lock` shared, so that a
  timing run holding it exclusively keeps simulations from starting; and every run taking one of
  four slots, the owner's rule of at most four ngspice processes on a machine.
- **One slot directory for every lab,** `~/.cache/ngspice-slots` (or `NGSPICE_SLOTS_DIR`), in
  place of the TR-808's `~/.cache/808-ngspice-slots`, so the four are the machine's and not one
  project's. `Slot::try_acquire_in` takes a slot without waiting, for the test that four can be
  held and a fifth cannot.
- **Kept from the kit:** the variables `NGSPICE` and `REQUIRE_NGSPICE` (the TR-808's were
  `TR808_NGSPICE`, `TR808_REQUIRE_NGSPICE` and `TR808_SLOTS_DIR`), and `for_test`'s rule that a
  test runs only on ngspice 47 (the TR-808's ran on any version).
- **0.2.0:** a plug-in moving to it sees its lab's runs capped and lowered, and on Windows finds
  ngspice where the TR-808's package put it; nothing a run computes changes.

**Evidence (2026-10-06):** CI at `15e79be` green on macOS 15, Windows 2025 and Ubuntu 22.04 (the
slots' test allows a moment: macOS 15 frees a slot let go a little later than the others). The
CA-74 and the MC-79 on it: clippy clean and their tests green on the Mac, on the Linux reference
machine with `REQUIRE_NGSPICE=1`, and on the Windows machine, where the runner now finds
`C:\Spice64\bin\ngspice_con.exe` and their ngspice tests ran for the first time (139 and 194
passed, none skipped). The TR-808 on it: 14 tests green, and five hits made again with the kit's
runner the same to the bit as its reference (its D36).

## K3. The update check, MIDI Learn and softbuffer's fix, from the CA-72

**The owner's plan, 2026-10-06** (K1): the update check and MIDI Learn's core come into the kit
after the worker pool and the presets' library. The CA-74's release lead asked for these two
first, with the CA-72's fix to softbuffer, so that the CA-74 reaches the CA-72's features before
its release; the CA-72's code (its 0.1.3, `07a7dfb`, and its softbuffer fix, made on the Mac and
not yet committed there) is the spec, ported rather than redesigned.

**Agent decisions, 2026-10-06** (not separately approved):
- **`plugin-kit-update`, the CA-72's `update.rs` (its R27) given the plug-in.** A plug-in names
  itself with an `App`: its name as its release assets begin, its version, its repository. The
  releases' page, the API's address for the latest release and curl's user agent
  (`<NAME>/<version>`) come from it, as do the installers' names, the contract
  `<NAME>-<version>-macOS.pkg`, `-Windows-setup.exe` and `-Linux-x86_64.tar.gz`. Everything
  else is the CA-72's: the system's curl (`/usr/bin/curl` on macOS, `System32\curl.exe` on
  Windows, the one on the path elsewhere), HTTPS only, redirects too, 20 s, 1 MB, nothing sent
  but the request; curl writing a file made afresh that the editor polls each frame
  (`Update::tick`), no thread waiting, past 30 s a failure, curl killed and the file removed
  when the check is dropped; versions as three numbers; DOWNLOAD opening only addresses under
  the repository's releases, in the browser, never running anything; the scenes' texts.
- **What differs from the CA-72's file:** the drawer's scene is the kit's `Scene` and `Tone`,
  which the plug-in draws (its panel is its own); `Update::every_scene` lists every scene, a
  long version's among them, for a plug-in's test that each fits its drawer (the CA-72 tested
  its own scenes against its panel); the stand-ins for curl and the browser (`fake`) are public,
  for a plug-in's editor tests; Windows' shell is reached through `windows-sys`, which the kit
  already takes, in place of `winapi` (the same `ShellExecuteW`); the answer's file is named
  after the plug-in (`ca74-latest-release-…`).
- **Its tests are the CA-72's**, on a plug-in of the tests' own (`KIT-TEST`), the versions
  relative to `CARGO_PKG_VERSION` (the kit's), never written out; the three run by hand ask the
  CA-72's public releases as if from a version 0.0.1.
- **`plugin-kit-learn`, the CA-72's `learn.rs` (its R34) given the plug-in's list.** A
  `MidiMap<N>` holds a plug-in's `N` learnable parameters (at most 255): the 16 × 128 table of
  atomics the audio thread only loads, the arming taken with a compare-and-swap, every change
  made off the audio thread under a lock it never takes; reserved controllers refused with the
  reason; absolute 7-bit values on an exact channel; jump takeover; one controller a control and
  one control a controller; the table as versioned JSON by stable parameter id, read leniently.
  The plug-in lists its parameters (`knob`, `stepped`, `switch`) and says which parameter each
  is (`LearnTargets`, in place of the CA-72's `target` and `knob_param` functions); `check`
  holds the two together, as the CA-72's first test did.
- **What every plug-in would otherwise repeat around nih-plug:** `control_change` is the
  CA-72's `Ca72::midi` and `Ca72::learned` without the plug-in's own MIDI (a learnable
  controller to the table, an assigned one setting its parameter through the host with
  `ProcessContext::set_parameter_normalized`, the kit's nih-plug change 10, a knob gliding; a
  reserved one left to the plug-in); `Dezip<N>` glides learned knobs over 10 ms, set every 32
  samples; `filter_state` gives a state without a table an empty one.
- **The table in the state:** nih-plug's persistent field holds a `Persisted`, written by the
  instance with its keys in the CA-72's order (`version`, `assignments`; `param`, `channel`,
  `cc`), read as any JSON and checked against the list as it loads. The CA-72's own type could
  not serve: its JSON needs the plug-in's list, which serde cannot be given, and a
  `serde_json::Value` would order its keys by whether a plug-in builds serde_json with
  `preserve_order` (the CA-74 does).
- **The kit's nih-plug as a path dependency** of `plugin-kit-learn`, its default features off:
  a plug-in taking both from the kit at one commit has one nih-plug. So the kit's CI now builds
  nih-plug's core.
- **softbuffer 0.4.8, vendored and patched** (`third_party/softbuffer/PATCHES.md`), the copy
  committed unchanged first and the change after it: each copy registers its observer class
  under a name of its own at run time, so that a second plug-in's editor in one process (Bitwig
  Studio on macOS) is not blank. A plug-in takes it through `[patch.crates-io]`, by git at the
  kit's commit. The CA-72 found and made the fix; its test (`softbuffer_copies`, macOS only)
  stays in the plug-ins, which build softbuffer with their features.
- **0.3.0.** A plug-in that moves to it and takes none of the new crates sees nothing change.

**Evidence (the Linux reference machine, 2026-10-06, rustc 1.97.1):** `cargo fmt --all --
--check`; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test
--workspace`: `plugin-kit-update` 11 passed (3 ignored, run by hand), `plugin-kit-learn` 15
and its allocation test passed. The allocation test seen to fail (1,527 allocations) with a
`Vec` made in `learned`.

**Evidence (GitHub CI, 2026-10-07 UTC, `f217c14`):** fmt, clippy and the tests green on macOS 15,
Windows 2025 and Ubuntu 22.04 (on Linux with ngspice 47 and `REQUIRE_NGSPICE=1`):
`plugin-kit-learn`'s 15 tests and its allocation test, and `plugin-kit-update`'s 11, ran on each.

**Evidence (the CA-74 on `f217c14`, its branch `learn-update`, 2026-10-06):** every kit crate at
this commit, softbuffer through `[patch.crates-io]`, and the update check and MIDI Learn wired
into its plug-in (its R-UPDATE, R-LEARN, R-SOFTBUFFER): every one of its 26 factory presets
rendered the same to the bit as its `main` (`preset_render`); on the Linux reference machine,
with `REQUIRE_NGSPICE=1`, fmt, clippy with and without `--all-features` and its tests (179
passed, 0 failed) clean; its allocation test, through its own `process`, seen to fail with an
allocation in its MIDI path and passing without. On the Mac (Apple silicon, macOS 27.0) its
`softbuffer_copies` test passed with this softbuffer, which compiles this backend's change for
the first time from the kit, and panicked with crates.io's 0.4.8 ("could not create new class
"SoftbufferObserver", perhaps a class with that name already exists?").

## K4. An edit in the editor survives a host that reads it back, as Cubase does

**The owner's decision, 2026-10-07** (the CA-72's R36): users reported that the CA-72's buttons
did nothing in Cubase 15 on Windows 11. The fault was in the VST3 wrapper of the nih-plug the
plug-ins share; the owner had it fixed in the CA-72's own copy for its 0.1.4, and "the same
change in `plugin-kit` for the CA-74 and the MC-79".

**What was wrong**, as the CA-72's R36 has it: while the host processes audio, the wrapper did
not set a value the editor changed, but left it for the host to send back to the processor at
the next process call, and the controller reported the value from before the edit until then.
Within `endEdit()`, Cubase 15 reads the controller's value and sends that to the processor,
after the edit, so a click on a switch or a button set nothing, and a quick drag could end one
move short. The CA-74's and the MC-79's switches and buttons would have done nothing in Cubase
alike: their editors set parameters through the same wrapper.

**Agent decisions, 2026-10-07** (not separately approved):
- **The CA-72's change as it is**, its change 12, here change 14 (`PATCHES.md`): the edit is
  held, the latest for each parameter; the controller reports it; the next process call sets it
  at its start, before the host's own changes at its first sample; `setProcessing(false)` sets
  it if no call will. The kit's `src/wrapper/vst3/inner.rs` and `context.rs` were the CA-72's to
  the byte, and its `wrapper.rs` differs only by changes 12 and 13, which this does not touch.
- **The test in `plugin-kit-learn`,** the kit's one crate on nih-plug:
  `tests/vst3_host.rs`, the CA-72's, with nih-plug's VST3 wrapper switched on for the crate's
  tests alone (a dev-dependency), which brings `vst3-sys` into the kit's `Cargo.lock`.
- **0.3.1:** a fix. A plug-in takes it by moving its `rev` to this commit; until it does, its
  VST3 has the fault. Moving the CA-74's and the MC-79's (their `plugin-kit` branches) is their
  next step, with their factory presets checked bit-identical as before.

**Evidence (the Windows machine, 2026-10-07):** see the CA-72's R36 for Cubase. Here, on this
change: `cargo test -p plugin-kit-learn --test vst3_host`, 6 passed; with the wrapper restored,
4 failed.
