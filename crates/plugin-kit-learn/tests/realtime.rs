//! MIDI Learn on the audio thread neither allocates nor frees (the CA-72's R34): learned
//! controllers setting knobs (gliding), a switch, a selector and a count through the host, a
//! controller caught while learning, the reserved ones refused, the host's automation ending a
//! glide, counted by a global allocator on this thread. (The stand-in host's record of the
//! changes is reserved first, as the kit's nih-plug reserves its wrappers'; its events are made
//! off the count.) A failure names where the first came from.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use nih_plug::context::process::TestProcessContext;
use nih_plug::prelude::*;
use plugin_kit_learn::{
    Cc, DEZIP_STEP, Dezip, LearnTargets, Learnable, MidiMap, Target, control_change, index, knob,
    stepped, switch,
};

struct Counting;
static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
static FIRST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
thread_local! {
    static HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Counts one call on the armed thread, recording the first's backtrace (disarmed meanwhile,
/// so the recording itself is not counted).
fn count(counter: &AtomicUsize) {
    if HERE.with(std::cell::Cell::get) && ARMED.swap(false, Ordering::SeqCst) {
        counter.fetch_add(1, Ordering::SeqCst);
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        if let Ok(mut f) = FIRST.lock() {
            f.get_or_insert(bt);
        }
        ARMED.store(true, Ordering::SeqCst);
    }
}

// SAFETY: forwards to the system allocator; only counts (and records a backtrace) while armed.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(&ALLOCS);
        // SAFETY: the caller's contract is passed through.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(&FREES);
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
enum Wave {
    #[id = "tri"]
    Triangle,
    #[id = "saw"]
    Saw,
    #[id = "square"]
    Square,
}

#[derive(Params)]
struct P {
    #[persist = "midi_map"]
    midi_map: Arc<MidiMap<5>>,
    #[id = "cutoff"]
    cutoff: FloatParam,
    #[id = "emphasis"]
    emphasis: FloatParam,
    #[id = "on"]
    on: BoolParam,
    #[id = "wave"]
    wave: EnumParam<Wave>,
    #[id = "voices"]
    voices: IntParam,
}

static LIST: [Learnable; 5] = [
    knob("cutoff", "CUTOFF"),
    knob("emphasis", "EMPHASIS"),
    switch("on", "ON"),
    stepped("wave", "WAVE"),
    stepped("voices", "VOICES"),
];

impl Default for P {
    fn default() -> Self {
        let linear = FloatRange::Linear {
            min: 0.0,
            max: 10.0,
        };
        P {
            midi_map: Arc::new(MidiMap::new(&LIST)),
            cutoff: FloatParam::new("Cutoff", 5.0, linear),
            emphasis: FloatParam::new("Emphasis", 0.0, linear),
            on: BoolParam::new("On", true),
            wave: EnumParam::new("Wave", Wave::Saw),
            voices: IntParam::new("Voices", 4, IntRange::Linear { min: 2, max: 10 }),
        }
    }
}

impl LearnTargets for P {
    fn target(&self, i: usize) -> Option<&dyn Target> {
        Some(match i {
            0 => &self.cutoff,
            1 => &self.emphasis,
            2 => &self.on,
            3 => &self.wave,
            4 => &self.voices,
            _ => return None,
        })
    }

    fn knob(&self, i: usize) -> Option<&FloatParam> {
        Some(match i {
            0 => &self.cutoff,
            1 => &self.emphasis,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct Plug {
    params: Arc<P>,
}

impl Plugin for Plug {
    const NAME: &'static str = "Test";
    const VENDOR: &'static str = "Test";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0.0.0";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

#[test]
fn midi_learn_neither_allocates_nor_frees() {
    let p = Arc::new(P::default());
    plugin_kit_learn::check(&LIST, &*p).unwrap();
    let mut c = TestProcessContext::<Plug>::new(p.clone(), 48_000.0, ProcessMode::Realtime);
    c.reported.reserve(1 << 16);
    let mut dezip = Dezip::<5>::default();
    dezip.prepare(48_000.0);
    let map = Arc::clone(&p.midi_map);
    for (id, cc) in [
        ("cutoff", 74),
        ("emphasis", 71),
        ("on", 20),
        ("wave", 21),
        ("voices", 22),
    ] {
        map.assign(index(&LIST, id).unwrap(), Cc { channel: 0, cc })
            .unwrap();
    }
    let cutoff = p.cutoff.as_ptr();
    let mut sink = 0.0f32;
    HERE.with(|h| h.set(true));
    ARMED.store(true, Ordering::SeqCst);
    for b in 0..400usize {
        let mut events: [(u32, u8, f32); 11] = [(0, 0, 0.0); 11];
        for k in 0..4usize {
            let v = ((b as f32 * 4.0 + k as f32) * 0.07).sin() * 0.5 + 0.5;
            events[2 * k] = (k as u32 * 30, 74, v);
            events[2 * k + 1] = (k as u32 * 30 + 3, 71, 1.0 - v);
        }
        events[8] = (5, 20, if b % 6 < 3 { 0.0 } else { 1.0 });
        events[9] = (6, 21 + (b % 2) as u8, (b % 9) as f32 / 8.0);
        events[10] = (9, if b % 2 == 0 { 1 } else { 100 }, 0.5);
        events.sort_unstable_by_key(|x| x.0);
        if b % 40 == 10 {
            // Learning EMPHASIS (the editor's), and a controller caught for it.
            map.arm(1);
            control_change(
                &map,
                &*p,
                &mut dezip,
                0,
                30 + (b / 40) as u8,
                0.5,
                11,
                &mut c,
            );
        }
        if b % 40 == 11 {
            map.poll();
        }
        if b % 30 == 29 {
            // The host's automation of CUTOFF between blocks ends its glide.
            c.automate(cutoff, 0.25);
        }
        // A block of 128 samples: its events in order, the voices' knobs where the glides have
        // them every `DEZIP_STEP` samples.
        dezip.follow(&*p);
        let mut i = 0usize;
        let mut e = 0usize;
        while i < 128 {
            while e < events.len() && events[e].0 as usize <= i {
                let (timing, cc, value) = events[e];
                control_change(&map, &*p, &mut dezip, 0, cc, value, timing, &mut c);
                e += 1;
            }
            dezip.follow(&*p);
            let next = events.get(e).map_or(128, |x| x.0 as usize);
            let end = next.min(128).min(i + DEZIP_STEP).max(i + 1);
            sink += dezip.value(&p.cutoff) + dezip.value(&p.emphasis);
            dezip.advance(end - i, &*p);
            i = end;
        }
    }
    ARMED.store(false, Ordering::SeqCst);
    let (allocs, frees) = (ALLOCS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst));
    let first = FIRST
        .lock()
        .ok()
        .and_then(|f| f.clone())
        .unwrap_or_default();
    assert_eq!(
        (allocs, frees),
        (0, 0),
        "MIDI Learn allocated {allocs} and freed {frees} times while playing; the first:\n{first}"
    );
    assert!(sink.is_finite());
    assert!(c.reported.len() > 1000, "{} changes told", c.reported.len());
}
