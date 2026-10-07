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
