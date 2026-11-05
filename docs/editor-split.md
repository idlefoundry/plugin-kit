# The editor's machinery and each plug-in's look

Inventory made on 2026-11-04 from the CA-72's `github/main` at `88575ba4` and the
CA-74's `origin/main` at `2ab6e04`. The frozen `brief-editor.md` remains the baseline;
its provisional R-LOOK, R-SKIN, R-TYPED and R-ULTRA became the CA-72's R44, R51,
R52 and R48. The kit's existing `origin/main` was fast-forwarded into `editor` at
`c46f7a1` in round 1. Rounds 2 and 3 use the same source commits. No round changes
what either plug-in uses or switches one to the additions.

The kit holds behaviour, state and machinery. The plug-in supplies artwork, colours,
fonts, measurements, layout, parameter identities and musical meaning. No new crate
below needs nih-plug, baseview or softbuffer. Window events are translated by each
plug-in's adapter. Only the pixel crate needs resvg's tiny-skia, as K5 does.

## Inventory for the whole job

Paths in the two source columns are relative to their respective repositories.
The storage crates were implemented in round 1, the pixels and clock/cache in round 2,
and drag, value, text, menu and browser state in round 3.

| Part and judgement | CA-72 source and record | CA-74 counterpart and difference | Destination and reason |
|---|---|---|---|
| VST3 view keys, general and already landed | `third_party/nih-plug/src/wrapper/vst3/view.rs`, R53 (R-KEYS), change 13 | The kit's nih-plug, change 15 | Existing nih-plug. Keep this host adapter there; no second key path in an editor crate. |
| Damage rectangles, sparse pictures, composition and pixel conversion, general | `crates/ca72-panel/src/render.rs`, `strip/worn.rs`; `crates/ca72-plugin/src/editor.rs`, `mod window`, `compose`, `fill`; R44, R48, R58, R59, R60 | `crates/ca74-panel/src/render.rs`, `strip/worn.rs`; `crates/ca74-plugin/src/editor.rs` and its window composition | `plugin-kit-pixels`, round 2. The newer CA-72's clipped rectangles and buffer-age fallback. Keep panel construction and platform presentation in each plug-in. |
| Still parts shared at a scale, general | `crates/ca72-panel/src/presets/drawer.rs`, `shared_still`, warming thread, R44 | `crates/ca74-panel/src/presets/drop.rs`, cached drawer pictures | `plugin-kit-editor-state::cache`, round 2. Key by caller identity, appearance generation and scale; scale alone would mix looks. The caller builds the pictures; the cache owns neither their style nor window. |
| One frame clock, general | `crates/ca72-plugin/src/opening.rs`, editor frame loop; R48 | `crates/ca74-plugin/src/opening.rs`, editor frame loop | `plugin-kit-editor-state::clock`, round 2. One time source, at most 30 frames a second and at most 50 ms advanced at once; animation duration and shape remain arguments. |
| Preset library and factory overlay, general | `crates/ca72-plugin/src/library.rs`, R10, R18 | `crates/ca74-plugin/src/library.rs`, R6, R10, R-PRESET-LAW | `plugin-kit-library`, round 1. Retain the CA-72's portable names, own-file handling, deterministic duplicate reports, overlay preservation and file-change cache. Both copies have the same storage model. |
| Sound capture and application, partly general | `crates/ca72-plugin/src/presets.rs`, `SoundParam`, `values_now`, `targets_for_sound`; R10, R18 | `crates/ca74-plugin/src/presets.rs`, `values_now`, `targets_of_sound`, tuning-law conversion | The library accepts a plug-in's codec and plain parameter ids. Framework parameter wrappers, gestures, defaults, migrations, curves and tuning-law conversion stay in the plug-in. The player's controls differ; the kit must not hard-code either list. |
| Preset list, prompts, filtering, scrolling and text caret, general | `crates/ca72-panel/src/presets.rs`, `presets/drawer.rs`; `crates/ca72-plugin/src/presets.rs`, R10, R18, R44 | `crates/ca74-panel/src/presets.rs`, `presets/drop.rs`; `crates/ca74-plugin/src/presets.rs` | `plugin-kit-editor-state::browser` and `text`, round 3. Share semantic actions and stable selection, with caller-supplied visible rows and hit targets. Keep each drawer's drawing and dimensions. Support the CA-74's INIT and preset pads as explicit caller actions, not mandatory keys. |
| Menus, general state and placement only | `crates/ca72-panel/src/learn.rs`, `Menu`; R34, R51 | `crates/ca74-panel/src/learn.rs`, `Menu`, grouped and marked items | `plugin-kit-editor-state::menu`, round 3. Titles and group heads cannot activate; disabled and current items remain distinct. Both renderers stay local, including all lettering and spacing. |
| Typed readouts and typed control values, general state | `crates/ca72-panel/src/value_entry.rs`, `strip/readout.rs`; `crates/ca72-plugin/src/editor.rs`, R52, R-KNOB-ENTRY | `crates/ca74-panel/src/entry.rs`, `crates/ca74-plugin/src/editor/entry.rs` and readout entry in `editor.rs`, R-KNOB-ENTRY | `plugin-kit-editor-state::value`, round 3. Selection, replacement, backspace, commit and cancel are general. Parsing units, clamping and formatting are caller functions; the field's dark glass and dot lettering stay local. |
| Stepped drags, general arithmetic | `crates/ca72-panel/src/interact.rs`, `drag`, R51 | `crates/ca74-panel/src/interact.rs`, stepped controls | `plugin-kit-editor-state::drag`, round 3. Supply count, start, distance a step and fine multiplier. The CA-72's particular knob travel and hit areas stay local. |
| Computer settings, general storage only | `crates/ca72-plugin/src/settings.rs`, R48, R51, R54, later per-key locking | `crates/ca74-plugin/src/settings.rs`, per-key writes and lock, four lettering sizes and preset pads | `plugin-kit-settings`, round 1. Re-read under an operating-system file lock, change chosen keys, retain unknown keys, replace a complete file. Keep defaults and schema in each plug-in; neither plug-in's lettering or keyboard choices become kit policy. |
| Settings key and menu contents, local choices over general menu state | Rail and editor settings menu, R51 | Rail and editor settings menu, including LOOK packs and more lettering sizes | The menu state above; each plug-in supplies its groups, labels and actions. No kit-wide skin enum. |
| Phosphor coverage, blur and compositing, general | `crates/ca72-panel/src/glow.rs`, R-LFO | No separate `glow.rs` at the inventoried CA-74 commit; K5's surface lighting is a different operation | `plugin-kit-pixels::glow`, round 2, correcting the proposed module name and counterpart. The mask, three box passes, clipping and source-over arithmetic are shared. Phosphor palettes and all visual constants remain local. Add this beside K5's existing materials; do not change their behaviour. |
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

Round 1's implemented interfaces, in outline (error results carry an explanatory string):

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

Round 2's implemented interfaces, without bodies (tiny-skia is re-exported):

```rust,ignore
// plugin-kit-pixels
pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self;
    pub const fn is_empty(self) -> bool;
    pub fn intersection(self, bounds: Self) -> Option<Self>;
    pub fn union(self, other: Self) -> Self;
}
pub struct Damage;
impl Damage {
    pub fn new() -> Self;
    pub fn full(bounds: Rect) -> Self;
    pub fn rects(&self) -> &[Rect];
    pub fn is_empty(&self) -> bool;
    pub fn clear(&mut self);
    pub fn add(&mut self, rect: Rect);
    pub fn add_clipped(&mut self, rect: Rect, bounds: Rect);
    pub fn clipped(&self, bounds: Rect) -> Self;
    pub fn merge(&mut self, gap: u32, maximum: usize);
    pub fn for_buffer(bounds: Rect, changes: Option<&Self>, age: u8, complete: bool) -> Self;
}
pub struct Sparse;
impl Sparse {
    pub fn new(image: Pixmap) -> Self;
    pub fn from_pixmap(image: &Pixmap) -> Self;
    pub fn pixmap(&self) -> &Pixmap;
    pub fn over(&self, target: &mut Pixmap, at: (i32, i32), clip: Rect);
}
pub fn copy(target: &mut Pixmap, source: &Pixmap, at: (i32, i32), clip: Rect);
pub fn over(target: &mut Pixmap, source: &Pixmap, at: (i32, i32), clip: Rect);
pub fn convert(source: &Pixmap, target: &mut [u32], damage: &Damage) -> Result<(), &'static str>;
// glow, also re-exported at the crate root:
pub struct Coverage;
impl Coverage {
    pub fn new(width: u32, height: u32) -> Option<Self>;
    pub fn width(&self) -> u32;
    pub fn height(&self) -> u32;
    pub fn data(&self) -> &[f32];
    pub fn clear(&mut self);
    pub fn add_mask(&mut self, mask: &Mask, alpha: f32) -> Result<(), &'static str>;
    pub fn stroke(&mut self, path: &Path, width: f32, alpha: f32);
    pub fn disc(&mut self, at: (f32, f32), radius: f32, alpha: f32);
    pub fn blur(&mut self, sigma: f32) -> Result<(), &'static str>;
    pub fn tint(&self, target: &mut Pixmap, rgb: [u8; 3], opacity: f32, at: (i32, i32));
    pub fn tint_clipped(&self, target: &mut Pixmap, rgb: [u8; 3], opacity: f32,
        at: (i32, i32), clip: Rect);
}

// plugin-kit-editor-state::clock: one caller-supplied time, no renderer.
pub const FRAME_INTERVAL: Duration; // A thirtieth of a second, rounded up to nanoseconds.
pub const MAX_ADVANCE: Duration; // 50 ms.
pub struct FrameClock;
impl FrameClock {
    pub fn new() -> Self;
    pub fn tick(&mut self, now: Instant, moving: bool) -> Option<Duration>;
    pub fn reset(&mut self);
}
// plugin-kit-editor-state::cache: the caller shares one cache between editors.
pub struct StillKey<I> { pub caller: I, pub appearance: u64, /* exact scale bits */ }
impl<I> StillKey<I> {
    pub fn new(caller: I, appearance: u64, scale: f64) -> Result<Self, &'static str>;
    pub fn scale(&self) -> f64;
}
pub struct BuildError; // A builder panicked.
pub struct Still<V>;
impl<V> Still<V> {
    pub fn get(&self) -> Option<Result<Arc<V>, BuildError>>;
    pub fn wait(&self) -> Result<Arc<V>, BuildError>;
}
pub struct StillCache<K, V>;
impl<K, V> StillCache<K, V> { pub fn new() -> Self; }
impl<K: Eq + Hash, V: Send + Sync + 'static> StillCache<K, V> {
    pub fn warm(&self, key: K, make: impl FnOnce() -> V + Send + 'static)
        -> std::io::Result<Still<V>>;
}
```

Refinements from the source code: `Damage::add` ignores empty rectangles and merges
overlap and touching edges transitively; optional extra merging takes the gap and
maximum count from the caller, rather than the CA-72's 8-pixel and 8-place choices.
`add_clipped` preserves R58's cost rule. `for_buffer` chooses the whole frame unless
the adapter knows the last frame is complete and buffer age is one; creation,
resize, failed presentation and repaint invalidate that knowledge in the adapter.
Composition takes arbitrary translated pixmaps and target-space clips. Conversion
accepts the already composed frame, validates the target before writing, preserves
RGB bytes and forces opaque alpha. It chooses no padding colour or panel layout.

Sparse and dense source-over use the same rounded division by 255, as the CA-72's
editor composition and `glow::lay` do; its older sparse path approximated that
division. No current plug-in changes. Coverage keeps the three box passes, with
bounded work even when the radius exceeds the mask; tint can be clipped after the
complete mask has been blurred. Palettes, core/halo strengths and the source's
minimum visible disc radius stay local. Invalid spreads are errors. Non-finite
opacity adds no light; finite opacity is clamped to zero through one.

The clock schedules from accepted ticks, rather than catching up a late deadline,
so no pair of ticks is closer than a thirtieth of a second. Stalls advance by at
most 50 ms, and the first moving tick after rest advances by zero while preserving
the pacing interval. Rapid stop/start requests cannot bypass the cap. Animation
shape and duration remain local. The cache reserves in-flight entries, closing the
CA-72's check-then-build race. Each editor keeps a clonable `Still` lease; it can
poll or wait, and the last lease joins its worker even if the cache was dropped.
Weak entries retain no pictures once editors release them. Caller, appearance and
scale changes cannot reuse an old key's build, including one still in flight.
Thread-start errors are returned; unwinding builders finish with an error.

Tests carry the source's parts/whole, clipped component, sparse/dense and conversion
checks, with fixed random seeds, translated layers, changing clips and buffer-age
fallbacks. Coverage has independent box-convolution and shape references and a
clipped tint comparison. Clock tests supply timestamps; cache tests coordinate
threads without sleeps or timing deadlines. No new dependency package or version
is needed: pixels takes K5's resvg, and editor-state uses the standard library.

Round 3's implemented interfaces, without bodies (the state crate still uses only
the standard library):

```rust,ignore
// drag: positions are counted from zero, count is the number of positions.
pub fn stepped_drag(start: usize, distance: f64, distance_a_step: f64,
    count: usize, fine_multiplier: f64) -> Result<usize, &'static str>;
pub fn normalised(position: usize, count: usize) -> Result<f64, &'static str>;

// text: positions count Unicode scalar values, not bytes or grapheme clusters.
pub enum TextEdit<'a> { Insert(&'a str), Backspace, Delete, Left, Right,
    Home, End, SelectAll }
pub struct TextField;
impl TextField {
    pub fn new(text: impl Into<String>) -> Self;
    pub fn text(&self) -> &str;
    pub fn caret(&self) -> usize;
    pub fn selection(&self) -> Option<Range<usize>>;
    pub fn set_caret(&mut self, character: usize);
    pub fn select(&mut self, anchor: usize, caret: usize);
    pub fn insert(&mut self, text: &str) -> bool;
    pub fn backspace(&mut self) -> bool;
    pub fn delete(&mut self) -> bool;
    pub fn edit(&mut self, edit: TextEdit<'_>) -> bool;
}

// value: the target is the caller's control identity; begin also serves a shortcut.
pub struct ValueEntry<T>;
impl<T> ValueEntry<T> {
    pub fn begin(&mut self, target: T, text: impl Into<String>, selected: bool);
    pub fn field(&self) -> Option<&TextField>;
    pub fn target(&self) -> Option<&T>;
    pub fn edited(&self) -> bool;
    pub fn edit(&mut self, edit: TextEdit<'_>) -> bool;
    pub fn cancel(&mut self);
    pub fn commit<V>(&mut self, parse: impl FnOnce(&str) -> Option<V>,
        clamp: impl FnOnce(V) -> V) -> Option<(T, V)>;
}
impl<T: PartialEq> ValueEntry<T> {
    pub fn click(&mut self, target: T, now: Instant, interval: Duration,
        text: impl Into<String>, selected: bool) -> bool;
}

// menu: the caller measures the whole menu and performs its own hit tests.
pub enum MenuRow<A> { Title(String), Head(String),
    Item { label: String, action: A, enabled: bool, marked: bool } }
pub struct Placement { pub bounds: [f64; 4], pub scale: f64 }
pub struct Menu<A>;
impl<A> Menu<A> {
    pub fn new(rows: Vec<MenuRow<A>>) -> Self;
    pub fn rows(&self) -> &[MenuRow<A>];
    pub fn choose(&self, row: usize) -> Option<&A>;
    pub fn set_hover(&mut self, row: Option<usize>);
    pub fn hover(&self) -> Option<usize>;
    pub fn fit(&self, at: (f64, f64), bounds: [f64; 4],
        measured_size: (f64, f64)) -> Result<Placement, &'static str>;
}

// browser: metadata copied from library entries; no parameter data or file handles.
pub enum Origin { Factory, Edited, User }
pub struct Preset { pub name: String, pub description: String, pub tags: Vec<String>,
    pub favorite: bool, pub origin: Origin }
pub struct PresetId { pub name: String, pub origin: Origin }
pub enum Tab { All, Favourites, Mine, Factory }
pub enum FieldId { Search, Edit, SaveName, SaveTags }
pub enum Prompt {
    Rename { preset: PresetId, field: TextField },
    Tags { preset: PresetId, field: TextField },
    Delete { preset: PresetId }, SaveAs { name: TextField, tags: TextField },
}
pub enum BrowserAction { Rename, Tags, Delete, Revert, SaveAs, Restore, MidiLearn,
    Close, Choose { row: usize, close: bool }, Favorite(usize), Init, PresetPad(usize) }
pub enum BrowserKey<'a> { Text(TextEdit<'a>), Enter, Escape, Tab, Previous, Next }
pub enum Operation {
    Rename { preset: PresetId, to: String }, Tags { preset: PresetId, text: String },
    Delete(PresetId), Revert(PresetId), SaveAs { name: String, tags: String },
    Favorite { preset: PresetId, on: bool }, Restore, MidiLearn, Init, PresetPad(usize),
}
pub struct Request { pub token: u64, pub operation: Operation }
pub enum Unavailable { NoPreset, NotEdited }
pub enum BrowserEffect { Choose(PresetId), Request(Request),
    Unavailable(Unavailable), Changed, Close }
pub enum Completion { Changed, Current(Option<String>) }
pub struct BrowserState;
impl BrowserState {
    pub fn new(visible_rows: usize) -> Self;
    pub fn refresh(&mut self, presets: impl IntoIterator<Item = Preset>);
    pub fn show(&mut self, on: bool);
    pub fn set_current(&mut self, name: Option<&str>);
    pub fn set_search(&mut self, text: impl Into<String>);
    pub fn set_tab(&mut self, tab: Tab);
    pub fn toggle_tag(&mut self, tag: &str);
    pub fn focus_field(&mut self, id: FieldId, caret: usize) -> bool;
    pub fn set_visible_rows(&mut self, visible: usize);
    pub fn scroll(&mut self, rows: isize, visible: usize);
    pub fn scroll_tags(&mut self, rows: isize, columns: usize, visible: usize);
    pub fn step(&mut self, by: isize) -> Vec<BrowserEffect>;
    pub fn act(&mut self, action: BrowserAction) -> Vec<BrowserEffect>;
    pub fn key(&mut self, key: BrowserKey<'_>) -> Vec<BrowserEffect>;
    pub fn confirm(&mut self) -> Vec<BrowserEffect>;
    pub fn complete(&mut self, token: u64, result: Result<Completion, String>)
        -> Vec<BrowserEffect>;
    pub fn list_changed(&mut self) -> Option<Vec<String>>;
    // Read-only accessors expose the fields, rows, current selection, tag chips,
    // first rows, prompt, focus, replacement, pending request and error.
}
```

Refinements from the source code in round 3: a drag takes a signed distance and a
caller-selected rate, returning an integer position before normalisation. Half a
step rounds away from the starting position, as the CA-72's selectors do; its
axes, knob travel and fine rate stay local. Empty position sets and invalid rates
are errors, and very large finite travel saturates without integer overflow.

Text selection and caret positions are kept private so edits cannot split a UTF-8
character. A parser receives the whole text, with no length limit or prefix
truncation. The caller validates units, acceptable input and finite values, clamps
the result and applies one host gesture. Enter ends invalid or unedited entry
without setting anything; the latter preserves precision hidden by a rounded
initial display, as both instruments' R-KNOB-ENTRY require. Readouts can begin
empty, knobs selected. Double-click timing and identity are supplied by the caller;
modifier clicks can begin directly. Escape, outside clicks and focus loss cancel.
The CA-74 has `entry.rs`, not `value_entry.rs`; its counterpart is corrected above.

Menu rows now include their labels, omitted by the proposed interface. Titles,
heads and disabled items cannot be chosen or hovered. Marking and enabling are
independent. Placement takes an anchor and a complete measured size and returns
bounds and a uniform reduction, as both source menus do when oversized. The
plug-in applies that same transform when drawing and finding rows; no font,
spacing, colour, row height or hit rectangle is imposed here.

The browser holds a metadata snapshot rather than a library or parameter type.
An adapter maps each `plugin-kit-library::Entry`'s sound name, description and tags,
its favourite flag and its three-way origin into `browser::Preset`, then calls
`refresh`. This keeps the crate dependency-free and filters without file I/O or
cloning sound parameters. Search words match name, description and tags; selected
tags are all required; tabs are exclusive. The later sources' optional FACTORY tab
is also available, using provenance rather than the favourite or tag overlay.
Picked tags remain visible if their last entry disappears. Row and tag scrolling
use measured row counts, with the tags' column count supplied too.

The newer CA-72's lifetime wins over the CA-74's older row-bound prompts: filtering
out a preset keeps its prompt. Refresh preserves selection by name and a prompt by
name plus origin, never an index; deletion, a changed origin or choosing another
ends it. A current preset removed elsewhere keeps its last name for the rail.
The caller supplies double-click detection for row selection; `close: true`
chooses and closes. SAVE AS opens a prompt with the current metadata, reports a
replacement and confirms on a second press; DELETE asks first. Escape takes back
what a key began, then closes. Tab visits only the fields currently shown.

Requests carry the stable identity and a completion token. Before a mutation, the
caller re-reads the library, checks the requested name and origin still exist,
validates the entire name or tags and executes the library operation. It reports
success or failure with `complete`, then refreshes the entries. Success can name
the caller's new current preset; failure keeps a prompt for correction. Duplicate
confirmations while pending issue no operation, and stale replies cannot end a
new prompt or select over another sound. The token guards UI lifetime; it does
not cancel file work the caller already began. Host parameter application,
refresh scheduling, animation, MIDI Learn's list and all drawing remain local.
INIT and preset pads return optional caller operations; `list_changed` provides
the filtered names for pads only when their set or order changes.

Round 3's tests carry the sources' character editing, commit/cancel, menu placement,
non-activating heads, every drag position on a step, search on a snapshot, prompt
identity through filtering and reordering, disappearance and origin changes. They
also cover oversized menus, empty and enormous scrolls, selection kept visible,
request errors and retries, stale completions, replacement warnings and optional
actions. No new dependency package, version or crate dependency is added. Visual
equivalence and frame cost are proved in a plug-in when it switches; no instrument
is switched and no look or performance claim is made by this round.

## Round 5: the released products are the default

The lead's K-EDITOR-CA72-PROOF decision supersedes the unreleased choices above.
The sources remain CA-72 `github/main` at `8d4c05b` and CA-74 `origin/main` at
`2ab6e04`. These four crates still sit beside everything either released product
uses. No existing framework, audio, materials or standalone crate changes.

| Difference | Released evidence | Settlement |
|---|---|---|
| Preset rewriting and opaque data | `crates/ca72-plugin/src/library.rs` and `crates/ca74-plugin/src/library.rs`, CA-72 R10/R18/R-FX-PRESETS and CA-74 R6/R10/R-PRESET-LAW: metadata rewrites reconstruct recognised keys; the CA-72 also appends effects verbatim | Reconstruct presets by default; codecs read original source through `read_source`, separate capture exclusions with `prepare_save`, and control bytes through `encode_preset` and `encode_table`. The CA-72 keeps its schema, migrations and effects interpretation. |
| Destination move and library locking | The same two library files move the owned filename before rewriting and create no directory lock file | The default follows both; same-process mutations are serialised without an extra file. Overlay unknown values survive, malformed individual marks are replaced on editing, and unchanged overlays are not written. |
| Tag chips and top row | `crates/ca72-plugin/src/presets.rs` and `crates/ca74-plugin/src/presets.rs`, CA-72 R18 and CA-74 R37/R-PRESET-KEYS, append vanished selected tags after sorted present tags and clamp the numeric top row | Both are the default. |
| Prompt lifetime | CA-72 keeps a filtered-out preset's prompt; CA-74 `crates/ca74-plugin/src/presets.rs::refilter` ends it when its row disappears (R37/R-PRESET-KEYS) | CA-72 default; `set_prompts_require_visible_row(true)` selects CA-74 behaviour. |
| Clock | `crates/ca72-plugin/src/editor.rs::move_opening` and `crates/ca74-plugin/src/editor.rs::move_opening`, CA-72 R48 and CA-74 R-ULTRA, advance a deadline with 15 ms callbacks and cap elapsed time at 50 ms | Deadline pacing is the default, including rounding the interval to 33,333,333 ns. |
| Damage | CA-72 `crates/ca72-panel/src/render.rs::merged` (R58/R59) defers merging, uses a strict gap and swap-removal order; CA-74 `crates/ca74-panel/src/render.rs::add_damage` (R-FRAMES) eagerly joins inclusively and appends the union | CA-72 default; `Damage::with_eager_merging(gap)` selects CA-74 order and inclusion, with its gap supplied by the caller. |
| Sparse blend | CA-72 `crates/ca72-panel/src/strip/worn.rs::Sparse` (R44/R48) divides by 256; CA-74 has no sparse counterpart | Sparse defaults to that pipeline arithmetic. Dense composition keeps rounded division by 255. |
| Coverage | CA-72 `crates/ca72-panel/src/glow.rs` (R-LFO) calculates radius in f32 and leaves box-pass round-off unclamped; CA-74 has no counterpart | Both become the default; palettes and the caller's minimum disc radius stay local. |
| Future settings | `crates/ca72-plugin/src/settings.rs::table` and `crates/ca74-plugin/src/settings.rs::table`, CA-72 R48/R51/R54 and CA-74 R-ULTRA, read known keys in newer files and rewrite only chosen keys | Read newer formats by default; bad TOML stays untouched. `Settings::read_with` and `change_with` let callers retain their schema and serialisation. |
| Creation mode and lock access | Both library writers create normal files under the process's umask; CA-72's settings lock is write-only and path-bearing errors identify failure | Normal creation permissions (0666 before umask and directory ACLs) and write-only settings locks are the defaults. |

Only the two documented product differences need options. Parameters, factory
versions, tuning laws, byte spelling, error wording and panel geometry are codec
or adapter policy. No unreleased behaviour is retained as a compatibility switch.

Further compatible interfaces let a caller implement `Metadata` on a thin entry
adapter, use the shared `matching` and `tags_of`, and reuse an immutable factory
set in `Library::in_directory` with a separate read cache. `raw_section` extracts
an opaque root's original header spans. Both released user readers treat a
non-integer format as an unversioned legacy file; factory roots and overlays
remain strict. Overlay errors name the file and say that it is left as it is.
