//! The operating point: the node equations with the capacitors open and the charges constant,
//! solved by Newton's method with SPICE's junction limiting (`DEVpnjlim`), falling back when
//! plain Newton fails on source stepping, then on gmin stepping (a conductance from every node
//! to ground, stepped down to nothing; ngspice tries it first, but the voices built before it
//! was added found theirs by source stepping, and a circuit with hysteresis may have more than
//! one: the order keeps theirs).

use crate::dev;
use crate::la::Lu;
use crate::net::Net;

/// An operating point: the unknowns, and how many Newton iterations it took.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Op {
    pub x: Vec<f64>,
    pub iters: usize,
}

/// Whether `PLUGIN_KIT_CIRCUIT_DC_TRACE` asks for each Newton iteration to be printed.
fn trace() -> bool {
    std::env::var_os("PLUGIN_KIT_CIRCUIT_DC_TRACE").is_some()
}

/// The limits of the operating point's search: SPICE's for the junctions, bounded steps for
/// the saturating behavioural models (their outputs' nodes can be nearly floating until the
/// solution is found, so a free step in saturation would throw the next iterate far away).
fn dc_limits(d: &crate::dev::Dev) -> Vec<crate::run::Lim> {
    use crate::dev::Dev;
    use crate::run::Lim::*;
    match d {
        Dev::Bjt(_) | Dev::Jfet(_) => vec![Junction, Junction],
        Dev::Mos(_) => vec![Step(0.5), Step(0.5), Junction, Junction],
        Dev::Diode(_) => vec![Junction],
        Dev::Ota(_) => vec![Junction, Step(0.05)],
        Dev::OpIn(_) => vec![Step(0.05)],
        Dev::Cmp(_) => vec![Step(0.05), Step(0.5), Step(0.5), Step(0.25), Step(0.5)],
        Dev::Schmitt(_) => vec![Step(0.1), Step(0.5), Step(0.25), Step(0.5)],
        Dev::Clamp(_) => vec![None, None],
        Dev::Cond(_) => vec![None],
    }
}

/// Where a Newton solve from no previous solution starts its junction devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Init {
    /// At the starting nodes' voltages (the engine's first users' operating points).
    #[default]
    Nodes,
    /// As ngspice's `MODEINITJCT` ([`dev::Dev::init_junctions`]): the first iteration evaluates
    /// each junction device at its starting junction voltages instead, so a junction between a
    /// node starting at 0 V and a negative rail does not start far forward (from where Newton
    /// comes down only one thermal voltage an iteration).
    Junctions,
}

/// One Newton solve from `x0` at inputs `u`, a conductance `gshunt` from every node to ground,
/// its junction devices first evaluated as `init` says when `fresh` (no previous solution);
/// `None` if it does not converge.
fn newton_from(
    net: &Net,
    u: &[f64],
    x0: &[f64],
    max_iter: usize,
    gshunt: f64,
    init: Option<Init>,
) -> Option<Op> {
    let n = net.n();
    let vt = dev::vt();
    let mut x = x0.to_vec();
    // The controls each device was last linearised at (for limiting).
    let mut vprev: Vec<Vec<f64>> = net
        .elems
        .iter()
        .map(|e| e.ctrl.iter().map(|c| c.eval(&x, u)).collect())
        .collect();
    let start: Vec<Option<Vec<f64>>> = net
        .elems
        .iter()
        .map(|e| match init {
            Some(Init::Junctions) => e.dev.init_junctions(),
            _ => None,
        })
        .collect();
    let bu = net.bu.mul_vec(u);
    let mut out = [0.0; 8];
    let mut jac = [0.0; 40];
    // How many iterations in a row (to the last) met ngspice's own criterion at the reference's
    // tolerances (each unknown's step within its RELTOL of it, 1e-5, and VNTOL, 1 uV;
    // unlimited): a node held on a comparator's threshold dithers by tenths of a microvolt and
    // never meets the tighter one below, which ngspice would accept. (Its device currents'
    // own criterion is not checked here.)
    let mut loose = 0;
    for it in 0..max_iter {
        // Each iteration a factorisation of the whole network (the metal bank's: tens of
        // seconds in all), so a cancelled build looks at its token every time.
        crate::cancel::now();
        let mut a = net.g.clone();
        for i in 0..n {
            a[(i, i)] += gshunt;
        }
        let mut rhs = bu.clone();
        let mut limited = false;
        for (k, e) in net.elems.iter().enumerate() {
            let (nv, ns, _nq) = e.dev.shape();
            let mut v: Vec<f64> = e.ctrl.iter().map(|c| c.eval(&x, u)).collect();
            match &start[k] {
                Some(v0) if it == 0 => {
                    v.clone_from(v0);
                    limited = true;
                }
                _ => {
                    for (ci, lim) in dc_limits(&e.dev).into_iter().enumerate() {
                        let (vl, l) = crate::run::limit(lim, v[ci], vprev[k][ci], vt);
                        v[ci] = vl;
                        limited |= l;
                    }
                }
            }
            vprev[k].clone_from(&v);
            e.dev.eval(&v, &mut out, &mut jac);
            // i(v) + J (Q x + Qu u - v): the u part of each control.
            let vu: Vec<f64> = e.ctrl.iter().map(|c| c.eval(&vec![0.0; n], u)).collect();
            for o in 0..ns {
                let mut c0 = out[o];
                for ci in 0..nv {
                    c0 += jac[o * nv + ci] * (vu[ci] - v[ci]);
                }
                for &(node, s) in &e.inj[o] {
                    rhs[node] += s * c0;
                    for ci in 0..nv {
                        let jv = jac[o * nv + ci];
                        if jv == 0.0 {
                            continue;
                        }
                        for &(xi, xc) in &e.ctrl[ci].x {
                            a[(node, xi)] -= s * jv * xc;
                        }
                    }
                }
            }
        }
        let lu = Lu::new(&a);
        if lu.rcond == 0.0 {
            return None;
        }
        let xn = lu.solve(&rhs);
        if xn.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let dmax = x
            .iter()
            .zip(&xn)
            .map(|(a, b)| (a - b).abs() / (1e-6 + 1e-9 * b.abs().max(a.abs())))
            .fold(0.0f64, f64::max);
        if trace() {
            let (mut w, mut wi) = (0.0f64, 0);
            for (i, (a, b)) in x.iter().zip(&xn).enumerate() {
                if (a - b).abs() > w {
                    w = (a - b).abs();
                    wi = i;
                }
            }
            eprintln!(
                "dc {it}: limited {limited} dmax {dmax:.3e}, largest step {w:.3e} at {} ({:+.6})",
                net.names[wi], xn[wi]
            );
        }
        let met = !limited
            && it > 0
            && x.iter()
                .zip(&xn)
                .all(|(a, b)| (a - b).abs() <= 1e-6 + 1e-5 * a.abs().max(b.abs()));
        loose = if met { loose + 1 } else { 0 };
        x = xn;
        if !limited && dmax < 1e-3 && it > 0 {
            return Some(Op { x, iters: it + 1 });
        }
    }
    if loose >= 3 {
        eprintln!(
            "plugin_kit_circuit::dc: an operating point accepted on ngspice's criterion (RELTOL 1e-5, VNTOL 1 uV, its last {loose} iterations), not on the tighter one"
        );
        return Some(Op { x, iters: max_iter });
    }
    None
}

/// One Newton solve from a previous solution `x0`.
fn newton(net: &Net, u: &[f64], x0: &[f64], max_iter: usize, gshunt: f64) -> Option<Op> {
    newton_from(net, u, x0, max_iter, gshunt, None)
}

/// The operating point at inputs `u`, every fresh start's junctions at the nodes' voltages.
pub fn operating_point(net: &Net, u: &[f64]) -> Result<Op, String> {
    operating_point_with(net, u, Init::Nodes)
}

/// The operating point at inputs `u`, each fresh start (plain Newton, source stepping's first
/// step, gmin stepping's first) with its junctions as `init` says.
pub fn operating_point_with(net: &Net, u: &[f64], init: Init) -> Result<Op, String> {
    let n = net.n();
    if let Some(op) = newton_from(net, u, &vec![0.0; n], 200, 0.0, Some(init)) {
        return Ok(polish(net, u, op));
    }
    // Source stepping: every input scaled from a tenth up.
    let mut x = vec![0.0; n];
    let mut total = 0;
    let mut lambda = 0.0f64;
    let mut step = 0.1f64;
    while lambda < 1.0 {
        let next = (lambda + step).min(1.0);
        let us: Vec<f64> = u.iter().map(|v| v * next).collect();
        let fresh = (lambda == 0.0).then_some(init);
        match newton_from(net, &us, &x, 100, 0.0, fresh) {
            Some(op) => {
                x = op.x;
                total += op.iters;
                lambda = next;
                step = (step * 2.0).min(0.25);
            }
            None => {
                step /= 4.0;
                if step < 1e-6 {
                    return gmin_stepping(net, u, init)
                        .map(|op| polish(net, u, op))
                        .ok_or_else(|| {
                            format!(
                                "no operating point (source stepping at {lambda}, gmin stepping)"
                            )
                        });
                }
            }
        }
    }
    Ok(polish(net, u, Op { x, iters: total }))
}

/// The largest shunt gmin stepping starts from, the smallest it steps to before none (ngspice's:
/// GMIN times ten to its ten steps, then GMIN).
const GSHUNT_FIRST: f64 = 1e-2;
const GSHUNT_LAST: f64 = 1e-12;

/// Gmin stepping: solved with a conductance from every node to ground, from
/// [`GSHUNT_FIRST`] down tenfold a step (finer where a step fails) to [`GSHUNT_LAST`], each from
/// the solution before, then without it; `None` if a step cannot be made.
fn gmin_stepping(net: &Net, u: &[f64], init: Init) -> Option<Op> {
    let mut op = newton_from(net, u, &vec![0.0; net.n()], 200, GSHUNT_FIRST, Some(init))?;
    let (mut g, mut factor, mut total) = (GSHUNT_FIRST, 10.0f64, op.iters);
    while g > GSHUNT_LAST {
        let next = (g / factor).max(GSHUNT_LAST);
        match newton(net, u, &op.x, 100, next) {
            Some(o) => {
                total += o.iters;
                op = o;
                g = next;
                factor = (factor * factor).min(10.0);
            }
            None => {
                factor = factor.sqrt();
                if factor < 1.001 {
                    return None;
                }
            }
        }
    }
    let o = newton(net, u, &op.x, 100, 0.0)?;
    Some(Op {
        x: o.x,
        iters: total + o.iters,
    })
}

/// A few more Newton steps from a converged point, to the last digit.
fn polish(net: &Net, u: &[f64], op: Op) -> Op {
    match newton(net, u, &op.x, 20, 0.0) {
        Some(p) => Op {
            x: p.x,
            iters: op.iters + p.iters,
        },
        None => op,
    }
}

/// The residual of the node equations at `x` (amperes, at each node), for tests.
pub fn residual(net: &Net, u: &[f64], x: &[f64]) -> Vec<f64> {
    let mut r = net.g.mul_vec(x);
    let bu = net.bu.mul_vec(u);
    for (ri, b) in r.iter_mut().zip(&bu) {
        *ri -= b;
    }
    let mut out = [0.0; 8];
    let mut jac = [0.0; 40];
    for e in &net.elems {
        let (_, ns, _) = e.dev.shape();
        let v: Vec<f64> = e.ctrl.iter().map(|c| c.eval(x, u)).collect();
        e.dev.eval(&v, &mut out, &mut jac);
        for o in 0..ns {
            for &(node, s) in &e.inj[o] {
                r[node] -= s * out[o];
            }
        }
    }
    r
}
