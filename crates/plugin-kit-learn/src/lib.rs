//! MIDI Learn (`docs/decisions.md` K3; the CA-72's R34): a hardware controller's absolute 7-bit
//! control change, on its own MIDI channel, assigned to one of a plug-in's sound parameters;
//! each instance its own table, saved with the host's project, never with the plug-in's sound
//! presets. What a plug-in gives it is its list of what may be learned ([`Learnable`]) and the
//! parameter each is ([`LearnTargets`]); the editor's menus and lists are the plug-in's own.
//!
//! - **What may be learned**: the plug-in's explicit list, in the order its editor lists it. The
//!   table is saved by each parameter's stable id, so the order may change.
//! - **What is not learned** ([`reserved`]): bank select (CC 0, 32), the modulation wheel (CC 1),
//!   data entry and RPN/NRPN (CC 6, 38, 96–101) and the channel mode messages (CC 120–127). The
//!   plug-in's own MIDI path keeps them ([`control_change`] leaves them to it).
//! - **One controller a parameter, one parameter a controller**: learning a parameter replaces
//!   its controller; learning a controller another parameter had moves it ([`MidiMap::assign`]
//!   says which). Removing one leaves the sound as it is.
//! - **Learning**: the editor arms a parameter ([`MidiMap::arm`]); the audio thread takes the
//!   next learnable control change, on its channel, as the one ([`MidiMap::incoming`]), changing
//!   nothing with it; the editor makes the assignment at its next frame ([`MidiMap::poll`]).
//!   Escape, CANCEL and the editor closing disarm it ([`MidiMap::cancel`]); arming another
//!   parameter moves the learning there. Nothing of learning is saved.
//! - **Playing** ([`control_change`]): a learned control change sets its parameter at its
//!   sample, through the host (`ProcessContext::set_parameter_normalized`, the kit's
//!   `third_party/nih-plug/PATCHES.md` change 10): a knob to value / 127 of its travel, a
//!   selector to the position its share of 0–127 falls in, a switch off at 0–63 and on at
//!   64–127. The value jumps there ("jump" takeover); a knob's voices follow it over [`DEZIP`]
//!   ([`Dezip`]).
//!
//! The audio thread reads the table and arms nothing: it only ever loads [`MidiMap`]'s atomics,
//! takes an arming with a compare-and-swap and posts what it caught in an atomic word. Every
//! change of the table is made off it, under a lock it never takes. Nothing here allocates on
//! the audio thread (`tests/realtime.rs`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// How a learned control change's 7-bit value sets a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A knob or slider: value / 127 of its travel.
    Continuous,
    /// A selector (or a count, such as VOICES): its positions each an equal share of 0–127.
    Stepped,
    /// A switch: off at 0–63, on at 64–127.
    Switch,
}

/// A parameter MIDI Learn may assign a controller to: its id (as saved), its name as the editor
/// gives it, and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Learnable {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

/// A knob in a plug-in's list.
pub const fn knob(id: &'static str, label: &'static str) -> Learnable {
    Learnable {
        id,
        label,
        kind: Kind::Continuous,
    }
}

/// A selector in a plug-in's list.
pub const fn stepped(id: &'static str, label: &'static str) -> Learnable {
    Learnable {
        id,
        label,
        kind: Kind::Stepped,
    }
}

/// A switch in a plug-in's list.
pub const fn switch(id: &'static str, label: &'static str) -> Learnable {
    Learnable {
        id,
        label,
        kind: Kind::Switch,
    }
}

/// The index in `list` of the learnable parameter of this id.
pub fn index(list: &[Learnable], id: &str) -> Option<usize> {
    list.iter().position(|l| l.id == id)
}

/// Why a control change is not learned, if it is not: what it is.
pub fn reserved(cc: u8) -> Option<&'static str> {
    Some(match cc {
        0 | 32 => "bank select",
        1 => "the modulation wheel",
        6 | 38 => "data entry",
        96 | 97 => "data increment and decrement",
        98..=101 => "an RPN or NRPN number",
        120 => "all sound off",
        121 => "reset all controllers",
        122 => "local control",
        123 => "all notes off",
        124..=127 => "a channel mode message",
        128..=u8::MAX => "not a control change",
        _ => return None,
    })
}

/// The reserved controllers, as an editor lists them.
pub const RESERVED_TEXT: &str = "NOT LEARNED: CC 0 AND 32 (BANK), 1 (MODULATION WHEEL), 6, 38 AND \
                                 96-101 (DATA ENTRY, RPN, NRPN), 120-127 (CHANNEL MODE)";

/// A controller: a MIDI channel (0 to 15; shown 1 to 16) and a control change (0 to 127).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cc {
    pub channel: u8,
    pub cc: u8,
}

impl Cc {
    /// As an editor shows it: `CH 1 · CC 74`.
    pub fn text(self) -> String {
        format!("CH {} · CC {}", u32::from(self.channel) + 1, self.cc)
    }

    fn slot(self) -> usize {
        usize::from(self.channel & 15) * 128 + usize::from(self.cc & 127)
    }

    fn of_slot(slot: usize) -> Cc {
        Cc {
            channel: (slot / 128) as u8,
            cc: (slot % 128) as u8,
        }
    }

    /// On a channel 0 to 15, a control change 0 to 127 and not reserved.
    pub fn learnable(self) -> bool {
        self.channel < 16 && self.cc < 128 && reserved(self.cc).is_none()
    }
}

/// What the audio thread does with a learnable control change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// Assigned to no parameter: nothing.
    Unassigned,
    /// Caught for the parameter being learned: nothing else (the sound is not changed by it).
    Caught,
    /// Assigned: set the learnable parameter of this index.
    Assigned(usize),
}

/// What an assignment changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assigned {
    /// The parameter and its controller now.
    pub param: usize,
    pub cc: Cc,
    /// The controller it had before (replaced), if another.
    pub replaced: Option<Cc>,
    /// The parameter the controller was taken from, if another had it.
    pub displaced: Option<usize>,
}

/// A slot of [`MidiMap::routes`] holds the learnable's index plus one; 0 is none.
const NONE: u8 = 0;

/// The learning word ([`MidiMap::armed`]): the parameter armed (index + 1; 0 none) in its low
/// byte, the arming's number above it.
const TARGET_BITS: u32 = 8;
/// A caught controller ([`MidiMap::caught`]): valid, the arming's number, the parameter, the
/// channel and the control change.
const CAUGHT: u64 = 1 << 63;

/// One instance's MIDI assignments and its learning, for a plug-in whose list of learnable
/// parameters has `N` (at most 255). Saved with the plug-in's state under [`STATE_KEY`]
/// ([`Persisted`]).
pub struct MidiMap<const N: usize> {
    /// What may be learned, in the editor's order.
    list: &'static [Learnable; N],
    /// By channel and controller (channel × 128 + CC): the learnable parameter it sets, its
    /// index plus one ([`NONE`] none). The audio thread only loads these; every change is made
    /// under `writer`, which keeps at most one controller a parameter.
    routes: [AtomicU8; 16 * 128],
    writer: Mutex<()>,
    /// The parameter being learned and the arming's number ([`TARGET_BITS`]).
    armed: AtomicU32,
    /// What the audio thread caught for the arming ([`CAUGHT`]), for [`MidiMap::poll`].
    caught: AtomicU64,
    /// The last reserved control change seen while learning, for the editor to explain: the
    /// arming's number above, the control change + 1 in the low byte (0 none).
    refused: AtomicU32,
    /// Counts every change of the assignments (the editor's lists follow it).
    version: AtomicU32,
}

impl<const N: usize> std::fmt::Debug for MidiMap<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiMap")
            .field("assignments", &self.saved().0)
            .field("armed", &self.armed())
            .finish()
    }
}

impl<const N: usize> MidiMap<N> {
    /// A table with no assignments for the parameters of `list`.
    pub fn new(list: &'static [Learnable; N]) -> Self {
        const { assert!(N <= 255, "at most 255 learnable parameters") };
        MidiMap {
            list,
            routes: std::array::from_fn(|_| AtomicU8::new(NONE)),
            writer: Mutex::new(()),
            armed: AtomicU32::new(0),
            caught: AtomicU64::new(0),
            refused: AtomicU32::new(0),
            version: AtomicU32::new(0),
        }
    }

    /// What may be learned.
    pub fn list(&self) -> &'static [Learnable; N] {
        self.list
    }

    // ---- The audio thread's: loads, a compare-and-swap and stores, nothing else.

    /// A learnable control change `cc` on `channel` (0 to 15), on the audio thread: caught for
    /// the parameter being learned (once), else its assignment.
    pub fn incoming(&self, channel: u8, cc: u8) -> Incoming {
        let c = Cc { channel, cc };
        if !c.learnable() {
            return Incoming::Unassigned;
        }
        // (Twice at most: the editor may arm again or cancel between the load and the swap.)
        for _ in 0..2 {
            let a = self.armed.load(Ordering::Acquire);
            let target = a & ((1 << TARGET_BITS) - 1);
            if target == 0 {
                break;
            }
            let disarmed = a & !((1 << TARGET_BITS) - 1);
            if self
                .armed
                .compare_exchange(a, disarmed, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let word = CAUGHT
                    | (u64::from(a >> TARGET_BITS) << 24)
                    | (u64::from(target - 1) << 16)
                    | (u64::from(channel) << 8)
                    | u64::from(cc);
                self.caught.store(word, Ordering::Release);
                return Incoming::Caught;
            }
        }
        match self.routes[c.slot()].load(Ordering::Acquire) {
            NONE => Incoming::Unassigned,
            k => Incoming::Assigned(usize::from(k) - 1),
        }
    }

    /// A reserved control change seen, on the audio thread: while learning, kept for the editor
    /// to say why it was not learned.
    pub fn refuse(&self, cc: u8) {
        let a = self.armed.load(Ordering::Acquire);
        if a & ((1 << TARGET_BITS) - 1) != 0 {
            let word = (a & !((1 << TARGET_BITS) - 1)) | (u32::from(cc.min(127)) + 1);
            self.refused.store(word, Ordering::Release);
        }
    }

    // ---- The editor's and the host's (state): never on the audio thread.

    /// The learnable parameter set by this controller, if any.
    pub fn target(&self, cc: Cc) -> Option<usize> {
        match self.routes[cc.slot()].load(Ordering::Acquire) {
            NONE => None,
            k => Some(usize::from(k) - 1),
        }
    }

    /// Every learnable parameter's controller, by its index.
    pub fn assignments(&self) -> [Option<Cc>; N] {
        let mut out = [None; N];
        for (slot, r) in self.routes.iter().enumerate() {
            let k = r.load(Ordering::Acquire);
            if let Some(o) = usize::from(k).checked_sub(1).and_then(|i| out.get_mut(i)) {
                *o = Some(Cc::of_slot(slot));
            }
        }
        out
    }

    /// The controller assigned to learnable parameter `param`, if any.
    pub fn assignment(&self, param: usize) -> Option<Cc> {
        self.assignments().get(param).copied().flatten()
    }

    /// How often the assignments have changed (the editor follows it).
    pub fn version(&self) -> u32 {
        self.version.load(Ordering::Acquire)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The slot of `param`'s controller, under the writer's lock.
    fn slot_of(&self, param: usize) -> Option<usize> {
        let want = u8::try_from(param + 1).ok()?;
        self.routes
            .iter()
            .position(|r| r.load(Ordering::Acquire) == want)
    }

    /// `cc` assigned to learnable parameter `param`: its controller before is let go (relearning
    /// replaces it), and a parameter that had `cc` loses it (it moves). None if either is not
    /// learnable.
    pub fn assign(&self, param: usize, cc: Cc) -> Option<Assigned> {
        if param >= N || !cc.learnable() {
            return None;
        }
        let _w = self.lock();
        let before = self.slot_of(param);
        if let Some(s) = before {
            self.routes[s].store(NONE, Ordering::Release);
        }
        let had = self.routes[cc.slot()].swap(param as u8 + 1, Ordering::AcqRel);
        self.version.fetch_add(1, Ordering::AcqRel);
        Some(Assigned {
            param,
            cc,
            replaced: before.map(Cc::of_slot).filter(|b| *b != cc),
            displaced: usize::from(had).checked_sub(1).filter(|&d| d != param),
        })
    }

    /// Learnable parameter `param`'s controller removed: which it was.
    pub fn remove(&self, param: usize) -> Option<Cc> {
        let _w = self.lock();
        let s = self.slot_of(param)?;
        self.routes[s].store(NONE, Ordering::Release);
        self.version.fetch_add(1, Ordering::AcqRel);
        Some(Cc::of_slot(s))
    }

    /// The assignments as saved: by learnable index, in its order.
    pub fn saved(&self) -> Saved {
        let _w = self.lock();
        Saved(
            self.assignments()
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.map(|c| (i, c)))
                .collect(),
        )
    }

    /// Every assignment replaced by `saved`'s (an empty table: none).
    pub fn load(&self, saved: &Saved) {
        let _w = self.lock();
        for r in &self.routes {
            r.store(NONE, Ordering::Release);
        }
        for &(i, c) in &saved.0 {
            if i < N && c.learnable() {
                self.routes[c.slot()].store(i as u8 + 1, Ordering::Release);
            }
        }
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Learnable parameter `param` armed: the next learnable control change is its controller.
    /// Another armed before is no longer (arming another moves the learning).
    pub fn arm(&self, param: usize) {
        if param >= N {
            return;
        }
        self.rearm(param as u32 + 1);
    }

    /// Learning cancelled: the assignments as they were.
    pub fn cancel(&self) {
        self.rearm(0);
    }

    fn rearm(&self, target: u32) {
        let n = (self.armed.load(Ordering::Acquire) >> TARGET_BITS).wrapping_add(1)
            & (u32::MAX >> TARGET_BITS);
        self.armed
            .store((n << TARGET_BITS) | target, Ordering::Release);
    }

    /// The parameter being learned, if any.
    pub fn armed(&self) -> Option<usize> {
        let target = self.armed.load(Ordering::Acquire) & ((1 << TARGET_BITS) - 1);
        (target as usize).checked_sub(1)
    }

    /// The arming's number.
    fn arming(&self) -> u32 {
        self.armed.load(Ordering::Acquire) >> TARGET_BITS
    }

    /// What the audio thread caught for the present arming, assigned (none: nothing caught, or
    /// caught for an arming since replaced or cancelled).
    pub fn poll(&self) -> Option<Assigned> {
        let word = self.caught.swap(0, Ordering::AcqRel);
        if word & CAUGHT == 0
            || ((word >> 24) & u64::from(u32::MAX >> TARGET_BITS)) as u32 != self.arming()
        {
            return None;
        }
        let param = ((word >> 16) & 0xff) as usize;
        let cc = Cc {
            channel: ((word >> 8) & 0xff) as u8,
            cc: (word & 0xff) as u8,
        };
        self.assign(param, cc)
    }

    /// The last reserved control change seen while this arming waits, if any.
    pub fn refused(&self) -> Option<u8> {
        let word = self.refused.load(Ordering::Acquire);
        let cc = word & 0xff;
        (cc != 0 && word >> TARGET_BITS == self.arming()).then(|| (cc - 1) as u8)
    }
}

/// The assignments as an instance holds them: learnable index and controller, in the list's
/// order, at most one a parameter and one a controller. As JSON ([`Saved::to_text`]):
/// `{"version":1,"assignments":[{"param":"cutoff","channel":1,"cc":74}]}`, by the parameters'
/// ids, the channel 1 to 16 as shown. Read leniently ([`Saved::from_json`]); an armed learning
/// is never part of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved(pub Vec<(usize, Cc)>);

/// The table's format.
pub const VERSION: u64 = 1;

impl Saved {
    /// A table as saved, checked against `list`: anything but an object of version 1 is an
    /// empty table; an assignment of a parameter not in `list`, a channel outside 1 to 16 or a
    /// controller outside 0 to 127 or reserved, or of a parameter or a controller already
    /// assigned above it, is left out.
    pub fn from_json(v: &serde_json::Value, list: &[Learnable]) -> Saved {
        let mut out: Vec<(usize, Cc)> = Vec::new();
        if v.get("version").and_then(serde_json::Value::as_u64) != Some(VERSION) {
            return Saved(out);
        }
        let Some(entries) = v.get("assignments").and_then(serde_json::Value::as_array) else {
            return Saved(out);
        };
        for a in entries {
            let param = a
                .get("param")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| index(list, id));
            let channel = a.get("channel").and_then(serde_json::Value::as_u64);
            let cc = a.get("cc").and_then(serde_json::Value::as_u64);
            let (Some(param), Some(channel @ 1..=16), Some(cc @ 0..=127)) = (param, channel, cc)
            else {
                continue;
            };
            let c = Cc {
                channel: (channel - 1) as u8,
                cc: cc as u8,
            };
            if c.learnable() && !out.iter().any(|&(p, o)| p == param || o == c) {
                out.push((param, c));
            }
        }
        out.sort_by_key(|&(p, _)| p);
        Saved(out)
    }

    /// The table from the state's text, checked against `list`: none (an empty table) if it is
    /// not JSON.
    pub fn from_text(text: &str, list: &[Learnable]) -> Saved {
        serde_json::from_str::<serde_json::Value>(text)
            .map(|v| Saved::from_json(&v, list))
            .unwrap_or_default()
    }

    /// The table as the state's text, by the ids of `list` (an index outside it is left out):
    /// its keys in this order whatever serde_json's features.
    pub fn to_text(&self, list: &[Learnable]) -> String {
        serde_json::to_string(&Table(list, &self.0))
            .unwrap_or_else(|_| format!("{{\"version\":{VERSION},\"assignments\":[]}}"))
    }
}

/// A table written by the ids of a list: `{"version":1,"assignments":[{"param":…,"channel":…,
/// "cc":…}]}`, its keys in that order.
struct Table<'a>(&'a [Learnable], &'a [(usize, Cc)]);

impl Serialize for Table<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Entry<'a>(&'a str, Cc);
        impl Serialize for Entry<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("param", self.0)?;
                m.serialize_entry("channel", &(u32::from(self.1.channel) + 1))?;
                m.serialize_entry("cc", &self.1.cc)?;
                m.end()
            }
        }
        struct List<'a>(&'a [Learnable], &'a [(usize, Cc)]);
        impl Serialize for List<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let entries: Vec<Entry<'_>> = self
                    .1
                    .iter()
                    .filter_map(|&(i, c)| Some(Entry(self.0.get(i)?.id, c)))
                    .collect();
                let mut seq = s.serialize_seq(Some(entries.len()))?;
                for e in &entries {
                    seq.serialize_element(e)?;
                }
                seq.end()
            }
        }
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("version", &VERSION)?;
        m.serialize_entry("assignments", &List(self.0, self.1))?;
        m.end()
    }
}

/// The table in the plug-in's state (nih-plug's persistent fields, `#[persist = "midi_map"]` on
/// an `Arc<MidiMap<N>>`): written by an instance as [`Saved::to_text`] writes it; read as any
/// JSON, a table that is not understood being an empty one, never an error, so that a state is
/// never half loaded over the assignments an instance had.
#[derive(Clone, Debug, PartialEq)]
pub enum Persisted {
    /// An instance's table, by the ids of its list.
    Written(&'static [Learnable], Saved),
    /// A state's, as read, checked when it is loaded ([`Saved::from_json`]).
    Read(serde_json::Value),
}

impl Serialize for Persisted {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Persisted::Written(list, saved) => Table(list, &saved.0).serialize(s),
            Persisted::Read(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Persisted {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Persisted::Read(serde_json::Value::deserialize(d)?))
    }
}

/// The table saved and loaded with the plug-in's state: loaded over every assignment the
/// instance had.
impl<'a, const N: usize> nih_plug::params::persist::PersistentField<'a, Persisted>
    for Arc<MidiMap<N>>
{
    fn set(&self, new_value: Persisted) {
        let saved = match new_value {
            Persisted::Read(v) => Saved::from_json(&v, self.list),
            Persisted::Written(list, saved) if list == self.list.as_slice() => saved,
            Persisted::Written(..) => Saved::default(),
        };
        self.load(&saved);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&Persisted) -> R,
    {
        f(&Persisted::Written(self.list, self.saved()))
    }
}

/// The key of the table in the plug-in's state.
pub const STATE_KEY: &str = "midi_map";

/// A plug-in state's table made good before it is loaded (`Plugin::filter_state`), checked
/// against `list`: a state without one (saved before MIDI Learn, or by a host's preset of
/// another version) gets an empty table, so that loading it into an instance with assignments
/// leaves none; one that is not JSON, or not understood, is an empty table too; one understood
/// is written back checked.
pub fn filter_state(fields: &mut BTreeMap<String, String>, list: &[Learnable]) {
    let saved = fields
        .get(STATE_KEY)
        .map(|t| Saved::from_text(t, list))
        .unwrap_or_default();
    fields.insert(STATE_KEY.to_owned(), saved.to_text(list));
}

/// A parameter as MIDI Learn sets it, whatever its type.
pub trait Target {
    /// The pointer the host knows it by.
    fn ptr(&self) -> ParamPtr;
    /// The normalized value a 7-bit `value` sets it to: through the parameter's own
    /// normalization, a knob value / 127 of its travel, a stepped parameter the position its
    /// share of 0–127 holds (a switch: off below 64).
    fn normalized_for(&self, value: u8) -> f32;
    /// Whether it is at `normalized` already.
    fn at(&self, normalized: f32) -> bool;
    /// How many positions it has (none: continuous).
    fn positions(&self) -> Option<usize>;
}

impl<P: Param> Target for P {
    fn ptr(&self) -> ParamPtr {
        self.as_ptr()
    }

    fn normalized_for(&self, value: u8) -> f32 {
        let value = value.min(127);
        match self.step_count() {
            None => f32::from(value) / 127.0,
            Some(steps) => {
                let n = steps + 1;
                let position = (usize::from(value) * n / 128).min(steps);
                self.preview_normalized(self.preview_plain(position as f32 / steps.max(1) as f32))
            }
        }
    }

    fn at(&self, normalized: f32) -> bool {
        self.preview_plain(normalized) == self.preview_plain(self.unmodulated_normalized_value())
    }

    fn positions(&self) -> Option<usize> {
        self.step_count().map(|s| s + 1)
    }
}

/// The plug-in's parameters as MIDI Learn finds them, by their index in its list.
pub trait LearnTargets {
    /// The learnable parameter of index `i`.
    fn target(&self, i: usize) -> Option<&dyn Target>;
    /// The knob of index `i`, if it is one (the ones [`Dezip`] glides): every
    /// [`Kind::Continuous`] one, and only those.
    fn knob(&self, i: usize) -> Option<&FloatParam>;
}

/// Whether a plug-in's `list` and its parameters (`p`) agree: each id once, each resolving to
/// its parameter, its kind its parameter's steps (two positions a switch, more a selector, none
/// a knob), the knobs, and only they, gliding, each knob the same parameter as its target. The
/// first disagreement, said.
pub fn check<const N: usize>(list: &[Learnable; N], p: &impl LearnTargets) -> Result<(), String> {
    if N > 255 {
        return Err(format!("{N} learnable parameters: at most 255"));
    }
    let mut ids: Vec<&str> = list.iter().map(|l| l.id).collect();
    ids.sort_unstable();
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("{} listed twice", w[0]));
    }
    for (i, l) in list.iter().enumerate() {
        let t = p
            .target(i)
            .ok_or_else(|| format!("{}: no parameter", l.id))?;
        let kind = match t.positions() {
            None => Kind::Continuous,
            Some(2) => Kind::Switch,
            Some(_) => Kind::Stepped,
        };
        if kind != l.kind {
            return Err(format!(
                "{}: listed as {:?}, its parameter {kind:?}",
                l.id, l.kind
            ));
        }
        match (p.knob(i), l.kind == Kind::Continuous) {
            (Some(k), true) if k.as_ptr() == t.ptr() => {}
            (Some(_), true) => return Err(format!("{}: its knob is another parameter", l.id)),
            (None, true) => return Err(format!("{}: a knob, but none glides", l.id)),
            (Some(_), false) => return Err(format!("{}: not a knob, but it glides", l.id)),
            (None, false) => {}
        }
    }
    Ok(())
}

/// A learnable control change's value as the host gives it (0 to 1: nih-plug's `v / 127`) as
/// its 7 bits.
pub fn seven(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// Learnable parameter `i`'s controller at `value` (0 to 1, as the host gives a control change;
/// 7 bits), `timing` samples into the block, on the audio thread: the parameter set through the
/// host, as the host's automation sets it (the editor and the host following, the state holding
/// it), and a knob gliding there in the voices (`dezip`). Whether it changed: a value the
/// parameter already has (a selector's other values within one position, a controller sending
/// the same value) changes nothing and tells the host nothing.
pub fn learned<P: Plugin, const N: usize>(
    params: &impl LearnTargets,
    dezip: &mut Dezip<N>,
    i: usize,
    value: f32,
    timing: u32,
    context: &mut impl ProcessContext<P>,
) -> bool {
    let Some(target) = params.target(i) else {
        return false;
    };
    let normalized = target.normalized_for(seven(value));
    if target.at(normalized) {
        return false;
    }
    let knob = params.knob(i);
    let from = knob.map(|k| dezip.value(k));
    context.set_parameter_normalized(target.ptr(), normalized, timing);
    if let (Some(k), Some(from)) = (knob, from) {
        dezip.start(i, k, from, k.value());
    }
    true
}

/// A MIDI control change on the audio thread, through MIDI Learn: a learnable one to the table
/// (an assigned one setting its parameter: [`learned`]; one caught while learning changing
/// nothing), `Some` with whether it set a parameter; a reserved one noted for the editor while
/// it learns ([`MidiMap::refuse`]) and left to the plug-in's own MIDI path, `None`.
#[allow(clippy::too_many_arguments)]
pub fn control_change<P: Plugin, const N: usize>(
    map: &MidiMap<N>,
    params: &impl LearnTargets,
    dezip: &mut Dezip<N>,
    channel: u8,
    cc: u8,
    value: f32,
    timing: u32,
    context: &mut impl ProcessContext<P>,
) -> Option<bool> {
    if reserved(cc).is_none() {
        return Some(match map.incoming(channel, cc) {
            Incoming::Assigned(i) => learned(params, dezip, i, value, timing, context),
            // Caught for the parameter being learned: the sound is not changed by it.
            Incoming::Caught | Incoming::Unassigned => false,
        });
    }
    map.refuse(cc);
    None
}

/// How long the voices take to follow a knob a learned controller moves, s: a 7-bit controller
/// moves a knob in steps of 1/127 of its travel, which the voices would otherwise take as steps
/// (the CA-72's MAIN OUTPUT VOLUME clicked: +6.6 dB above 8 kHz, its R34). The parameter itself
/// (the host's, the editor's, the state's) is at the new value at once.
pub const DEZIP: f64 = 0.010;

/// While a knob glides, the voices' controls are set again every this many samples.
pub const DEZIP_STEP: usize = 32;

/// A knob gliding to where a learned controller put it: the knob (its parameter's address, to
/// know it by), from and to (its plain values), and the samples gone and in all.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    knob: usize,
    from: f32,
    to: f32,
    done: u32,
    len: u32,
}

impl Glide {
    fn now(&self) -> f32 {
        if self.done >= self.len {
            self.to
        } else {
            self.from + (self.to - self.from) * (self.done as f32 / self.len as f32)
        }
    }
}

/// The knobs that learned controllers move, gliding over [`DEZIP`] to their parameters' values
/// (the voices only: the parameters are at the new values at once), for a list of `N`. Only
/// these knobs glide, and only after a learned change: the host's automation, the editor and
/// presets set the voices as they always have. A glide ends early where anything else sets the
/// parameter meanwhile.
#[derive(Clone, Debug)]
pub struct Dezip<const N: usize> {
    glides: [Option<Glide>; N],
    moving: usize,
    len: u32,
}

impl<const N: usize> Default for Dezip<N> {
    fn default() -> Self {
        Dezip {
            glides: [None; N],
            moving: 0,
            len: 1,
        }
    }
}

impl<const N: usize> Dezip<N> {
    /// The glides' length at `rate` Hz.
    pub fn prepare(&mut self, rate: f64) {
        self.len = (DEZIP * rate).round().max(1.0) as u32;
        self.clear();
    }

    /// No glide.
    pub fn clear(&mut self) {
        self.glides = [None; N];
        self.moving = 0;
    }

    /// Whether a knob glides.
    pub fn moving(&self) -> bool {
        self.moving > 0
    }

    /// Learnable knob `i`, `knob`, set from `from` (where the voices had it) to `to`: it glides
    /// there, from where it is if it was gliding.
    pub fn start(&mut self, i: usize, knob: &FloatParam, from: f32, to: f32) {
        let Some(slot) = self.glides.get_mut(i) else {
            return;
        };
        let from = slot.map_or(from, |g| g.now());
        if slot.is_none() {
            self.moving += 1;
        }
        *slot = Some(Glide {
            knob: std::ptr::from_ref(knob) as usize,
            from,
            to,
            done: 0,
            len: self.len.max(1),
        });
    }

    /// `n` samples on; a glide whose knob's parameter has been set elsewhere meanwhile (not at
    /// its end) ends.
    pub fn advance(&mut self, n: usize, p: &impl LearnTargets) {
        if self.moving == 0 {
            return;
        }
        for (i, slot) in self.glides.iter_mut().enumerate() {
            let Some(g) = slot else { continue };
            g.done = g.done.saturating_add(n as u32);
            let set_elsewhere = p.knob(i).is_none_or(|k| k.value() != g.to);
            if g.done >= g.len || set_elsewhere {
                *slot = None;
                self.moving -= 1;
            }
        }
    }

    /// The glides ended where anything other than a learned controller set their knobs: at a
    /// run's start, before the voices' controls are taken.
    pub fn follow(&mut self, p: &impl LearnTargets) {
        self.advance(0, p);
    }

    /// Where the voices have `knob` now: its glide's place, else its value.
    pub fn value(&self, knob: &FloatParam) -> f32 {
        if self.moving > 0 {
            let at = std::ptr::from_ref(knob) as usize;
            for g in self.glides.iter().flatten() {
                if g.knob == at {
                    return g.now();
                }
            }
        }
        knob.value()
    }
}

#[cfg(test)]
mod tests;
