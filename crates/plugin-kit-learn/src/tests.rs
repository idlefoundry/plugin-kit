//! The CA-72's tests of its MIDI Learn (its R34), on a plug-in of the tests' own: a few of its
//! parameters of each kind, under its ids.

use std::sync::Arc;

use nih_plug::context::process::TestProcessContext;

use super::*;

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Range {
    #[id = "lo"]
    Lo,
    #[id = "32"]
    R32,
    #[id = "16"]
    R16,
    #[id = "8"]
    R8,
    #[id = "4"]
    R4,
    #[id = "2"]
    R2,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Noise {
    #[id = "white"]
    White,
    #[id = "pink"]
    Pink,
}

/// A plug-in's parameters: knobs, a switch, a two-way and a six-way selector, a count, and two
/// that are not learned (a wheel, the host's bypass).
#[derive(Params)]
pub(crate) struct TestParams {
    #[persist = "midi_map"]
    pub midi_map: Arc<MidiMap<8>>,
    #[id = "tune"]
    pub tune: FloatParam,
    #[id = "glide"]
    pub glide: FloatParam,
    #[id = "cutoff"]
    pub cutoff: FloatParam,
    #[id = "emphasis"]
    pub emphasis: FloatParam,
    #[id = "osc1_on"]
    pub osc1_on: BoolParam,
    #[id = "noise_type"]
    pub noise_type: EnumParam<Noise>,
    #[id = "osc1_range"]
    pub osc1_range: EnumParam<Range>,
    #[id = "voices"]
    pub voices: IntParam,
    #[id = "pitch_wheel"]
    pub pitch_wheel: FloatParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
}

/// What may be learned of them.
pub(crate) static LIST: [Learnable; 8] = [
    knob("tune", "TUNE"),
    knob("glide", "GLIDE"),
    knob("cutoff", "CUTOFF FREQUENCY"),
    knob("emphasis", "EMPHASIS"),
    switch("osc1_on", "OSCILLATOR-1 ON"),
    switch("noise_type", "NOISE (WHITE or PINK)"),
    stepped("osc1_range", "OSCILLATOR-1 RANGE"),
    stepped("voices", "VOICES"),
];

impl Default for TestParams {
    fn default() -> Self {
        let linear = |min, max| FloatRange::Linear { min, max };
        TestParams {
            midi_map: Arc::new(MidiMap::new(&LIST)),
            tune: FloatParam::new("Tune", 0.0, linear(-2.5, 2.5)),
            glide: FloatParam::new("Glide", 0.0, linear(0.0, 10.0)),
            cutoff: FloatParam::new("Cutoff", 0.0, linear(-5.0, 5.0)),
            emphasis: FloatParam::new("Emphasis", 0.0, linear(0.0, 10.0)),
            osc1_on: BoolParam::new("Oscillator 1 On", true),
            noise_type: EnumParam::new("Noise", Noise::White),
            osc1_range: EnumParam::new("Oscillator 1 Range", Range::R8),
            voices: IntParam::new("Voices", 4, IntRange::Linear { min: 2, max: 10 }),
            pitch_wheel: FloatParam::new("Pitch Wheel", 0.0, linear(-1.0, 1.0)),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
        }
    }
}

impl LearnTargets for TestParams {
    fn target(&self, i: usize) -> Option<&dyn Target> {
        Some(match LIST.get(i)?.id {
            "tune" => &self.tune,
            "glide" => &self.glide,
            "cutoff" => &self.cutoff,
            "emphasis" => &self.emphasis,
            "osc1_on" => &self.osc1_on,
            "noise_type" => &self.noise_type,
            "osc1_range" => &self.osc1_range,
            "voices" => &self.voices,
            _ => return None,
        })
    }

    fn knob(&self, i: usize) -> Option<&FloatParam> {
        Some(match LIST.get(i)?.id {
            "tune" => &self.tune,
            "glide" => &self.glide,
            "cutoff" => &self.cutoff,
            "emphasis" => &self.emphasis,
            _ => return None,
        })
    }
}

/// A plug-in for the context the parameters are set through.
#[derive(Default)]
pub(crate) struct TestPlugin {
    pub params: Arc<TestParams>,
}

impl Plugin for TestPlugin {
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

fn cc(channel: u8, cc: u8) -> Cc {
    Cc { channel, cc }
}

fn at(id: &str) -> usize {
    index(&LIST, id).expect("learnable")
}

fn map() -> MidiMap<8> {
    MidiMap::new(&LIST)
}

/// The list and the parameters agree, and a list that does not is told why: an id twice, a
/// kind not its parameter's, a knob that does not glide, a parameter that is not there.
#[test]
fn a_list_is_checked_against_its_parameters() {
    let p = TestParams::default();
    assert_eq!(check(&LIST, &p), Ok(()));
    for id in ["pitch_wheel", "bypass"] {
        assert_eq!(index(&LIST, id), None, "{id} is not learnable");
    }
    let mut twice = LIST;
    twice[1] = knob("tune", "TUNE");
    assert!(check(&twice, &p).is_err_and(|e| e.contains("tune listed twice")));
    let mut kind = LIST;
    kind[4] = stepped("osc1_on", "OSCILLATOR-1 ON");
    assert!(check(&kind, &p).is_err_and(|e| e.contains("osc1_on")));
    let mut switch_as_knob = LIST;
    switch_as_knob[4] = knob("osc1_on", "OSCILLATOR-1 ON");
    assert!(check(&switch_as_knob, &p).is_err());
    // A plug-in whose parameters lack one of its list's.
    struct Lacking<'a>(&'a TestParams);
    impl LearnTargets for Lacking<'_> {
        fn target(&self, i: usize) -> Option<&dyn Target> {
            (i != 0).then(|| self.0.target(i)).flatten()
        }
        fn knob(&self, i: usize) -> Option<&FloatParam> {
            self.0.knob(i)
        }
    }
    assert!(check(&LIST, &Lacking(&p)).is_err_and(|e| e.contains("tune: no parameter")));
}

/// Bank select, the modulation wheel, data entry and RPN/NRPN, and the channel mode messages
/// are not learned; every other control change is.
#[test]
fn the_reserved_controllers_are_not_learned() {
    let reserved_ccs: Vec<u8> = (0..=127).filter(|&c| reserved(c).is_some()).collect();
    let mut want = vec![0, 1, 6, 32, 38, 96, 97, 98, 99, 100, 101];
    want.extend(120..=127);
    assert_eq!(reserved_ccs, want);
    let m = map();
    for &c in &want {
        assert_eq!(m.assign(0, cc(0, c)), None, "CC {c}");
    }
    m.arm(at("cutoff"));
    assert_eq!(m.incoming(0, 1), Incoming::Unassigned);
    assert_eq!(m.armed(), Some(at("cutoff")), "a reserved CC is not caught");
    m.refuse(1);
    assert_eq!(m.refused(), Some(1));
    m.arm(at("emphasis"));
    assert_eq!(m.refused(), None, "said for the arming it came in");
    m.cancel();
    m.refuse(7);
    assert_eq!(m.refused(), None, "nothing said while not learning");
}

/// A knob: value / 127 of its travel. A switch: off at 0–63, on at 64–127. A selector: its six
/// positions each an equal share of 0–127; VOICES its nine.
#[test]
fn a_value_sets_each_kind_through_its_parameters_normalization() {
    let p = TestParams::default();
    let cutoff = p.target(at("cutoff")).expect("a knob");
    assert_eq!(cutoff.normalized_for(0), 0.0);
    assert_eq!(cutoff.normalized_for(127), 1.0);
    assert_eq!(cutoff.normalized_for(64), 64.0 / 127.0);
    let on = p.target(at("osc1_on")).expect("a switch");
    assert_eq!(on.normalized_for(63), 0.0);
    assert_eq!(on.normalized_for(64), 1.0);
    let pink = p.target(at("noise_type")).expect("a two-way switch");
    assert_eq!(
        (pink.normalized_for(63), pink.normalized_for(64)),
        (0.0, 1.0)
    );
    let range = p.target(at("osc1_range")).expect("a selector");
    let position = |v: u8| (range.normalized_for(v) * 5.0).round() as u8;
    let firsts: Vec<u8> = (1..=127)
        .filter(|&v| position(v) != position(v - 1))
        .collect();
    assert_eq!(firsts, [22, 43, 64, 86, 107]);
    assert_eq!((position(0), position(127)), (0, 5));
    let voices = p.target(at("voices")).expect("VOICES");
    let plain = |v: u8| p.voices.preview_plain(voices.normalized_for(v));
    assert_eq!((plain(0), plain(127)), (2, 10));
    let mut seen: Vec<i32> = (0..=127).map(plain).collect();
    seen.dedup();
    assert_eq!(seen, (2..=10).collect::<Vec<_>>());
    // The host's value back to its 7 bits.
    assert_eq!((seven(0.0), seven(64.0 / 127.0), seven(1.0)), (0, 64, 127));
    assert_eq!((seven(-1.0), seven(2.0)), (0, 127));
}

/// Learning: the next learnable controller, on its channel, is caught (changing nothing), and
/// assigned at the editor's next look; what follows sets the parameter.
#[test]
fn learning_catches_the_next_controller_and_assigns_it() {
    let m = map();
    let cutoff = at("cutoff");
    m.arm(cutoff);
    assert_eq!(m.armed(), Some(cutoff));
    assert_eq!(m.incoming(1, 74), Incoming::Caught);
    assert_eq!(m.armed(), None);
    // (Before the editor assigns it, the knob's next moves do nothing.)
    assert_eq!(m.incoming(1, 74), Incoming::Unassigned);
    let a = m.poll().expect("assigned");
    assert_eq!(
        a,
        Assigned {
            param: cutoff,
            cc: cc(1, 74),
            replaced: None,
            displaced: None
        }
    );
    assert_eq!(m.poll(), None, "once");
    assert_eq!(m.incoming(1, 74), Incoming::Assigned(cutoff));
    assert_eq!(m.assignment(cutoff), Some(cc(1, 74)));
    assert_eq!(cc(1, 74).text(), "CH 2 · CC 74");
}

/// Cancelling keeps every assignment; a controller caught for an arming since replaced or
/// cancelled is not assigned.
#[test]
fn cancelling_keeps_the_assignments() {
    let m = map();
    let (cutoff, emphasis) = (at("cutoff"), at("emphasis"));
    m.assign(emphasis, cc(0, 71));
    m.arm(cutoff);
    m.cancel();
    assert_eq!(m.armed(), None);
    assert_eq!(m.incoming(0, 72), Incoming::Unassigned, "not caught");
    assert_eq!(m.incoming(0, 71), Incoming::Assigned(emphasis));
    m.arm(cutoff);
    assert_eq!(m.incoming(0, 72), Incoming::Caught);
    m.cancel();
    assert_eq!(m.poll(), None, "caught, then cancelled: not assigned");
    assert_eq!(m.saved(), Saved(vec![(emphasis, cc(0, 71))]));
}

/// Arming another control moves the learning to it.
#[test]
fn arming_another_control_moves_the_learning() {
    let m = map();
    m.arm(at("cutoff"));
    m.arm(at("emphasis"));
    assert_eq!(m.incoming(3, 20), Incoming::Caught);
    assert_eq!(m.poll().map(|a| a.param), Some(at("emphasis")));
    assert_eq!(m.assignment(at("cutoff")), None);
    // Caught for one, then another armed before the editor looked: the other waits.
    m.arm(at("glide"));
    assert_eq!(m.incoming(3, 21), Incoming::Caught);
    m.arm(at("tune"));
    assert_eq!(m.poll(), None);
    assert_eq!(m.armed(), Some(at("tune")));
}

/// One controller a parameter (relearning replaces it), one parameter a controller (reusing one
/// moves it, and says from where); removing one leaves the others.
#[test]
fn relearning_replaces_and_reusing_moves() {
    let m = map();
    let (cutoff, emphasis) = (at("cutoff"), at("emphasis"));
    m.assign(cutoff, cc(0, 74));
    let a = m.assign(cutoff, cc(0, 75)).expect("assigned");
    assert_eq!((a.replaced, a.displaced), (Some(cc(0, 74)), None));
    assert_eq!(m.target(cc(0, 74)), None);
    let b = m.assign(emphasis, cc(0, 75)).expect("assigned");
    assert_eq!((b.replaced, b.displaced), (None, Some(cutoff)));
    assert_eq!(m.assignment(cutoff), None);
    assert_eq!(m.assignment(emphasis), Some(cc(0, 75)));
    assert_eq!(
        m.assign(emphasis, cc(0, 75))
            .map(|a| (a.replaced, a.displaced)),
        Some((None, None))
    );
    assert_eq!(m.remove(emphasis), Some(cc(0, 75)));
    assert_eq!(m.remove(emphasis), None);
    assert_eq!(m.incoming(0, 75), Incoming::Unassigned);
    // Nothing outside the list.
    assert_eq!(m.assign(LIST.len(), cc(0, 9)), None);
}

/// The same controller number on two channels is two controllers.
#[test]
fn channels_are_told_apart() {
    let m = map();
    let (cutoff, emphasis) = (at("cutoff"), at("emphasis"));
    m.assign(cutoff, cc(0, 74));
    assert_eq!(m.incoming(1, 74), Incoming::Unassigned);
    m.assign(emphasis, cc(1, 74));
    assert_eq!(m.incoming(0, 74), Incoming::Assigned(cutoff));
    assert_eq!(m.incoming(1, 74), Incoming::Assigned(emphasis));
    assert_eq!(m.incoming(15, 74), Incoming::Unassigned);
}

/// Saved as JSON by id, the channel 1 to 16, and read back the same.
#[test]
fn the_table_is_saved_by_id_and_read_back() {
    let m = map();
    m.assign(at("emphasis"), cc(15, 71));
    m.assign(at("cutoff"), cc(0, 74));
    m.assign(at("voices"), cc(2, 20));
    let text = m.saved().to_text(&LIST);
    assert_eq!(
        text,
        r#"{"version":1,"assignments":[{"param":"cutoff","channel":1,"cc":74},{"param":"emphasis","channel":16,"cc":71},{"param":"voices","channel":3,"cc":20}]}"#
    );
    let back = map();
    back.load(&Saved::from_text(&text, &LIST));
    assert_eq!(back.assignments(), m.assignments());
    assert_eq!(
        map().saved().to_text(&LIST),
        r#"{"version":1,"assignments":[]}"#
    );
}

/// A table read leniently: another version or no object is an empty table; an entry of an
/// unknown or unlearnable parameter, a channel or controller out of range or reserved, a value
/// of the wrong type, or a parameter or controller already assigned above it, is left out (the
/// first one kept).
#[test]
fn malformed_duplicate_and_unknown_entries_are_left_out() {
    for text in [
        "",
        "garbage",
        "[]",
        "42",
        r#"{"assignments":[{"param":"cutoff","channel":1,"cc":74}]}"#,
        r#"{"version":2,"assignments":[{"param":"cutoff","channel":1,"cc":74}]}"#,
        r#"{"version":"1","assignments":[{"param":"cutoff","channel":1,"cc":74}]}"#,
        r#"{"version":1,"assignments":{"param":"cutoff","channel":1,"cc":74}}"#,
    ] {
        assert_eq!(Saved::from_text(text, &LIST), Saved::default(), "{text}");
    }
    let text = r#"{"version":1,"extra":true,"assignments":[
        {"param":"cutoff","channel":1,"cc":74},
        {"param":"cutoff","channel":1,"cc":75},
        {"param":"emphasis","channel":1,"cc":74},
        {"param":"emphasis","channel":2,"cc":74,"later":"ignored"},
        {"param":"no_such","channel":1,"cc":10},
        {"param":"pitch_wheel","channel":1,"cc":11},
        {"param":"bypass","channel":1,"cc":12},
        {"param":"glide","channel":0,"cc":14},
        {"param":"glide","channel":17,"cc":14},
        {"param":"glide","channel":1,"cc":128},
        {"param":"glide","channel":1,"cc":-1},
        {"param":"glide","channel":"1","cc":14},
        {"param":"glide","channel":1,"cc":1},
        {"param":"glide","channel":1,"cc":120},
        {"param":"glide","channel":1},
        "nonsense",
        {"param":"tune","channel":16,"cc":127},
        {"param":"tune","channel":16,"cc":119}
    ]}"#;
    assert_eq!(
        Saved::from_text(text, &LIST),
        Saved(vec![
            (at("tune"), cc(15, 119)),
            (at("cutoff"), cc(0, 74)),
            (at("emphasis"), cc(1, 74)),
        ])
    );
}

/// Loaded over an instance's assignments, a table replaces every one of them; an armed learning
/// is never part of what is saved.
#[test]
fn a_table_loaded_replaces_every_assignment_and_learning_is_not_saved() {
    let m = map();
    m.assign(at("cutoff"), cc(0, 74));
    m.arm(at("glide"));
    let saved = m.saved();
    assert_eq!(saved, Saved(vec![(at("cutoff"), cc(0, 74))]));
    let text = saved.to_text(&LIST);
    assert!(!text.contains("glide"), "{text}");
    m.load(&Saved(vec![(at("emphasis"), cc(2, 3))]));
    assert_eq!(m.assignment(at("cutoff")), None);
    assert_eq!(m.assignment(at("emphasis")), Some(cc(2, 3)));
    m.load(&Saved::default());
    assert!(m.assignments().iter().all(Option::is_none));
}

/// A state's table made good before it loads: none (saved before MIDI Learn) or not understood
/// is an empty one; one understood is written back checked.
#[test]
fn a_states_table_is_made_good_before_it_loads() {
    let empty = r#"{"version":1,"assignments":[]}"#;
    let mut fields = BTreeMap::new();
    filter_state(&mut fields, &LIST);
    assert_eq!(fields.get(STATE_KEY).map(String::as_str), Some(empty));
    fields.insert(STATE_KEY.into(), "{not json".into());
    filter_state(&mut fields, &LIST);
    assert_eq!(fields.get(STATE_KEY).map(String::as_str), Some(empty));
    fields.insert(
        STATE_KEY.into(),
        r#"{"version":1,"assignments":[{"param":"cutoff","channel":1,"cc":74},{"param":"x","channel":1,"cc":2}]}"#.into(),
    );
    filter_state(&mut fields, &LIST);
    assert_eq!(
        fields.get(STATE_KEY).map(String::as_str),
        Some(r#"{"version":1,"assignments":[{"param":"cutoff","channel":1,"cc":74}]}"#)
    );
}

/// The table through nih-plug's persistent fields, as a state saves and loads it: written by id,
/// read back over the assignments an instance had; a broken table is none.
#[test]
fn the_table_goes_with_the_plug_ins_state() {
    let p = TestParams::default();
    p.midi_map.assign(at("cutoff"), cc(0, 74));
    p.midi_map.assign(at("voices"), cc(3, 22));
    let fields = p.serialize_fields();
    assert_eq!(
        fields.get(STATE_KEY).map(String::as_str),
        Some(
            r#"{"version":1,"assignments":[{"param":"cutoff","channel":1,"cc":74},{"param":"voices","channel":4,"cc":22}]}"#
        )
    );
    let q = TestParams::default();
    q.midi_map.assign(at("glide"), cc(0, 5));
    q.deserialize_fields(&fields);
    assert_eq!(q.midi_map.saved(), p.midi_map.saved());
    let mut broken = fields.clone();
    broken.insert(STATE_KEY.into(), r#"{"version":7}"#.into());
    q.deserialize_fields(&broken);
    assert_eq!(q.midi_map.saved(), Saved::default());
}

/// A knob glides over `DEZIP` to the value a learned controller set, from where the voices had
/// it (from where it was gliding, if it was), and stops gliding when anything else sets its
/// parameter.
#[test]
fn a_knob_glides_to_a_learned_value_and_stops_where_set_elsewhere() {
    let p = TestParams::default();
    let i = at("cutoff");
    let k = p.knob(i).expect("a knob");
    let mut d = Dezip::<8>::default();
    d.prepare(48_000.0);
    assert_eq!(d.value(k), 0.0);
    // (The parameter itself stays at its default here: as if set elsewhere unless `to` is its
    // value.)
    d.start(i, k, 4.0, 0.0);
    assert!(d.moving());
    assert_eq!(d.value(k), 4.0);
    d.advance(240, &p);
    assert!((d.value(k) - 2.0).abs() < 1e-5, "{}", d.value(k));
    d.advance(240, &p);
    assert!(!d.moving());
    assert_eq!(d.value(k), 0.0);
    // Set elsewhere meanwhile (its value is not the glide's end): the glide ends.
    d.start(i, k, 0.0, 3.0);
    d.follow(&p);
    assert!(!d.moving());
    assert_eq!(d.value(k), k.value());
    // A second change mid-glide glides on from where it was.
    d.start(i, k, 4.0, 0.0);
    d.advance(120, &p);
    let mid = d.value(k);
    d.start(i, k, 99.0, 0.0);
    assert_eq!(d.value(k), mid);
}

/// Through the audio thread's path: a learned controller sets its parameter at its event's
/// sample through the host, told once, a knob gliding; the same value again tells nothing; a
/// controller caught while learning changes nothing; a reserved one is the plug-in's own.
#[test]
fn a_learned_controller_sets_its_parameter_through_the_host() {
    let p = Arc::new(TestParams::default());
    let mut c = TestProcessContext::<TestPlugin>::new(p.clone(), 48_000.0, ProcessMode::Realtime);
    let mut d = Dezip::<8>::default();
    d.prepare(48_000.0);
    let m = &p.midi_map;
    m.assign(at("cutoff"), cc(0, 74));
    m.assign(at("osc1_range"), cc(0, 21));
    let mut change = |cc: u8, value: f32, timing: u32| {
        control_change(m, &*p, &mut d, 0, cc, value, timing, &mut c)
    };
    assert_eq!(change(74, 1.0, 17), Some(true));
    assert_eq!(change(74, 1.0, 18), Some(false), "the same value");
    // A selector's other values within one position change nothing.
    assert_eq!(change(21, 0.0, 19), Some(true));
    assert_eq!(change(21, 10.0 / 127.0, 20), Some(false));
    assert_eq!(change(9, 0.5, 21), Some(false), "unassigned");
    assert_eq!(change(1, 0.5, 22), None, "the modulation wheel");
    m.arm(at("emphasis"));
    assert_eq!(change(71, 0.9, 23), Some(false), "caught");
    assert_eq!(p.emphasis.value(), 0.0);
    assert_eq!(
        c.reported,
        vec![
            (p.cutoff.as_ptr(), 1.0, 17),
            (p.osc1_range.as_ptr(), 0.0, 19)
        ]
    );
    assert_eq!(p.cutoff.value(), 5.0);
    assert!(d.moving());
    assert_eq!(d.value(&p.cutoff), 0.0, "the voices from where they were");
    assert_eq!(m.poll().map(|a| a.cc), Some(cc(0, 71)));
}
