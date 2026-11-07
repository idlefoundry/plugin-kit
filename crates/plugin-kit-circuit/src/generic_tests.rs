//! The engine on small networks that belong to no instrument, each against its closed form: the
//! pipeline (parts, network, operating point, state space, modes, stepper) as an instrument uses
//! it.

use crate::dc::operating_point;
use crate::devices::{Bjt, Diode, Jfet, Library, Mos, fixtures};
use crate::net::{Circuit, Net, Opts};
use crate::part::{Kind, Part};
use crate::run::Stepper;
use crate::ss::{Modal, System};

/// A part in section `t`, its local nodes written `t_name` and `@name` global, as an
/// instrument's builder writes them.
fn part(refdes: &str, kind: Kind, nodes: &[&str]) -> Part {
    Part {
        refdes: refdes.to_string(),
        kind,
        nodes: nodes
            .iter()
            .map(|n| match n.strip_prefix('@') {
                Some(g) => g.to_string(),
                None if *n == "0" => "0".to_string(),
                None => format!("t_{n}"),
            })
            .collect(),
        section: "t".to_string(),
        block: "",
        pins: "",
        note: "",
    }
}

fn opts(sources: &[&str]) -> Opts {
    Opts {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn an_rc_low_pass_steps_exactly() {
    let (r, c) = (1e3, 1e-6);
    let parts = [
        part("R1", Kind::R(r), &["@in", "out"]),
        part("C1", Kind::C(c), &["out", "0"]),
    ];
    let net = Net::build(&parts, &opts(&["in"]), &Circuit::default());
    let u0 = [1.0, 0.0];
    let op = operating_point(&net, &u0).expect("operating point");
    let sys = System::new(&net, &op, &u0, &["t_out"]);
    let h = 1.0 / 48_000.0;
    let mut s = Stepper::new(&sys, Modal::new(&sys), h);
    let tau = r * c;
    // The input steps to 1 V at the first sample's end; the stepper holds it first-order over
    // the step (a ramp from 0 to 1 V over the first sample), then 1 V: the exact response is
    // 1 - (tau / h)(1 - e^(-h / tau)) e^(-(t - h) / tau) from t = h on.
    let mut worst: f64 = 0.0;
    for k in 1..=480 {
        s.step(&[1.0, 1.0]);
        let t = k as f64 * h;
        let exact = 1.0 - tau / h * (1.0 - (-h / tau).exp()) * (-(t - h) / tau).exp();
        let out = *s.y.last().expect("the probe");
        assert!(out.is_finite());
        worst = worst.max((out - exact).abs());
    }
    assert!(worst < 1e-9, "RC response {worst:e} V from its closed form");
}

#[test]
fn a_junction_to_a_negative_rail_starts_as_ngspice_starts_it() {
    // A diode from a node to a -15 V rail, the node held up by a resistor to ground: from 0 V
    // the junction starts 15 V forward. Started at the nodes, Newton comes down one thermal
    // voltage an iteration; started as ngspice's MODEINITJCT starts it, in a few.
    static DIODES: [&Diode; 1] = [&fixtures::D1S2473];
    let d = &fixtures::D1S2473;
    let r = 1e3;
    let parts = [
        part("R1", Kind::R(r), &["a", "0"]),
        part("D1", Kind::D(d.name), &["a", "@vn"]),
    ];
    let circuit = Circuit {
        library: Library {
            diodes: &DIODES,
            ..Default::default()
        },
        rails: vec![("vn".to_string(), -15.0)],
    };
    let net = Net::build(&parts, &opts(&[]), &circuit);
    let u = [1.0];
    let nodes = operating_point(&net, &u).expect("from the nodes");
    let junctions = crate::dc::operating_point_with(&net, &u, crate::dc::Init::Junctions)
        .expect("from the junctions");
    assert!(
        junctions.iters < 30,
        "{} iterations from the junctions",
        junctions.iters
    );
    assert!(
        nodes.iters > junctions.iters,
        "{} from the nodes",
        nodes.iters
    );
    let a = |op: &crate::dc::Op| op.x[net.names.iter().position(|n| n == "t_a").expect("t_a")];
    let vt = crate::dev::vt();
    let i = -a(&junctions) / r;
    let vd = (d.n * vt) * (i / d.is + 1.0).ln() + i * d.rs;
    assert!(
        (a(&junctions) - (-15.0 + vd)).abs() < 1e-9,
        "the anode at {} V, the diode equation's {} V",
        a(&junctions),
        -15.0 + vd
    );
    assert!((a(&nodes) - a(&junctions)).abs() < 1e-9);
}

#[test]
fn a_diode_clippers_operating_point_is_the_diode_equation() {
    static DIODES: [&Diode; 1] = [&fixtures::D1S2473];
    let d = &fixtures::D1S2473;
    let rser = 1e3;
    let parts = [
        part("R1", Kind::R(rser), &["@in", "a"]),
        part("D1", Kind::D(d.name), &["a", "0"]),
    ];
    let circuit = Circuit {
        library: Library {
            diodes: &DIODES,
            ..Default::default()
        },
        rails: Vec::new(),
    };
    let net = Net::build(&parts, &opts(&["in"]), &circuit);
    let vt = crate::dev::vt();
    for vin in [0.3, 1.0, 5.0, 15.0] {
        let u = [1.0, vin];
        let op = operating_point(&net, &u).expect("operating point");
        let sys = System::new(&net, &op, &u, &["t_a"]);
        let va = *sys.y0.last().expect("the probe");
        // The current through the series resistor is the diode's, Is (e^(Vd / (N Vt)) - 1),
        // with Vd = Va - I RS; solved here by bisection on I (SPICE's GMIN, 1e-12 S across the
        // junction, is below this test's tolerance).
        let f = |i: f64| {
            let vd = (d.n * vt) * (i / d.is + 1.0).ln();
            vd + i * d.rs + i * rser - vin
        };
        let (mut lo, mut hi) = (0.0, vin / rser);
        for _ in 0..200 {
            let m = 0.5 * (lo + hi);
            if f(m) > 0.0 { hi = m } else { lo = m }
        }
        let i = 0.5 * (lo + hi);
        let exact = vin - i * rser;
        assert!(
            (va - exact).abs() < 1e-6,
            "at {vin} V the anode is {va} V, the diode equation {exact} V"
        );
    }
}

#[test]
fn a_common_emitter_stage_obeys_kirchhoff_and_its_small_signal_gain() {
    static BJTS: [&Bjt; 1] = [&fixtures::Q2SC945];
    let (rc, rb) = (10e3, 470e3);
    let parts = [
        part("RC", Kind::R(rc), &["@vcc", "c"]),
        part("RB", Kind::R(rb), &["@in", "b"]),
        part("Q1", Kind::Q(fixtures::Q2SC945.name), &["c", "b", "0"]),
    ];
    let circuit = Circuit {
        library: Library {
            bjts: &BJTS,
            ..Default::default()
        },
        rails: vec![("vcc".to_string(), 15.0)],
    };
    let net = Net::build(&parts, &opts(&["in"]), &circuit);
    let u = [1.0, 2.0];
    let op = operating_point(&net, &u).expect("operating point");
    let sys = System::new(&net, &op, &u, &["t_c", "t_b"]);
    let np = sys.y0.len();
    let (vc, vb) = (sys.y0[np - 2], sys.y0[np - 1]);
    let ic = (15.0 - vc) / rc;
    let ib = (2.0 - vb) / rb;
    assert!(
        ic > 0.2e-3 && vc > 1.0,
        "a stage biased in its forward-active region: Ic {ic}, Vc {vc}"
    );
    let beta = ic / ib;
    assert!(
        (100.0..1000.0).contains(&beta),
        "the collector and base currents' ratio {beta}"
    );
    // A small step at the input. The hybrid-pi model with the card's series resistances: a base
    // current ib = dVb / (rb + r_pi + (beta + 1) RE) for a move dVb of the external base, a
    // collector current beta ib into Rc || ro, so the base-referred gain is
    // -beta (Rc || ro) / (rb + r_pi + (beta + 1) RE), with gm = Ic / Vt, r_pi = beta / gm and
    // ro = (VAF + Vce) / Ic (the Early effect). The collector's own 1 ohm is left out (1e-4 of Rc).
    let h = 1.0 / 48_000.0;
    let mut s = Stepper::new(&sys, Modal::new(&sys), h);
    let dv = 1e-4;
    for _ in 0..4800 {
        s.step(&[1.0, 2.0 + dv]);
    }
    let dvc = s.y[np - 2] - vc;
    let dvb = s.y[np - 1] - vb;
    let gain = dvc / dvb;
    let q = &fixtures::Q2SC945;
    let gm = ic / crate::dev::vt();
    let ro = (q.vaf + vc) / ic;
    let r_pi = beta / gm;
    let expected = -beta * (rc * ro / (rc + ro)) / (q.rb + r_pi + (beta + 1.0) * q.re);
    assert!(
        (gain / expected - 1.0).abs() < 0.003,
        "small-signal gain {gain}, its closed form {expected}"
    );
}

#[test]
fn a_cmos_inverter_switches_at_half_its_supply_and_sits_at_its_rails() {
    // A symmetric pair: the same KP W / L and channel-length modulation, thresholds +-1.5 V,
    // each bulk at its rail.
    static N: Mos = Mos {
        name: "TEST_N",
        pmos: false,
        vto: 1.5,
        beta: 0.36e-3,
        lambda: 0.01,
        is: 1e-14,
    };
    static P: Mos = Mos {
        name: "TEST_P",
        pmos: true,
        vto: -1.5,
        ..N
    };
    static MOS: [&Mos; 2] = [&N, &P];
    let parts = [
        part("MN", Kind::M(N.name), &["out", "@in", "0", "0"]),
        part("MP", Kind::M(P.name), &["out", "@in", "@vdd", "@vdd"]),
        part("CL", Kind::C(10e-12), &["out", "0"]),
    ];
    let circuit = Circuit {
        library: Library {
            mos: &MOS,
            ..Default::default()
        },
        rails: vec![("vdd".to_string(), 15.0)],
    };
    let net = Net::build(&parts, &opts(&["in"]), &circuit);
    let out = |vin: f64| {
        let u = [1.0, vin];
        let op = operating_point(&net, &u).expect("operating point");
        *System::new(&net, &op, &u, &["t_out"])
            .y0
            .last()
            .expect("the probe")
    };
    // At half the supply each device is the other's mirror image: the output is there too.
    assert!((out(7.5) - 7.5).abs() < 1e-6, "{}", out(7.5));
    // Off, each rail is reached within what the off device's junction leakage and GMIN draw.
    assert!((out(0.0) - 15.0).abs() < 1e-6, "{}", out(0.0));
    assert!(out(15.0).abs() < 1e-6, "{}", out(15.0));
    // Between, the output falls as the input rises.
    let (a, b) = (out(6.0), out(9.0));
    assert!(a > 7.5 && b < 7.5 && a > b, "{a} {b}");
}

#[test]
fn a_jfets_ohmic_contacts_and_built_in_potential_match_ngspice() {
    // ngspice 47's drain node (omarchy, 2026-11-07; the deck in K-CIRCUIT-MS20, its rshunt the
    // network's default and its tolerances tight): 15 V through 100 ohm into the drain, the
    // source at ground through the card's 1 k contact, the gate at 0, -0.3 and -0.6 V.
    static J: Jfet = Jfet {
        name: "TEST_J",
        vto: -0.8,
        beta: 1.6e-3,
        lambda: 0.005,
        cgs: 5e-12,
        cgd: 3e-12,
        is: 1e-14,
        pb: 2.0,
        rd: 10.0,
        rs: 1000.0,
    };
    static JFETS: [&Jfet; 1] = [&J];
    let parts = [
        part("R1", Kind::R(100.0), &["@vdd", "d"]),
        part("J1", Kind::J(J.name), &["d", "@g", "0"]),
    ];
    let circuit = Circuit {
        library: Library {
            jfets: &JFETS,
            ..Default::default()
        },
        rails: vec![("vdd".to_string(), 15.0)],
    };
    let net = Net::build(&parts, &opts(&["g"]), &circuit);
    for (vg, want) in [
        (0.0, 14.965_091_394_295_62),
        (-0.3, 14.982_191_805_750_35),
        (-0.6, 14.995_739_786_846_79),
    ] {
        let u = [1.0, vg];
        let op = operating_point(&net, &u).expect("operating point");
        let vd = *System::new(&net, &op, &u, &["t_d"])
            .y0
            .last()
            .expect("the probe");
        assert!(
            (vd - want).abs() < 1e-9,
            "gate {vg} V: drain {vd} V, ngspice {want} V"
        );
    }
}

#[test]
fn a_loaded_cmos_inverters_step_converges_on_ngspice() {
    // ngspice 47's output (Gear order 2, 0.05 us steps, reltol 1e-6; the deck in
    // K-CIRCUIT-MS20) for the inverter above with 100 nF on its output, its input ramped from
    // 0 to 15 V over the third sample at 192 kHz, every fourth sample from the first.
    const NGSPICE: [f64; 15] = [
        14.999999997,
        12.479088444,
        5.988536190,
        2.442235505,
        0.927171753,
        0.342361688,
        0.125122734,
        0.045556467,
        0.016564065,
        0.006019586,
        0.002187199,
        0.000794661,
        0.000288713,
        0.000104895,
        0.000038111,
    ];
    static N: Mos = Mos {
        name: "TEST_N",
        pmos: false,
        vto: 1.5,
        beta: 0.36e-3,
        lambda: 0.01,
        is: 1e-14,
    };
    static P: Mos = Mos {
        name: "TEST_P",
        pmos: true,
        vto: -1.5,
        ..N
    };
    static MOS: [&Mos; 2] = [&N, &P];
    let parts = [
        part("MN", Kind::M(N.name), &["out", "@in", "0", "0"]),
        part("MP", Kind::M(P.name), &["out", "@in", "@vdd", "@vdd"]),
        part("CL", Kind::C(100e-9), &["out", "0"]),
    ];
    let circuit = Circuit {
        library: Library {
            mos: &MOS,
            ..Default::default()
        },
        rails: vec![("vdd".to_string(), 15.0)],
    };
    let net = Net::build(&parts, &opts(&["in"]), &circuit);
    let u0 = [1.0, 0.0];
    let op = operating_point(&net, &u0).expect("operating point");
    let sys = System::new(&net, &op, &u0, &["t_out"]);
    let mut errors = Vec::new();
    for sub in [1usize, 2, 4, 8, 16] {
        let h = 1.0 / 192_000.0 / sub as f64;
        let mut s = Stepper::new(&sys, Modal::new(&sys), h);
        let mut out = vec![*sys.y0.last().expect("the probe")];
        for k in 1..=56 {
            for j in 1..=sub {
                // The input's ramp over the third sample, as the deck's PWL.
                let f = ((k - 1) as f64 + j as f64 / sub as f64 - 2.0).clamp(0.0, 1.0);
                s.step(&[1.0, 15.0 * f]);
            }
            out.push(*s.y.last().expect("the probe"));
        }
        let mut worst: f64 = 0.0;
        for (i, want) in NGSPICE.iter().enumerate() {
            assert!(out[4 * i].is_finite());
            worst = worst.max((out[4 * i] - want).abs());
        }
        errors.push(worst);
    }
    // The stepper converges on ngspice at second order (0.41, 0.099, 0.023, 0.0058, 0.0014 V
    // when written): a 15 V edge in under ten samples is its step's, not the devices', error.
    for w in errors.windows(2) {
        assert!(w[1] < w[0] / 3.5, "{errors:?}");
    }
    assert!(errors[4] < 2e-3, "{errors:?}");
}
