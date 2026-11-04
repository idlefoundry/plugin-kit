# The editor's machinery and each plug-in's look

Inventory made on 2026-11-04 from the CA-72's `github/main` at `88575ba4` and the
CA-74's `origin/main` at `2ab6e04`. The frozen `brief-editor.md` remains the baseline;
its provisional R-LOOK, R-SKIN, R-TYPED and R-ULTRA became the CA-72's R44, R51,
R52 and R48. The kit's existing `origin/main` was fast-forwarded into `editor` at
`c46f7a1`. No existing crate is changed by this round.

The kit holds behaviour, state and machinery. The plug-in supplies artwork, colours,
fonts, measurements, layout, parameter identities and musical meaning. No new crate
below needs nih-plug, baseview or softbuffer. Window events are translated by each
plug-in's adapter. Only the proposed pixel crate needs resvg's tiny-skia, as K5 does.

## Inventory for the whole job

Paths in the two source columns are relative to their respective repositories.
An interface below is a proposal unless its crate is implemented in this round.

| Part and judgement | CA-72 source and record | CA-74 counterpart and difference | Destination and reason |
|---|---|---|---|
| VST3 view keys, general and already landed | `third_party/nih-plug/src/wrapper/vst3/view.rs`, R53 (R-KEYS), change 13 | The kit's nih-plug, change 15 | Existing nih-plug. Keep this host adapter there; no second key path in an editor crate. |
| Damage rectangles, sparse pictures, composition and pixel conversion, general | `crates/ca72-panel/src/render.rs`, `strip/worn.rs`; `crates/ca72-plugin/src/editor.rs`, `mod window`, `compose`, `fill`; R44, R48, R58, R59, R60 | `crates/ca74-panel/src/render.rs`, `strip/worn.rs`; `crates/ca74-plugin/src/editor.rs` and its window composition | Proposed `plugin-kit-pixels`. Take the newer CA-72's clipped rectangles and buffer-age fallback, rather than only its older changed rows. Keep panel construction and platform presentation in each plug-in. |
| Still parts shared at a scale, general | `crates/ca72-panel/src/presets/drawer.rs`, `shared_still`, warming thread, R44 | `crates/ca74-panel/src/presets/drop.rs`, cached drawer pictures | Proposed `plugin-kit-editor-state::cache`. Key by caller identity, appearance generation and scale; scale alone would mix looks. The caller builds the pictures; the cache owns neither their style nor window. |
| One frame clock, general | `crates/ca72-plugin/src/opening.rs`, editor frame loop; R48 | `crates/ca74-plugin/src/opening.rs`, editor frame loop | Proposed `plugin-kit-editor-state::clock`. One time source, at most 30 frames a second and at most 50 ms advanced at once; animation duration and shape remain arguments. |
| Preset library and factory overlay, general | `crates/ca72-plugin/src/library.rs`, R10, R18 | `crates/ca74-plugin/src/library.rs`, R6, R10, R-PRESET-LAW | `plugin-kit-library`, this round. Retain the CA-72's portable names, own-file handling, deterministic duplicate reports, overlay preservation and file-change cache. Both copies have the same storage model. |
| Sound capture and application, partly general | `crates/ca72-plugin/src/presets.rs`, `SoundParam`, `values_now`, `targets_for_sound`; R10, R18 | `crates/ca74-plugin/src/presets.rs`, `values_now`, `targets_of_sound`, tuning-law conversion | The library accepts a plug-in's codec and plain parameter ids. Framework parameter wrappers, gestures, defaults, migrations, curves and tuning-law conversion stay in the plug-in. The player's controls differ; the kit must not hard-code either list. |
| Preset list, prompts, filtering, scrolling and text caret, general | `crates/ca72-panel/src/presets.rs`, `presets/drawer.rs`; `crates/ca72-plugin/src/presets.rs`, R10, R18, R44 | `crates/ca74-panel/src/presets.rs`, `presets/drop.rs`; `crates/ca74-plugin/src/presets.rs` | Proposed `plugin-kit-editor-state::browser` and `text`. Share semantic actions and stable selection, with caller-supplied visible rows and hit targets. Keep each drawer's drawing and dimensions. Support the CA-74's INIT and preset pads as explicit caller actions, not mandatory keys. |
| Menus, general state and placement only | `crates/ca72-panel/src/learn.rs`, `Menu`; R34, R51 | `crates/ca74-panel/src/learn.rs`, `Menu`, grouped and marked items | Proposed `plugin-kit-editor-state::menu`. Titles and group heads cannot activate; disabled and current items remain distinct. Both renderers stay local, including all lettering and spacing. |
| Typed readouts and typed control values, general state | `crates/ca72-panel/src/value_entry.rs`, `strip/readout.rs`; `crates/ca72-plugin/src/editor.rs`, R52, R-KNOB-ENTRY | `crates/ca74-panel/src/value_entry.rs`, editor's typed-entry state, R-KNOB-ENTRY | Proposed `plugin-kit-editor-state::value`. Selection, replacement, backspace, commit and cancel are general. Parsing units, clamping and formatting are caller functions; the field's dark glass and dot lettering stay local. |
| Stepped drags, general arithmetic | `crates/ca72-panel/src/interact.rs`, `drag`, R51 | `crates/ca74-panel/src/interact.rs`, stepped controls | Proposed `plugin-kit-editor-state::drag`. Supply count, start, distance a step and fine multiplier. The CA-72's particular knob travel and hit areas stay local. |
| Computer settings, general storage only | `crates/ca72-plugin/src/settings.rs`, R48, R51, R54, later per-key locking | `crates/ca74-plugin/src/settings.rs`, per-key writes and lock, four lettering sizes and preset pads | `plugin-kit-settings`, this round. Re-read under an operating-system file lock, change chosen keys, retain unknown keys, replace a complete file. Keep defaults and schema in each plug-in; neither plug-in's lettering or keyboard choices become kit policy. |
| Settings key and menu contents, local choices over general menu state | Rail and editor settings menu, R51 | Rail and editor settings menu, including LOOK packs and more lettering sizes | Proposed menu state above; each plug-in supplies its groups, labels and actions. No kit-wide skin enum. |
| Phosphor coverage, blur and compositing, general | `crates/ca72-panel/src/glow.rs`, R-LFO | `crates/ca74-panel/src/glow.rs`, R-LFO | Proposed `plugin-kit-pixels::coverage`. The mask, three box passes, clipping and source-over arithmetic can be shared. Phosphor palettes and all visual constants remain local. Add this beside K5's existing materials; do not change their behaviour. |
| Skin packs and look validation, mixed | `crates/ca72-panel/src/look.rs`, R-SKIN-PACKS; separate `skin-pack` crate | `crates/ca74-panel/src/look.rs`, same pack format but its own slots and control checks | Leave with the plug-ins this job. Pack safety already belongs to `skin-pack`; slot names, font fitting and supported controls express the instrument's look. A future shared discovery cache would need its own brief. |
| Declarative arrangement, local | `crates/ca72-panel/src/arrangement.rs`, R-SKIN-PACKS | `crates/ca74-panel/src/arrangement.rs`, different controls, fixtures and allowed areas | Leave local. An arrangement's legality depends on the instrument's fixed controls and layout. The kit receives rectangles, never panel units or a control map. |
| What's New, partly general but outside this job | `crates/ca72-panel/src/whats_new.rs`, R-WHATS-NEW, with plug-in release selection | `crates/ca74-panel/src/whats_new.rs`, R-WHATS-NEW | Leave local this job. Text scrolling could later use the proposed scroll state, but release coverage and the dialog's drawing need a separate extraction; no update behaviour changes here. |

## Interfaces

The two storage crates use only packages already in the kit's dependency graph:
`toml_edit` with parsing alone, `serde_json` for quoted strings and `tempfile` for
complete replacements. A small TOML writer emits values and table headers, including
unknown nested values; it needs no additional writer package. Unchanged preset
table sections retain their original bytes, including comments and mixed line
endings (the CA-72's R-FX-PRESETS). Settings retain unknown values but normalise
spacing and comments. This round does not switch either plug-in.

Implemented interfaces, in outline (error results carry an explanatory string):

```rust,ignore
// plugin-kit-settings: TOML types are re-exported, with no framework types.
pub fn shared_dir() -> PathBuf;
pub fn plugin_dir(name: &str) -> Result<PathBuf, String>;
pub struct Settings { /* a file path */ }
impl Settings {
    pub fn shared(name: &str) -> Result<Self, String>;
    pub fn at(path: impl Into<PathBuf>) -> Self;
    pub fn path(&self) -> &Path;
    pub fn read(&self) -> Result<Option<Table>, String>;
    pub fn change(&self, change: impl FnOnce(&mut Table) -> Result<(), String>)
        -> Result<(), String>;
}
pub fn parse(text: &str) -> Result<Table, String>;
pub fn encode(table: &Table) -> Result<String, String>;
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String>;

// plugin-kit-library: built-in factory sounds are supplied, never written.
pub trait PresetCodec: Clone + Debug {
    type Parameters: Clone + Debug;
    fn read(&self, preset: &Table, factory_defaults: &Table)
        -> Result<Self::Parameters, String>;
    fn write(&self, parameters: &Self::Parameters, preset: &mut Table)
        -> Result<(), String>;
}
pub struct PlainValues { pub kept_on_save: Vec<String> }
pub struct Sound<P> { pub name: String, pub description: String,
    pub tags: Vec<String>, pub parameters: P }
pub struct Entry<P> { pub sound: Sound<P>, pub favorite: bool,
    pub origin: Origin, pub replaces: Option<String>, /* source file and table */ }
pub enum Origin { Factory, Edited, User }
pub fn parse_factory<C: PresetCodec>(text: &str, codec: &C)
    -> Result<Vec<Sound<C::Parameters>>, String>;
pub fn check_name(name: &str) -> Result<(), String>;
pub fn parse_tags(text: &str) -> Result<Vec<String>, String>;
pub struct Library<C: PresetCodec> { /* directory, factory sounds, codec, cache */ }
impl<C: PresetCodec> Library<C> {
    pub fn at(dir: impl Into<PathBuf>, factory: &str, codec: C) -> Result<Self, String>;
    pub fn shared(name: &str, factory: &str, codec: C) -> Result<Self, String>;
    pub fn entries(&self) -> (Vec<Entry<C::Parameters>>, Vec<String>);
    pub fn find(&self, name: &str) -> Option<Entry<C::Parameters>>;
    pub fn save(&self, name: &str, parameters: C::Parameters, tags: Option<Vec<String>>)
        -> Result<Entry<C::Parameters>, String>;
    pub fn rename(&self, name: &str, to: &str) -> Result<Entry<C::Parameters>, String>;
    pub fn favorite(&self, name: &str, on: bool) -> Result<(), String>;
    pub fn tag(&self, name: &str, tags: Vec<String>) -> Result<(), String>;
    pub fn remove(&self, name: &str) -> Result<(), String>;
    pub fn revert(&self, name: &str) -> Result<String, String>;
    pub fn restore(&self) -> Result<Vec<String>, String>;
    pub fn search(&self, query: &str, tags: &[String], favorite: bool, mine: bool)
        -> Vec<Entry<C::Parameters>>;
    pub fn tags(&self) -> Vec<String>;
}
```

The codec receives the factory root so the CA-72 can inherit its writer version and
the CA-74 can inherit its tuning law. User files receive no factory defaults. Their
different compatibility rules stay outside the kit. Metadata edits retain the parsed
source table instead of rebuilding a sound and discarding unknown fields. Capturing
and applying parameters remains an adapter's job: absent player controls are left
alone; explicitly named ones may still be applied.

Proposed later interfaces, without bodies:

```rust,ignore
// plugin-kit-pixels
pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
pub struct Damage;
impl Damage { pub fn add(&mut self, rect: Rect); pub fn clipped(&self, bounds: Rect) -> Self; }
pub struct Sparse;
impl Sparse { pub fn from_pixmap(image: &Pixmap) -> Self;
    pub fn over(&self, target: &mut Pixmap, at: (i32, i32), clip: Rect); }
pub fn convert(source: &Pixmap, target: &mut [u32], damage: &Damage);
pub struct Coverage;
impl Coverage { pub fn blur(&mut self, sigma: f32);
    pub fn tint(&self, target: &mut Pixmap, rgb: [u8; 3], opacity: f32, at: (i32, i32)); }

// plugin-kit-editor-state: geometry and timings are arguments, no renderer.
pub struct FrameClock;
impl FrameClock { pub fn tick(&mut self, now: Instant, moving: bool) -> Option<Duration>; }
pub struct StillCache<K, V>;
impl<K, V> StillCache<K, V> { pub fn warm(&self, key: K, make: impl FnOnce() -> V + Send); }
pub struct TextField;
impl TextField { pub fn insert(&mut self, text: &str); pub fn backspace(&mut self);
    pub fn set_caret(&mut self, character: usize); }
pub enum BrowserAction { Rename, Tags, Delete, Revert, SaveAs, Restore, MidiLearn, Close }
pub enum BrowserEffect { Choose(String), Request(BrowserAction), Changed, Close }
pub struct BrowserState;
impl BrowserState { pub fn act(&mut self, action: BrowserAction) -> Vec<BrowserEffect>;
    pub fn scroll(&mut self, rows: isize, visible: usize, count: usize); }
pub enum MenuRow<A> { Title(String), Head(String), Item { action: A, enabled: bool, marked: bool } }
pub struct Menu<A>;
impl<A> Menu<A> { pub fn choose(&self, row: usize) -> Option<&A>;
    pub fn fit(&mut self, bounds: [f64; 4], measured_size: (f64, f64)); }
pub struct ValueEntry;
impl ValueEntry { pub fn begin(&mut self, text: String); pub fn cancel(&mut self);
    pub fn commit<T>(&mut self, parse: impl FnOnce(&str) -> Option<T>) -> Option<T>; }
pub fn stepped_drag(start: usize, distance: f64, pixels_a_step: f64, count: usize) -> usize;
```

Those later crates must carry the source tests for parts equalling whole frames,
clipped damage, one shared still build, clock pacing, Unicode caret movement, prompt
lifetime and selection through refresh, non-activating headings, commit/cancel and
every drag position lying on a step. Visual equivalence and frame cost are proved in
the plug-in when it switches; no look or performance claim is made by this round.
