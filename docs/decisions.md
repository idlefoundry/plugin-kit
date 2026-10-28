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

## K5. The worn skins' materials, from the CA-74

**The owner, 2026-10-09:** asked, for the CA-72's realistic look, whether the CA-74's materials
code (its glow, glowing caps and dot lettering) should move into plugin-kit rather than be
copied: "plugin-kit".

**Agent decisions, 2026-10-09** (not separately approved):
- **`plugin-kit-materials`**, from the CA-74's `materials.rs` (its R36 to R38, R40): pictures
  placed and resampled at a scale (`place`, `sprite`, `resample`, `widened`), a display's glass
  (`glass`), a lit button's light on the surface round it (`Glow`), the translucent cap lit
  from inside made from an opaque cap's picture (`lighting`), its sheen as SVG (`lit_caps`),
  dots filled as one shape (`discs`, `fill`), a star's path, a rounded rectangle's path, and a
  dot-matrix display's lettering, five dots by seven with descenders (`font`). It depends only on
  resvg (0.45, the plug-ins'), for its tiny-skia.
- **The plug-in's own stays with it:** its pictures (the opaque cap, the glass), its colours
  and its SVG documents (`svg_over` in the CA-74). The colours a material takes are arguments:
  a lit button's light (`Underlight`; the CA-74's red, `RED_LIGHT`) and a translucent cap's
  (`Tint`, its lit and unlit colour of its glow and shade; the CA-74's red, `RED`, the same
  arithmetic as before).

**Evidence (the Linux reference machine, 2026-10-09):** the CA-74 on it (its branch
`kit-materials` from `release-0.1`, the crate by path while this is on its branch): its strip
(SCATTER and DOUBLE in use), its drop-down (twice) and its whole window with and without it
drawn the same to the bit as before; its panel's and plug-in's tests (213) pass; rustfmt;
clippy with `-D warnings`. This crate: rustfmt, clippy, its tests.

## K6. The stereo and AUTO GAIN, from the CA-74

**The owner, 2026-10-09**, of the CA-74's stereo copied into the CA-72: "are we going to move
the stereo effects to the plugin kit as well? seems like a common thing we would use"; told
the general parts would move, after the materials (K5), and on 2026-10-10: "Do 1 & 2" (K5
published, and this).

**Agent decisions, 2026-10-10** (not separately approved):
- **`plugin-kit-stereo`**, from the CA-74's stereo (its R25, R27 to R30, R41) as the CA-72 took
  it (its R-STEREO), moved without a change to its arithmetic:
  - `place`: the pan law (`pan_gains`, constant power, both sides whole at the centre),
    SCATTER's placement of a voice among VOICES at full SPREAD (`Placement`: EVEN, EDGES,
    CENTER) and DOUBLE's pair (`Placement::pair`, `pair_gains`), a voice's gains by SPREAD
    (`voice_gains`), the glide a voice's place follows its controls with (`GLIDE`,
    `glide_share`), DOUBLE's detune and trim, UNISON's trim.
  - `auto_gain`: DRIVE's correction at its steps (`Curve`, drawn straight between them), as a
    session saves it (`Saved`, `{"sound": .., "db": [..]}`), shared by the editor, the helper
    thread, the audio thread (read without a lock) and the host (`Calibration`), measured a
    render at a time (`Job`), K-weighted (`KWeighted`).
- **The instrument's own stays with it:** its voices, which measure a sound (the `Measure`
  trait: made for a sound at a rate, a sound's K-weighted energy at a DRIVE), what makes a
  sound and its key (passed to `Calibration::ask`), the number of steps (`N`), DRIVE's top and
  the average curve of its presets, and the session's field (nih-plug's `PersistentField`, on
  the plug-in's own wrapper: this crate takes no nih-plug, so a plug-in with its own, as the
  CA-72 has, can take it).

**Evidence (the Linux reference machine, 2026-10-10):** the CA-72 on it (its branch
`realistic-look`): every factory preset rendered with POLY's ten voices (`preset_render`) and
every factory preset's AUTO GAIN curve, measured at 48 kHz, the same to the bit as before;
three presets each with DOUBLE, UNISON and each placement forced on (eighteen renders) the same
to the bit as at the commit before; the session's field saved as the same text; its tests pass
(the placement's and the curve's moved here). This crate: its tests (14), rustfmt, clippy with
`-D warnings`. Not yet: the CA-74 on it.


## K7. INNER: the inner edge of each side's band

**The owner, 2026-10-10:** "right now, we have a way to control the outer edges, which is
width ... Imagine we're just dealing with the left-hand side of the stereo spectrum. You have
your far left-hand edge ... and then you have your right-hand edge, which is currently our
center. Width would bring things in from the left-hand edge, but we would have no control to
bring things in from the right-hand edge, from the center." Asked two questions, with a
recommendation for each, the owner chose the name: "Go with 'inner' and yes, this should affect
the CA-74 as well", taking the recommendations for the rest (below).

**Agent decisions, 2026-10-10**, as recommended to the owner:
- **INNER is a share of SPREAD (WIDTH), 0 to 100 %, not a place of its own.** Each side's voices
  sit in a band from INNER's share of SPREAD's way out to SPREAD's edge, each as far across it
  as its placement puts it at full SPREAD (`band`: `inner + (1 - inner) * out`). So both
  controls always do something, they never cross, and SPREAD still scales the whole picture
  as it did; at INNER 100 % every voice sits at SPREAD's edge, and at SPREAD 0 both sides are
  whole, whatever INNER.
- **A voice placed in the centre takes the side its turn falls on** (`Placement::side`), as
  EVEN takes them, the left first: EVEN's last voice of an odd number and CENTER's first. So
  INNER clears the centre of every voice; the voice moves only as far as INNER takes it, so
  there is no jump as INNER leaves 0.
- **DOUBLE's pairs keep to the band too** (`pair_gains`), still mirrored about the centre: a
  pair placed in the centre opens to INNER's share either side.
- **`voice_gains` and `pair_gains` take `inner` after `spread`.** INNER at 0 gives every voice
  and every pair the gains it got before, to the bit (`band(0, out)` is `out`), so a plug-in
  with INNER at 0 renders as before. **`voice_at` and `pair_at`** give where those gains put a
  voice or a pair, -1 to 1, so a plug-in's display of the field draws its voices where they are
  heard (the CA-74's and the CA-72's strips placed them by `place` times SPREAD themselves).

**Evidence (the Linux reference machine, 2026-10-10):** this crate's tests (19: INNER at 0 to
the bit for every placement, VOICES 1 to 12 and SPREAD 0 to 1; every voice within its side's
band and in its order; the centre's voice to its turn's side; the pairs; a display's places
where the gains put them), rustfmt, clippy with
`-D warnings`.

### EDGES dropped (2026-10-10)

**The owner, 2026-10-10**, in order: "I don't really see any difference between edge and even,
because edge is now just even with the inner controls on maximum, is it not?"; told it is close
but not identical at 8 to 10 voices: "if there is cases where it yields different behavior,
then maybe we can just leave it. Just make sure that that's true"; then "Do you think those
differences will be practically audible? I'm changing my mind again…"; "Drop it."; asked for
the renders first ("Actually render first"), listened, then: "k, I think we should remove
edges".

**Agent decisions, 2026-10-10:**
- **EDGES removed, because INNER covers it.** With VOICES 2 to 7, EVEN with the right INNER puts
  every voice where EDGES did (INNER 90 % at 3 voices down to 70 % at 7). With DOUBLE, EDGES
  (every pair at the edge) is any placement at INNER 100 %, to the bit (`band(1, out)` is 1).
  With VOICES 8 to 10 and no DOUBLE, EDGES filled strictly outside-in while EVEN puts its second
  pair halfway in, so no setting matches exactly; the closest EVEN is off by at most about 8 %
  of the way out (WIDTH 95 %, INNER 68 % at 10 voices), which the A/B renders showed is not
  heard in playing. CENTER never matched (its second voice goes left). The table's length,
  which was also the most voices placed apart, is now `MOST` (10).
- **`ALONE` (1) for a lone voice's pair** (POLY and UNISON off): all the way out at full SPREAD,
  whatever the placement and whatever INNER (`band(inner, 1)` is 1), since it has no place among
  others (the CA-74's R41). The plug-ins got that by passing EDGES for a lone voice; they pass
  `ALONE` to `pair_at` and `pair_gains` instead.
- **Two placements, EVEN (index 0) and CENTER (1)**; any other index is EVEN. The index is only
  how a voices' mix carries the placement to the worker threads; each plug-in maps its own
  parameter's index. `Placement::pair` is the voice's own place, as far out, for both.
- **The CA-74's more thorough pairs test moved here**
  (`double_pairs_are_as_far_out_as_their_voices_places`, without its EDGES loop), as its agent
  asked "next time the kit is open". Nothing in EVEN's or CENTER's places, `side`, `band`,
  `voice_at`, `pair_at`, `voice_gains`, `pair_gains` or `pan_gains` changed its arithmetic.

**Evidence (the Linux reference machine, 2026-10-10):** this crate's tests (20: every pair at
the edge is INNER at 100 % to the bit, for both placements, VOICES 2 to 10, every voice and
four SPREADs; a lone voice's pair at SPREAD's edge whatever INNER; each pair as far out as its
voice's place; the rest as before), rustfmt, clippy with `-D warnings`, and the workspace
checks without `Edges`. The A/B renders from the CA-72 (`ca72-edges-ab`): POLY and UNISON with
EDGES against EVEN with INNER 55 % measured the same stereo width (side over mid within
0.05 dB, the left-right correlation within 0.006); a single note, the worst case, differed by
about 2 dB of left-right balance on notes already 12 to 17 dB to one side, where plain EVEN,
the reference, differed by 6 to 8 dB.

## K8. The CA-72's two macOS fixes; the settings key's gear; DOUBLE_CENTS gone

**The owner, 2026-10-11**, of the CA-72's two macOS fixes, which the CA-74 takes from the kit
(its baseview and clap-wrapper are the kit's `third_party/`): "Fix them"; and of the gear both
plug-ins now draw, and the stereo constant the CA-74 stopped using: "Yup, tidy this up."

**Agent decisions, 2026-10-11** (not separately approved), on branch `mac-fixes` from the
`editor` branch's f22e2a5 (the CA-74's pin), so the `editor` branch's own work is untouched:
- **baseview change 4** (the CA-72's a0f06eb): on macOS each press and release reports the
  event's own position first, so the first click into a floating window Bitwig Studio sent no
  mouseMoved to lands where it is. **clap-wrapper change 5** (the CA-72's 8030fcd, its R-MAC):
  the AUv2 wrapper answers a plug-in's request to flush at the next idle while no block is
  rendered, so an editor's change takes in GarageBand while its transport is stopped. Both
  applied unchanged: the kit's vendored copies were the CA-72's, byte for byte, before them;
  their `PATCHES.md` entries came with them.
- **`gear_path`** in `plugin-kit-materials`, beside `star_path`: the settings key's mark, the
  CA-74's proportions (its R-SETTINGS); the CA-72's (its R-SKIN) is the same shape within a
  hair and can take this one at its next pin (`a_gear_has_eight_teeth_and_an_open_hub`).
- **`place::DOUBLE_CENTS` removed** from `plugin-kit-stereo`: it was the CA-74's 20 cents (its
  R28); the CA-74 now carries 100 as the CA-72 does (its R-DETUNE), and nothing in the kit or
  the plug-ins read it.

**Evidence (the Linux reference machine, 2026-10-11):** the kit's tests and Clippy with
warnings denied pass. The macOS fixes are the CA-72's, tried there in Bitwig and GarageBand;
the CA-74's build on this branch is tried on the Mac.

## K9. The CA-72's last editor fixes: softbuffer's tiles on macOS, baseview's press on Windows

**The owner, 2026-10-11,** of what the kit still lacked once the CA-72 was published (its
records R44 to R60): "get codex agents to do this"; then "merge anything into main that should
be merged. Just don't push anything public", and, for the workers, "You can run that launch
script".

**Agent decisions, 2026-10-11** (the CA-72's lead; two Codex workers, each reviewed), on branches
from `mac-fixes` (5cf2b39), merged after it (K8), in the order agreed with the CA-74's lead:
- **softbuffer change 2** (the CA-72's R59 and R60; `third_party/softbuffer/PATCHES.md`). On
  macOS the buffer is kept from frame to frame (`age` 1 once presented, as on Windows and X11)
  and shown through tiles of 256 pixels. A present gives new images only to the tiles its damage
  touches. The layer is flipped only where the layers above it leave the coordinates running up
  (`top_down`); without that, a flipped view (baseview's, so every editor's) showed the tiles
  in rows bottom to top. The code is the CA-72's, comments aside. An editor that presents only
  what changed (the CA-72's R58) now does so on macOS too; one that presents the whole frame is
  shown as before. A plug-in taking it should check the editor in a Mac host, settled against a
  whole repaint.
- **baseview change 5** (the CA-72's R60; `third_party/baseview/PATCHES.md`). On Windows a
  button's message gives the handler a `CursorMoved` at its own place before the press or
  release, as change 4 does on macOS, so a click with no move before it (Cubase 15) lands where
  it is. The worker's independent review found that the count of held buttons was read before
  that call and written after it, so a press or release let through by a modal loop run from the
  handler would be lost. It is now read after the call, in the kit and in the CA-72 alike, so the
  two copies stay the same.
- The tests stay in the plug-ins (K3): each change is proved with the CA-72's own tests, in an
  export of its main with the kit's copy put over its own.

**Evidence (2026-10-11):**
- **The Mac** (macOS 27.0, Rust 1.97.1, over SSH, nothing on its screen). The CA-72's
  `softbuffer_copies` passes with the kit's softbuffer, in a view of AppKit's own and in a
  flipped one. Without `top_down` it fails in the flipped view (180000 pixels wrong). With
  every tile given a new image, it fails at the one-pixel change. Restored, it passes again.
  Clippy over the CA-72's workspace with every feature and `-D warnings` passes. A present of
  the whole window took 1.43 ms, a usual frame's 0.10 ms.
- **Windows** (MSVC 1.97.1). The CA-72's seven window tests (`windows_window_tests`) pass in the
  desktop session with the kit's baseview, the counter's read moved included. Its
  `a_press_with_no_move_before_it_lands_where_it_is` fails with the kit's previous `window.rs`
  ("POLY's tab not chosen") and passes once it is restored. Clippy over the CA-72's workspace
  with every feature and `-D warnings` passes, and the kit's copy of baseview is the CA-72's,
  file for file.
- **Linux:** the kit's fmt, Clippy with warnings denied and tests (71 passed) pass on each
  branch.

## K10. On Windows, a held Escape reaches the editor before the host takes it (baseview change 6)

**The owner, 2026-10-11:** a host test of the CA-74 on Windows found that in REAPER 7.82, CLAP,
Escape closed the plug-in's window with a menu open or a value being typed (as VST3 it closed the
menu); asked, the owner had the fault fixed by a Codex worker ("preferably send codex agents to
fix them").

**Decisions, 2026-10-11** (the worker's, reviewed by the CA-74's lead; not separately approved):
- **The cause:** REAPER takes Escape from the queue for a CLAP's window before it asks the window
  `WM_GETDLGCODE` or dispatches the key, so change 3's answer (`DLGC_WANTALLKEYS` while the
  editor holds the keyboard) never comes into it. A trace of the window procedure showed neither
  the key nor the question before REAPER closed its window.
- **The fix:** each GUI thread with a parented baseview window installs a `WH_GETMESSAGE` hook
  (`third_party/baseview/src/win/escape.rs`). It acts only on a removed `WM_KEYDOWN` for Escape
  to a registered window that has the focus and holds the keyboard (`set_wants_keys`): it empties
  the queued message and sends the key to that window instead. Every other key, a peek, an
  unfocused window and an idle Escape keep their path, so REAPER's shortcuts work as before.
  The hook goes with the thread's last window; destruction inside the handler and after the
  thread's teardown are safe. Surge's REAPER fix uses a message hook too; no code was copied.
- **Numbered 6:** change 5 is the CA-72's press fix (K9). Both are Windows-only; macOS and Linux
  are unchanged. The README's baseview row counts five changes for Windows and one for macOS.

**Evidence (2026-10-11):** three native Windows tests in `src/win/window.rs` (retrieval, peeking,
focus, idle Escape, Space, two editors, destruction inside the handler and after the thread's
teardown): the two retrieval tests fail with the hook's registration disabled; all three pass,
on Windows with MSVC, after the rebase onto K9. Clippy with warnings denied passes for
`x86_64-pc-windows-gnu`, with and without all features; the kit's own checks pass (71 tests).
In REAPER 7.82 (CLAP and VST3) and Cubase 15 (VST3), the CA-74's renamed build closed QUALITY's
and the settings menus and cancelled a readout's typing with Escape, its window kept, Enter and
Backspace working; idle Escape and Space matched the VST3. The CA-72 has the same path and takes
this as its own baseview change 6.

## K-STANDALONE. Shared standalone audio, MIDI and sessions

**The owner, 2026-10-13:** asked for a researched standalone specification and each
task implemented, with the onscreen keyboard replacing the voices/stereo/output
area and a button on the preview/settings rail switching views. The CA-72's frozen
`docs/standalone-spec.md` records the scope and acceptance criteria.

**Decision:** put host services in `plugin-kit-standalone`, with an instrument/NIH
adapter owning parameters and the editor. Keep CPAL 0.15.3 and midir 0.9.1 rather
than raising the macOS floor. Native-default audio backends are the first support
tier; ASIO/native PipeWire/production JACK routing remain separate integrations.
The output callback owns the processor Box; destroying the stream returns it to
the control thread. MIDI/input queues are preallocated and multi-producer safe;
an overflow requests an emergency release. No callback acquires a mutex or does
file/UI work. Channel routing, capture-clock adaptation, recovery, startup
readiness, transactional apply and atomic sessions live in the same crate.

The first implementation conservatively rejected separate input/output devices.
Windows enumerates capture/render endpoints separately, so that would prevent
its external input. The final implementation uses a bounded adaptive capture
reservoir with linear interpolation and permits common-rate independent endpoints.
Input denial keeps output-only playback available with a visible warning.

**Evidence:** 19 deterministic tests pass on Linux, including zero counted
callback allocations with variable frame counts and MIDI traffic, and 60-second
independent-clock simulations at ±500 ppm, 48 kHz and 128/256 frames without
capture over/underflow after priming. Strict Clippy and formatting pass. No physical
audio/MIDI devices were opened. See `docs/standalone.md` for exact checks and
support limits: CPAL's lack of stable OS identifiers, preserved malformed
sessions, input adaptation quality/latency needing hardware verification, and
bounded caller shutdown with OS-driver operations that cannot be forcibly
terminated. Hardware and cross-OS acceptance remain unverified here.

**Independent review corrections, 2026-10-13:** the first source router conflated
channels and reported the final releaser's identity, and its controller clear
could erase another device's ownership. Merge devices only within channel/pitch,
retain the first emitted source, and give processors an explicit `Channel`/`Global`
controller policy. CA-72 chooses Global to preserve its existing omni behavior;
generic source/channel releases do not hard-mute an instrument's release tails.
Lost MIDI devices have independent generation invalidation and bounded releases;
surviving MIDI connections remain open.

The final save's result now reaches `shutdown()`. A fingerprinted bad-file guard
permits saving after the original is moved aside, while protecting a replacement
file. A cross-platform file lock refuses a second instance using the same state
directory. Saves coalesce. Sample-format choice checks the same buffer range as
negotiation. Capture starts after output construction; bounded startup/overflow
purging discards stale input before recent capture is played.

Crossbeam's packed-queue predecessor could retry when a producer was preempted
before publication. The shared `plugin-kit-queue` now uses atomic u64 slots,
one-attempt producer/consumer claims and publication only after the complete
payload is stored. It contains no unsafe code or retry loop. Failed MIDI claims
request an emergency release; source generations prevent a paused producer's
event from restarting a note afterward. There are 28 standalone tests and two
queue tests, including these reproductions; strict Clippy and formatting pass.
The separate CPAL CoreAudio retention/capability fixes are owned by the lead and
must be integrated before claiming verified macOS stream lifecycle behavior.

## K-APPLY-RECEIPT. Explicit standalone settings completion

2026-10-14: CA-72's Done/Apply UI needs the outcome of its exact application.
Running plus matching configuration cannot identify completion: an older snapshot
can match a queued request, and repeated failures can leave the same status and
configuration. Add `AppHandle::apply_tracked` alongside the existing `apply` API.
A bounded, nonblocking completion receipt returns the accepted normalised Config
or a failure after the control thread publishes the attempt's resulting status.
Each request has its own channel; dropping a dialog cannot block the host, and
control-thread exit reports a disconnected receipt. No callback, persistence or
automatic-recovery behavior changes. CA-72 is the first consumer; other instruments
can adopt this additive API when they add an explicit settings completion UI.

## K-PANIC-TOKEN. Preserve the identity of a captured standalone panic

2026-10-14: independent CA-72 integration review reproduced two UI PANIC clicks
straddling a captured callback. A boolean hardware reset could acknowledge the
newer UI epoch, allowing a later callback to cancel a fresh note. Add the optional
`AppHandle::panic_tagged` API and a default `AudioProcessor::panic_token` metadata
hook without changing `AudioBlock` or behavior for existing untagged callers.

One atomic packs the pending bit and a nonzero 63-bit UI token. The callback captures
both with one swap. Generic resets and MIDI queue overflow preserve a pending token;
multiple tagged requests coalesce to the last published one. Invalid tokens are
rejected without overwriting a pending request. Each process call receives its own
metadata, including None on following nonpanic blocks. Consumers acknowledge only
an exact matching UI token. Core tests cover capture interleaving, coalescing,
overflow, invalid tags and allocation-free per-block metadata clearing. CA-72 is
the first consumer; its adapter and note-source race regressions are checked there.

## K-STARTUP-READY. Restore the editor before warming the DSP

2026-10-14: CA-72's exact-artifact macOS native check opened a fresh session but
reported a startup timeout reopening its saved session. The control thread later
rewrote that session correctly. The GUI's two-second readiness wait included cold
processor preparation, whose circuit factory calibration may take seconds. That
work does not determine the editor's restored size and must not hold the handshake.

Publish readiness and the restored configuration immediately after session restore,
then prepare/deactivate the stopped processor on the same control thread before
handling commands or opening devices. Audio processing and persistence formats are
unchanged. A channel-gated cold processor proves restored state/configuration are
available while preparation is still pending; the test failed before this change.
All 43 standalone tests and strict workspace Clippy pass locally. The consumer's
native two-launch restoration check remains required on its updated artifact.

## K-DEVICE-LISTING. List devices by property reads, off the control thread

2026-10-19, CA-72 issue 112. On the owner's Mac the standalone opened with no audio
devices and no MIDI ports, and no error. The control thread listed devices with
cpal, whose CoreAudio probes made a HAL AudioUnit and set its device to read the
buffer frame size range; setting the device creates an IO proc, and for five
virtual drivers (Immersed, Steam Streaming Microphone and Speakers, Virtual Desktop
Mic and Speakers) that call never returned. Nothing was published and MIDI was
never polled. After a reboot cleared the drivers, the owner found the second face
of the same cause: the app asked for microphone access at every launch, with
external input disabled. "I don't think we should be requesting access for that
unless the user specified that they were wanting to use the microphone input."
The input probe's AudioUnit had input enabled.

cpal's macOS probes now read `kAudioDevicePropertyBufferFrameSizeRange` as a device
property (third_party/cpal change 3); only building a stream makes an AudioUnit,
and a hidden counter lets a Mac test prove that listing makes none. Devices are
listed by a worker with a 1.5-second limit per device, MIDI by a worker of its own;
the control loop only reads their results. A device past the limit is named in
`Devices::unresponsive` and the status, and skipped by later rounds while its call
is still blocked. WASAPI and ALSA listing open no stream (an audio client is
activated; the PCM is opened non-blocking for its parameters) and ask for no
permission; the same limit bounds them. Opening a chosen device describes only
devices of its name. On macOS the MIDI worker runs its run loop for an instant
before each poll: CoreMIDI serves endpoint notifications on the run loop of the
thread that made the process's first client, and with none served (as on the old
control thread) a source published after launch was never listed; on the owner's
Mac a long-lived client saw it only after one run-loop pass. Tests: a fake host whose devices block (one, five, and one
whose name never answers), MIDI that stalls and recovers, and the control loop
with a blocked device (MIDI published first, commands handled, the list published
naming the device, the apply answered, shutdown under two seconds).

## K-MICROPHONE-WAIT. Ask for the microphone without waiting; the output plays meanwhile

2026-10-19, CA-72 issue 112, after K-DEVICE-LISTING. With listing fixed, choosing
an external input on macOS showed the microphone prompt as it should, but opening
the input waited for the answer: the control thread sat in `CreateIOProcID`
(`_TellServerAboutStreamUsage`, a reply from the audio server that comes only once
the user answers), the same call as the original hang. Sound stopped and the
settings read STARTING until the prompt was answered.

The control thread now reads the permission before opening an input, without
waiting (`+[AVCaptureDevice authorizationStatusForMediaType:]`, audio). Granted:
the input opens as before. Not determined: one `requestAccessForMediaType:
completionHandler:`, which returns at once; the output opens alone and the status
reads "Waiting for microphone permission… The external input starts once access is
allowed."; the answer comes back as a command, and an Allow reopens the streams
with the input. Denied or restricted: no input is opened or asked for; the output
runs and the status reads "Microphone access denied. Allow <app> in System Settings
› Privacy & Security › Microphone." The choice of input is kept in the settings and
the saved session, not cleared as a failed open clears it: the user did choose it,
and once access is allowed (the prompt, or later in System Settings, which each
two-second device poll reads) it opens without choosing again. The request is made
only for an app whose Info.plist has `NSMicrophoneUsageDescription`, since without
it macOS ends the process; an unbundled binary keeps the plain open. The
Objective-C calls and the completion handler (a global block, by the Blocks ABI)
are written against the runtime directly, so the kit takes no new crates. Other
platforms are unchanged.

To test it, opening streams is now a trait the control loop holds (`Streams`,
cpal's or a fake that renders on a thread of its own) beside the device lists and
the permission (`Hardware`). `Snapshot::output_frames` counts rendered frames, so a
test, and a check of an installed app, can see sound continue. Tests: the prompt
pending (output rendering, the notice, the input not opened, a second apply asks
nothing more), then allowed (the input opens, the notice clears); the prompt
refused (output running, the denial, the choice kept, disabling the input clears
it); already denied (never asked, never opened; allowed later in settings, the
next poll opens it); and the notice taking the device-list warning's place and
handing it back, never covering another owner's warning.


## K-AU-PITCH-BEND. A channel pitch wheel reaches the instrument through the Audio Unit

2026-10-17: the CA-72's issue 43 (its R-AU-PITCH-BEND, 3647201): in GarageBand the AU ignored
the pitch wheel. clap-wrapper's AU adapter turned channel pitch bend into per-note tuning
whenever the plugin preferred CLAP notes, though the port took MIDI too; nih-plug makes that
`PolyTuning`, which the CA-72 and the CA-74 never read (they follow `MidiPitchBend`), and the
conversion fixed the range at two semitones. clap-wrapper's change 6 (`PATCHES.md`) keeps the
MIDI bytes when the port supports MIDI or MPE; a CLAP-only port keeps the tuning.

`third_party/clap-wrapper/tests/auv2-midi.cpp`, the adapter on its own event list, is built
and run by each plug-in's `scripts/auv2.sh` with its component. Measured with the CA-74's
(Highstead's, Apple silicon, macOS 27, in-process through the public AU API, its own test
code): before the change the adapter test failed for CLAP with MIDI and CLAP with MPE, and a
full bend up at MIDI BEND RANGE 2 moved A4 by +2.1 cents; after it, every adapter case passed
and the wheel moved A4 by +200.1 and -202.5 cents at range 2, +703.7 and -700.3 at 7, under 3
at 0, from MIDI bytes and MIDI 1 and MIDI 2 event lists alike. Not tried: a host (GarageBand,
Logic), a physical controller.

## K-RESIZE-RESULT. A host's answer to a resize reaches the editor (nih-plug change 17)

2026-10-17: the CA-72's R72 (its nih-plug change 14, b86a394), for the CA-74. VST3's
`request_resize()` returned success before the host answered, always on Linux (the request is
queued onto the host's `IRunLoop`), so a host that then refused left its frame and the editor's
window at different sizes. The frame now reports the host's answer, with the id of the request
the editor made (`Editor::resize_request()` and `resize_result()`), and the context says that
an answer will follow (`resize_response_will_follow()`). Each editor keeps its own record of
which request is which and what to restore; one that overrides neither hook is unchanged.
`crates/plugin-kit-learn/tests/vst3_host.rs`: `a_resize_reports_the_hosts_answer_or_queues_to_its_gui_thread`
and, on Linux, `a_queued_resize_reports_refusal_on_the_hosts_run_loop`, as the CA-72 has them.

## K-CURSOR. The pointer's look on macOS (baseview change 7)

2026-10-17: the CA-72's R-MAC-FIXES (eb2f705), for the CA-74's grip (its R-SIZE). baseview's
macOS `Window::set_mouse_cursor` was `todo!()`: a call panicked, so an editor could not show the
resize arrows over its grip on the Mac. `src/macos/cursor.rs` maps each `MouseCursor` to an
`NSCursor` (the diagonal arrows to AppKit's own window corners'); the window keeps the look
asked for and shows it again as the pointer moves and as AppKit updates it. Windows and Linux
are unchanged. The four macOS files are the CA-72's to the byte (they were the same before the
change), built and tried there on macOS 27; not built in the kit on the Mac this time.

## K-CA72-0.2.8. The CA-72's window, wrapper and Audio Unit fixes of its 0.2.6 to 0.2.8

**The owner, 2026-10-22**, asked for the CA-74's first release, of the CA-72's fixes the kit
lacked: "yes, you can push branches to the plugin kit and merge the 8 fixes once tests pass on all
3 machines". The eight, as the CA-72 numbers them in its own copies (`third_party/` at its
`5a8b3b9`): baseview's changes 8 to 12, nih-plug's 15 and 16 and clap-wrapper's 7.

**Agent decisions, 2026-10-23** (the CA-74's kit worker; not separately approved):
- **The whole trees compared, not only the numbered patches.** The CA-72's
  `third_party/{baseview,nih-plug,clap-wrapper}` at `5a8b3b9` against the kit's at `bcc4d8f`,
  every difference put in one of four kinds:
  - *already here by another route:* everything up to baseview's 7, nih-plug's 17 and
    clap-wrapper's 6 (the CA-72's nih-plug 12 to 14 are the kit's 14, 15 and 17, and its
    comments number them so); nih-plug taking `plugin-kit-standalone` by path here, by git in
    the CA-72; clap-wrapper's change 6 worded for any plug-in here. The kit alone has nih-plug's
    12 and 13 (the MC-79's gate for background tasks), kept.
  - *the eight:* baseview 8 to 12 under their own numbers; nih-plug 15 and 16 as 18 and 19
    (the kit's 16 and 17 took theirs); clap-wrapper 7 under its own.
  - *the CA-72's other changes to these crates in its 0.2.2 to 0.2.8, without a number there,*
    ported as nih-plug 20 to 22: the token of a standalone's PANIC (`e43d39c`, the adapter's
    half of K-PANIC-TOKEN, which the kit's adapter missed), the standalones' window (the
    nih-plug half of R-WIN-STANDALONE, `8109b28`: the kit's baseview by path, without OpenGL,
    and one scale on Windows) and quitting it (the nih-plug half of R-QUIT, `45d3104`).
  - *for the CA-72's 0.3 alone, left out:* `TestProcessContext::set_transport` (its
    arpeggiator, R-ARP) and `set_plugin_api` (its TEMPO as a modulation target, R-LFO);
    `util::with_flush_to_zero` (its offline rendering for the MCP, R-REFERENCE-AUDIO); and the
    lockfiles it commits for baseview and nih-plug, which the kit ignores.
- **The code is the CA-72's,** applied from its commits; the files of the eight are its own
  byte for byte, but for the comments that number the kit's changes, the kit's own changes
  kept beside them, and the tests' new homes. The four baseview commits of a first worker,
  stopped mid-task, were the CA-72's code; their `PATCHES.md` entries were missing and are
  here, and the two macOS tests build in baseview's own edition (they had a per-target
  edition, which Cargo deprecates).
- **The tests come with the crates.** The CA-72 keeps most of them in its plug-in; those that
  need only the crate are here, so that the kit's CI runs them: `tests/backing_scale.rs` and
  `tests/mac_quit.rs` (macOS, on the main thread), the X11 identity tests, a Windows test of
  change 9's styles, and `tests/x11_expose.rs` (an X server; in CI under Xvfb) in baseview;
  `visibility_tests` (the CA-72's), the note expressions' test and
  `src/wrapper/standalone/production_tests.rs` (the CA-72's test of the quit key, and new
  ones of the token, the window's scale and the request to quit) in nih-plug;
  `tests/native-window-occlusion.cpp` (the CA-72's) in clap-wrapper. Each change's test was
  seen to fail without it where the change can be undone (`PATCHES.md`). The CA-72's tests of
  its own editor stay with it.
- **`cargo fmt --all` now reaches baseview and softbuffer** (nih-plug takes baseview by path,
  baseview's macOS test softbuffer), so softbuffer's `cg.rs` has its imports in its own
  edition's order, as the CA-72's has them.
- **`mac_quit` is not run on the owner's Mac:** `open_blocking` puts a regular application's
  window in front, and the rules for the owner's Mac forbid a window on its screen. GitHub's
  macOS runner runs it.
- **0.3.1 still:** the changes are fixes, and what is new (`native_window_occlusion_reliable`,
  `standalone_panic_token`, `app_window`) is additive.

**What a plug-in must do to have each:**
- baseview 8 (a window moved to a screen of another scale) and 11 (a standalone's macOS menus
  and quitting): nothing but take this commit; an application that opens its own window must
  shut down after `open_blocking` returns, as nih-plug's adapters do.
- baseview 9 (no frame to drag on Windows): nothing for a standalone opened by nih-plug
  (change 21); its editor sizes the window, through the adapter, as before.
- baseview 10 (Linux): call `baseview::set_application_class` with its desktop entry's
  `StartupWMClass` before any window opens (the CA-72's `identify_standalone`); without it a
  standalone's class is its first window's title, nih-plug's `Plugin::NAME`.
- baseview 12 (X11): draw and present the whole frame at `WindowEvent::Damaged`, as change 2
  asks on Windows: an editor that presents only what changed now gets one after each map and
  resize under XWayland.
- nih-plug 18 and clap-wrapper 7: an editor that skips presenting while its macOS window is
  occluded asks `GuiContext::native_window_occlusion_reliable()` and keeps presenting where it
  is false; the AU and the CLAP inside it are built from this one commit (a plug-in's
  `scripts/auv2.sh` takes clap-wrapper from the kit's checkout), and its AU build may add
  `tests/native-window-occlusion.cpp` beside `tests/auv2-midi.cpp`.
- nih-plug 19: nothing; a plug-in that reads `PolyBrightness` or `PolyExpression` from VST3
  now gets each as the host meant it.
- nih-plug 20: a standalone that tags its PANIC (`AppHandle::panic_tagged`) reads
  `ProcessContext::standalone_panic_token()` before `take_standalone_panic()` and acknowledges
  only its own token.
- nih-plug 21: a plug-in no longer builds a second baseview for its standalone, and must not
  turn on baseview's `opengl` feature (the standalones' window options no longer name a
  `gl_config`); on Windows its editor is told the monitor's scale (`set_scale_factor`) before
  its first `size()`, and answers `size()` in logical pixels.
- nih-plug 22: a standalone's editor that holds the keys on Linux hands Ctrl+Q over: where it
  is the standalone and `app_window::quits(&key)`, it calls `app_window::request_quit()` before
  any use of its own (the CA-72's editor does so ahead of its typing).

**Evidence (omarchy, Linux, 2026-10-23, rustc 1.97.1, ngspice 47, load 4 to 6 from other
work):** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace --no-fail-fast` with `REQUIRE_NGSPICE=1` (127 passed, 3 ignored, run by
hand); nih_plug_derive's 17; baseview's 6 identity tests, `x11_expose` (XWayland, its own
windows off the screen, override-redirect) and Clippy with warnings denied; nih-plug's 9
(`visibility_tests` 3, `note_expressions` 1, `production` 5) with `vst3,standalone-production`;
both standalone features compiled together; the occlusion test. Failing without their change:
`x11_expose` (timed out), two of `visibility_tests`' three, the note expressions' test, the
token's test, the occlusion test ("remote wrapper capability was not dispatched").

**Evidence (the Mac, macOS 27, M4 Pro, 2026-10-23, rustc 1.97.1; Live not running, load 11 to
22 from the owner's work, nothing timed):** workspace Clippy with warnings denied and its tests
(128 passed, 3 ignored); nih_plug_derive's 17; baseview's 6 identity tests and
`backing_scale` (a window never shown); nih-plug's 9; the occlusion test, built against
clap-wrapper's {fmt} headers. baseview's macOS backend fails Clippy with warnings denied, as
upstream's always has (objc 0.2's `msg_send!` trips `unexpected_cfgs`, and one `ptr_eq` in
upstream's `view.rs`); the kit does not check it. That run was of the tree at `a6a01fd`; what
followed changed only the order of softbuffer's `use` lists, `mac_quit` (which did not compile
in baseview's edition, the Mac having built it no more than it ran it) and text. By then the Mac
had 4.6 GiB free, below the 6 GiB the shared rules allow a build at, so GitHub's macOS runner
built and ran them.

**Evidence (Windows 11, MSVC, 2026-10-23, rustc 1.97.1):** over SSH, workspace Clippy with
warnings denied and its tests (125 passed, 3 ignored); nih_plug_derive's 17; nih-plug's 9 (the
refit is Windows'); both standalone features compiled together; baseview's Clippy with warnings
denied. In the signed-in session (a one-shot scheduled task), baseview's 10 library tests: the
6 identity tests, change 6's three and change 9's styles, which fails with the two styles put
back ("a frame to drag").

**Evidence (GitHub CI, PR #11, 2026-10-23, `cad311b`):** fmt, Clippy and the tests green on
macOS 15, Windows 2025 and Ubuntu 22.04
(https://github.com/idlefoundry/plugin-kit/actions/runs/39975306147), the vendored crates' steps
with them: on macOS `backing_scale` and `mac_quit`'s three (⌘Q, the menus, the quit Apple
event); on Windows baseview's 10 window tests; on Linux `x11_expose` under Xvfb; nih-plug's 9 on
each; the occlusion test on Linux and macOS.

## K-AGENT. Agents operate a standalone instrument through MCP (`plugin-kit-agent`)

**Agent decision, 2026-10-28** (the owner asked for the CA-72's agent support in Malleator and
for its shared part to live in the kit; not separately reviewed). The CA-72's agent transport
(its R-MCP: `crates/ca72-plugin/src/agent/` bridge, mcp, server, discovery, awake, setup) knew
nothing of the circuit but was written inside the CA-72. Malleator's R40 rebuilt it as an
instrument-independent crate and it moves here unchanged but for its manifest, which now takes
the workspace's version, edition, licence and shared dependencies.

- An instrument supplies `Config`: its identity, version, instructions and a table of `Tool`s.
  Its bridge binary runs `Bridge::at` then `mcp::serve` over stdio; its standalone's editor
  starts `Server::start_in` only after the player allows agents, drains at most 32 `Job`s a
  frame and answers them on the window thread. The plug-in in a host never listens.
- MCP revisions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05; newline-delimited JSON-RPC;
  input bounded to 1 MiB; eight connections; a request not taken in ten seconds is cancelled.
  Sockets live in a private (0700) per-user folder, each 0600; Windows uses AF_UNIX in the
  user's temporary folder. No TCP listener.
- The CA-72 keeps its own copy until it moves onto this crate, one change at a time, as the
  kit's other crates were taken.

**Evidence (Linux, 2026-10-28, rustc 1.97.1):** `cargo clippy -p plugin-kit-agent --all-targets
-- -D warnings` clean; `cargo test -p plugin-kit-agent` 15 passed; `cargo fmt --all -- --check`
clean. Malleator's R40 ran the whole flow on Linux through its packaged application (initialize,
13 tools, a chord played and listened to, consent saved, clean shutdown) under nih-plug's
allocation guard. Not built here on macOS or Windows.
