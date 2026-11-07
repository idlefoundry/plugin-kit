//! The engine on small networks that belong to no instrument, each against its closed form: the
//! pipeline (parts, network, operating point, state space, modes, stepper) as an instrument uses
//! it.

use crate::dc::operating_point;
use crate::devices::{Bjt, Diode, Library, fixtures};
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
    // A small step at the input: the collector moves by -gm (Rc || ro) times the intrinsic base's
    // move, gm = Ic / Vt, ro = (VAF + Vce) / Ic (the Early effect), and the intrinsic base sees
    // the external one through RB's divider with r_pi = beta / gm.
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
    let expected = -gm * (rc * ro / (rc + ro)) * r_pi / (r_pi + q.rb);
    assert!(
        (gain / expected - 1.0).abs() < 0.02,
        "small-signal gain {gain}, its closed form {expected}"
    );
}
