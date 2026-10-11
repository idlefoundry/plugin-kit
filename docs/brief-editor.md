# Brief: the editor's machinery in the kit, from the CA-72

Written 2026-10-10 for an agent working alone. This brief is the frozen baseline for this job:
don't edit it. Record every decision, deviation and open question in `docs/decisions.md` as the
next K record (K8 at the time of writing), dated, with the reason, and whether the owner
approved it.

Before anything else, load the `circuit-plugin` skill and read its `SKILL.md`, then
`references/realistic-ui.md`, `references/plugin.md`, `references/conduct.md`,
`references/repo.md`, `references/hosts.md` and `references/release.md` (what may not appear in
a public repository). Then this repository's K1, K3 and K5: K5 is the precedent, the CA-74's
materials made general here and the CA-74 shown to draw the same to the bit on them.

## The goal

The editor's machinery the CA-72 built for its realistic look, in the kit, so that the TR-808's
plug-in (whose editor is built next) and later the CA-74 and the MC-79 build on one copy, and a
fix made once reaches them all. **The kit holds behaviour, state and machinery; each plug-in
brings its own look**: its pictures, colours, fonts, sizes and layout, as arguments or through a
trait, as K5 left the CA-74's colours and pictures with the CA-74.

**The owner, 2026-10-10**, asked whether the CA-72's editor parts should move into the kit
before the 808 builds on them or be copied into the 808 now: "Into plugin-kit first" (the 808's
D67).

## Where the CA-72 is

The CA-72's development line, at `5dc7b81` or later on its `main`, is on the owner's Mac and not
yet on GitHub (`idlefoundry/ca-72` is about 114 commits behind); the prompt that starts you says
where to reach it. Read its `docs/decisions.md` (R-LOOK, R-SKIN, R-ULTRA, R-TYPED, R-KEYS) and
its code there, never an older checkout. The CA-72 still builds on its own copies of the
framework (`third_party/nih-plug`, `baseview`, `softbuffer`) and of the update check and MIDI
Learn; it takes only `plugin-kit-materials` and `plugin-kit-stereo` from here.

## What moves

The candidates, from the CA-72 at `5dc7b81`. Confirm each against the code, decide what is
general and how to split it into crates (their names are yours), and record the split before
writing them.

| What | In the CA-72 | Its record |
|---|---|---|
| The keys a host gives a VST3 view (`IPlugView::onKeyDown`, `onKeyUp`): Cubase gives a plug-in its keys only this way, and the kit's nih-plug answers `kNotImplemented` (`src/wrapper/vst3/view.rs`), so no typed field works there | `third_party/nih-plug`, its change 13 (`PATCHES.md`); here it becomes the next change in `third_party/nih-plug/PATCHES.md` | R-KEYS |
| Frames drawn in part: each renderer's damaged rectangle (`ca72-panel/src/render.rs` `Renderer::damage`, `strip/worn.rs`'s), large mostly clear pictures laid only where they have pixels (`Sparse`), the editor's frame put together in the rows that changed and turned into the window's pixels a row at a time (`ca72-plugin/src/editor.rs`, `mod window`, `window::fill`), with the tests that a frame drawn in parts is the frame drawn whole | as named | R-LOOK, R-ULTRA |
| Still parts made once a scale on a thread begun as the editor opens, shared by the editors open at that scale | `ca72-panel/src/presets/drawer.rs` | R-LOOK |
| One frame clock for what moves: at most 30 frames a second, moved on by the time between frames (at most 50 ms of it) | `ca72-plugin/src/opening.rs` (`ANIMATE_S`) and the editor's frame loop (`editor.rs`) | R-ULTRA |
| The preset library: the TOML library under the shared "Idle Foundry" root, the factory overlay, files written whole and moved into place (the CA-74 and MC-79 have diverged copies) | `ca72-plugin/src/library.rs`, `presets.rs` | R10, R18; the skill's `plugin.md`, Presets |
| The presets' list and drawer, their behaviour and state: the search, the tags as filters, ALL, FAVORITES and MINE, the keys acting on the preset set (RENAME, TAGS, DELETE, REVERT, SAVE AS, RESTORE, MIDI LEARN, CLOSE), what a key has begun and when it ends, scrolling, a text field and its caret. The drawing (the CA-72's dots behind glass) stays the CA-72's | `ca72-panel/src/presets.rs`, `presets/drawer.rs` | R-LOOK, R18 |
| Menus: a title unlike its items, groups under a head, the current choice marked, within the window | `ca72-panel/src/learn.rs` (`Menu`) | R-SKIN, R34 |
| A value typed into a readout: begun by a double click, Enter sets it within the travel, Escape leaves it | the strip and the editor | R-TYPED |
| Controls of steps dragged by steps, never between two | `ca72-panel/src/interact.rs` (`drag`) | R-SKIN |
| The computer's settings (`settings.toml` beside the presets' library: a bad file left alone) and the settings key's menu | `ca72-plugin/src/settings.rs` | R-SKIN |

If a part cannot be made general without changing what the CA-72 draws or does, leave it in the
CA-72, record why, and move on.

## Scope

**In scope:** the parts above in the kit; the keys' change in the kit's nih-plug; the CA-72
switched to the new crates on a branch of its own, drawing and behaving the same; the kit's
README and the K record.

**Out of scope:** the CA-72's move onto the kit's framework, update check and MIDI Learn (a later
job); the CA-74's and MC-79's switch to the new crates (each later, on its own branch); the
808's plug-in; any change to how the CA-72 looks or behaves.

## Rules for the design

- No plug-in's look in the kit: no colours, pictures, fonts, sizes or panel units of the CA-72's
  (its `art::W`, its typeface, its SVG). The 808's drawer, menus and keys will look nothing like
  the CA-72's.
- Depend on as little as K5 does (resvg's tiny-skia for pixels). Keep window and framework types
  out of what can do without them, so the CA-72 on its own framework and the 808 on the kit's
  can both use it.
- No OFL or TeX Gyre fonts here.
- What moves keeps its tests, made general, and gains the CA-72's place in its doc comments
  (the record it came from).
- A public repository: systems named, not machines; no private paths, names or addresses;
  scan every commit before you push it (`release.md`).

## Order of work

1. **The inventory and the split**, in the K record: each part, general or not, the crates and
   their interfaces in outline.
2. **The keys' change** in the kit's nih-plug, with its test: small, and every plug-in on the kit
   needs it.
3. **The preset library**, then the drawing machinery, then the drawer's and menus' behaviour,
   the typed values, the drags and the settings.
4. **The CA-72 on them**, on its branch, proven as below.
5. **An independent review** by an agent with fresh context, of the code and every claim against
   its evidence. Fix its findings or record why not.

The 808's plug-in agent may be working at the same time on its mock-ups and audio, and its
editor waits for this. If landing the keys' change and the library first, as a pull request of
their own, would help, do that.

## Git

- **The kit:** this branch, `editor`; push it; open a pull request; merge only on the owner's go.
  Only one agent changes the kit at a time: until this merges, that is you.
- **The CA-72:** a branch from its latest `main`, pushed to the repository it lives in; merged
  only on the owner's go. Other agents change the CA-72's `main` most days: merge it into your
  branch often, keep the switch mechanical, and change nothing it does.
- Small commits, each ending with the session's co-author line.
- The machines are shared: heavy work at the lowest priority (`conduct.md`); kill only your own
  processes, by exact PID; never load a test build into a host the owner has open.

## When to stop and ask

- A decision that is the owner's: anything that would change how the CA-72 looks or behaves; a
  merge; anything new made public beyond this repository's own pull request.
- **Stuck:** three experiments in a row end without an identified cause, or one working day
  passes with no progress towards "Done when". Stop, and report the blocker with options and a
  recommendation. Don't open another diagnostic.

## Done when

- The new crates are in the kit with their tests; the README's table lists them; the K record
  says what moved, what stayed and why.
- The kit's nih-plug takes a VST3 view's keys, with a test, and a renamed test build of a
  plug-in on the kit takes typed text in Cubase 15 on Windows, with real key presses.
- The CA-72 on the new crates, on its branch: every editor state it can show (the panel, the
  presets' list open, each key's prompt, MIDI Learn's list, each menu, a value being typed) the
  same to the bit as before at 0.44 and 0.87 of the drawing; its approval test, its tests that a
  frame drawn in parts is the frame drawn whole, and all its panel's and plug-in's tests pass;
  its frame times (`--example frames`) and its first opening no slower, measured the same way
  before and after on the same machine.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass here and in the CA-72, on the Mac, the Linux reference machine
  and the Windows machine.
- The review is done, and its findings are fixed or recorded.

When done, report in at most 15 lines: what moved and what stayed, the proof, the branches and
pull request, what the 808 can now build on, and every question for the owner.
