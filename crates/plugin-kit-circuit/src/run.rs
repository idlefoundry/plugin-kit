//! Stepping a [`Modal`] system in time: each mode advanced exactly over the step with the
//! inputs and the devices' static remainders under a first-order hold, and their charges'
//! currents held at the step's end (backward Euler on charge: charge is conserved, and the
//! junctions' nanosecond time constants stay stable at audio steps). The devices' controls at
//! the step's end are solved by Newton's method, with SPICE's junction limiting.

use crate::dev::{self, Dev};
use crate::la::{C64, Mat};
use crate::ss::{Modal, System, weights};

/// Counters a run keeps, which the tests read.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Counters {
    pub steps: u64,
    pub iterations: u64,
    /// Steps whose Newton solve stopped at its iteration cap.
    pub capped: u64,
    /// Samples taken again in sub-steps because a switch turned in them (the live part's:
    /// taken in sub-steps because a switch was about to turn).
    pub resteps: u64,
    /// The live part's samples in which a switch turned unforeseen, in one step.
    pub missed: u64,
    /// Steps whose switches' rounds stopped at their cap and were taken as found (the last
    /// target's move under a ten-thousandth of its swing, near a fold): counted, as every cap.
    pub rounds_capped: u64,
}

/// The most Newton iterations a step may take.
pub const MAX_ITER: usize = 60;

/// The exact weights of every mode over one step size, and the outputs' direct response to
/// the step's final inputs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Rate {
    pub h: f64,
    e: Vec<C64>,
    g0: Vec<C64>,
    g1: Vec<C64>,
    ga: Vec<C64>,
    /// Outputs from the step's final first-order-held inputs, and from its charge currents.
    kf: Mat,
    kq: Mat,
    /// The static remainders' direct response when held at the step's end (a stiff device).
    ka: Mat,
}

impl Rate {
    fn new(sys: &System, m: &Modal, h: f64) -> Rate {
        let nf = sys.nu + sys.ns;
        let no = m.c.len();
        let nm = m.mu.len();
        let mut e = Vec::with_capacity(nm);
        let mut g0 = Vec::with_capacity(nm);
        let mut g1 = Vec::with_capacity(nm);
        let mut ga = Vec::with_capacity(nm);
        for &mu in &m.mu {
            let (ee, a0, a1, aa) = weights(mu, h);
            e.push(ee);
            g0.push(a0);
            g1.push(a1);
            ga.push(aa);
        }
        let mut kf = Mat::zeros(no, nf);
        let mut kq = Mat::zeros(no, sys.nq);
        let mut ka = Mat::zeros(no, sys.ns);
        for o in 0..no {
            for k in sys.nu..nf {
                let mut s = 0.0;
                for j in 0..nm {
                    s += (m.c[o][j] * ga[j] * m.b[j][k]).re;
                }
                ka[(o, k - sys.nu)] = s + m.d[(o, k)];
            }
            for k in 0..nf {
                let mut s = 0.0;
                for j in 0..nm {
                    s += (m.c[o][j] * g1[j] * m.b[j][k]).re;
                }
                kf[(o, k)] = s + m.d[(o, k)];
            }
            for q in 0..sys.nq {
                let k = nf + q;
                let mut s = 0.0;
                for j in 0..nm {
                    s += (m.c[o][j] * ga[j] * m.b[j][k]).re;
                }
                kq[(o, q)] = s + m.d[(o, k)];
            }
        }
        Rate {
            h,
            e,
            g0,
            g1,
            ga,
            kf,
            kq,
            ka,
        }
    }
}

/// How many sub-steps a sample splits into where an input has an edge.
pub const SUB: usize = 16;

/// A copy of a stepper's state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Snap {
    z: Vec<C64>,
    wf: Vec<f64>,
    rq: Vec<f64>,
    v: Vec<f64>,
    y: Vec<f64>,
    held: Vec<f64>,
    beta: Vec<f64>,
}

/// Whether a device's first output is a switching state's target (held through a solve).
fn is_switch(d: &Dev) -> bool {
    matches!(d, Dev::Cmp(_) | Dev::Schmitt(_))
}

/// A switching device's target held at `held`: its static
/// remainder and Jacobian row as if the target did not move with the controls.
fn hold(s: &crate::ss::Slot, held: f64, v: &[f64], rs: &mut [f64], js: &mut [f64]) {
    let nv = s.nv;
    let mut r = held - s.f0[0];
    for c in 0..nv {
        r -= s.j0[c] * (v[c] - s.v0[c]);
        js[c] = -s.j0[c];
    }
    rs[0] = r;
}

/// The stiffness from which a device's static currents are held mostly at the step's end
/// (see `Stepper::advance`).
pub const STIFF: f64 = 1e4;

/// How many times a step's switches may be brought up to date.
pub const MAX_ROUNDS: usize = 12;

/// A system stepped at a fixed rate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Stepper {
    /// The step sizes it can take: the sample, and its `sub`th for edges.
    rates: Vec<Rate>,
    sub: usize,
    m: Modal,
    slots: Vec<crate::ss::Slot>,
    nu: usize,
    ns: usize,
    nq: usize,
    nv: usize,
    no: usize,
    z: Vec<C64>,
    /// The first-order-held inputs' departures at the step's start (u - u0 and the static
    /// remainders).
    wf: Vec<f64>,
    u0: Vec<f64>,
    y0: Vec<f64>,
    du: Vec<f64>,
    rq: Vec<f64>,
    pub v: Vec<f64>,
    pub y: Vec<f64>,
    pub counters: Counters,
    /// Each switching device's (comparator's, Schmitt's) state-node target current, held
    /// through a Newton solve and then brought up to date until it agrees with the solution.
    held: Vec<f64>,
    switch: Vec<bool>,
    /// Each device's static currents' hold over a step: `beta` of it at the step's end
    /// (backward Euler, L-stable) and the rest first-order (second-order accurate), set from how
    /// stiff the device's own loop was in the last step.
    beta: Vec<f64>,
    col_beta: Vec<f64>,
    /// The inputs at the last sample's end, and a copy of the state at its start (a sample in
    /// which a switch turns is taken again in sub-steps).
    u_last: Vec<f64>,
    snap: Snap,
    /// Each device's largest static remainder (A) since this was last cleared, when it is on.
    pub activity: Option<Vec<f64>>,
    /// Every step's size and its end's inputs (`u - u0`, the static remainders, the charges'
    /// currents) and then its controls, when it is on: what a played-back strike is made
    /// from.
    pub record: Option<Vec<(f64, Vec<f64>)>>,
    /// The largest departure of every input (`w`, its sources' too) and of every control from
    /// the operating point, when it is on.
    pub extremes: Option<(Vec<f64>, Vec<f64>)>,
    // Scratch.
    zhat: Vec<C64>,
    p: Vec<f64>,
    rs_new: Vec<f64>,
    rq_new: Vec<f64>,
    iq: Vec<f64>,
    js: Vec<f64>,
    jq: Vec<f64>,
    jg: Mat,
    dv: Vec<f64>,
    v_lim: Vec<f64>,
    /// Each control's last Newton step in this solve (see [`damped`]).
    step_prev: Vec<f64>,
    vt: f64,
}

impl Stepper {
    /// A stepper at step `h` (and `h / SUB` for edges), at rest at the system's operating
    /// point.
    pub fn new(sys: &System, m: Modal, h: f64) -> Stepper {
        Stepper::with_sub(sys, m, h, SUB)
    }

    /// A stepper whose samples with an edge are taken in `sub` steps.
    pub fn with_sub(sys: &System, m: Modal, h: f64, sub: usize) -> Stepper {
        let nf = sys.nu + sys.ns;
        let no = m.c.len();
        let rates = vec![Rate::new(sys, &m, h), Rate::new(sys, &m, h / sub as f64)];
        // At rest: no departure from the operating point.
        let z = vec![C64::ZERO; m.mu.len()];
        let y = sys.y0.clone();
        let nv = sys.nv;
        Stepper {
            sub,
            slots: sys.slots.clone(),
            nu: sys.nu,
            ns: sys.ns,
            nq: sys.nq,
            nv,
            no,
            rates,
            zhat: z.clone(),
            z,
            wf: vec![0.0; nf],
            u0: sys.u0.clone(),
            y0: sys.y0.clone(),
            du: vec![0.0; sys.nu],
            rq: vec![0.0; sys.nq],
            v: y[..nv].to_vec(),
            y,
            counters: Counters::default(),
            held: sys
                .slots
                .iter()
                .map(|s| if is_switch(&s.dev) { s.f0[0] } else { 0.0 })
                .collect(),
            switch: sys.slots.iter().map(|s| is_switch(&s.dev)).collect(),
            beta: vec![0.0; sys.slots.len()],
            col_beta: vec![0.0; sys.ns],
            u_last: sys.u0.clone(),
            snap: Snap::default(),
            activity: None,
            record: None,
            extremes: None,
            p: vec![0.0; no],
            rs_new: vec![0.0; sys.ns],
            rq_new: vec![0.0; sys.nq],
            iq: vec![0.0; sys.nq],
            js: vec![0.0; 64],
            jq: vec![0.0; 64],
            jg: Mat::zeros(nv, nv),
            dv: vec![0.0; nv],
            v_lim: vec![0.0; nv],
            step_prev: vec![0.0; nv],
            m,
            vt: dev::vt(),
        }
    }

    /// The state in its system's own coordinates, with the held inputs, charges and controls:
    /// what another stepper of a system with the same devices takes over.
    pub fn export(&self) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        (
            self.m.to_real(&self.z),
            self.wf.clone(),
            self.rq.clone(),
            self.v.clone(),
        )
    }

    /// Takes over another stepper's state, its coordinates mapped by `map` (a matrix from the
    /// other system's state to this one's).
    pub fn import(&mut self, st: &(Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>), map: &Mat) {
        let x = map.mul_vec(&st.0);
        self.z = self.m.from_real(&x);
        self.wf.clone_from(&st.1);
        self.rq.clone_from(&st.2);
        self.v.clone_from(&st.3);
    }

    /// The held inputs (`u - u0` and the static remainders) at the last step's end.
    pub fn held_inputs(&self) -> &[f64] {
        &self.wf
    }

    /// The devices' names (refdes and role), in the order of [`Stepper::activity`].
    pub fn device_names(&self) -> Vec<String> {
        self.slots
            .iter()
            .map(|s| format!("{} {}", s.refdes, s.role).trim().to_string())
            .collect()
    }

    /// The sample's step.
    pub fn h(&self) -> f64 {
        self.rates[0].h
    }

    /// One sample's step to inputs `u` at its end; the outputs are then in `self.y`.
    pub fn step(&mut self, u: &[f64]) {
        crate::cancel::point();
        let rates = std::mem::take(&mut self.rates);
        let any_switch = self.switch.iter().any(|x| *x);
        if any_switch {
            self.save();
        }
        let turned = self.advance(&rates[0], u);
        self.rates = rates;
        if turned {
            // A switch turned in the sample: take it again in sub-steps, the inputs between
            // the sample's ends linearly.
            self.restore();
            if let Some(rec) = self.record.as_mut() {
                rec.pop();
            }
            let u0 = self.u_last.clone();
            let u1 = u.to_vec();
            self.counters.resteps += 1;
            self.step_fine(|f, uu| {
                for k in 0..uu.len() {
                    uu[k] = u0[k] + (u1[k] - u0[k]) * f;
                }
            });
        }
        self.u_last.copy_from_slice(u);
    }

    /// One sample in `sub` steps, the inputs at each from `u_at(fraction of the sample)`
    /// (for a sample in which an input has an edge).
    pub fn step_fine(&mut self, mut u_at: impl FnMut(f64, &mut [f64])) {
        crate::cancel::point();
        let rates = std::mem::take(&mut self.rates);
        let mut u = vec![0.0; self.nu];
        for k in 1..=self.sub {
            u_at(k as f64 / self.sub as f64, &mut u);
            self.advance(&rates[1], &u);
        }
        self.u_last.copy_from_slice(&u);
        self.rates = rates;
    }

    /// The Newton residual `v - p - K (remainders)` at controls `v` into `g`, its Jacobian
    /// into `self.jg`, the remainders into `self.rs_new`, `self.rq_new`, `self.iq`; returns
    /// its largest magnitude.
    fn residual(&mut self, r: &Rate, v: &[f64], g: &mut [f64]) -> f64 {
        let nv = self.nv;
        let nu = self.nu;
        let nf = self.nu + self.ns;
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        for i in 0..nv {
            g[i] = v[i] - self.p[i];
        }
        self.jg.a.iter_mut().for_each(|x| *x = 0.0);
        for i in 0..nv {
            self.jg[(i, i)] = 1.0;
        }
        for (sk, s) in self.slots.iter().enumerate() {
            let (sv, ss_, sq) = (s.nv, s.ns, s.nq);
            let vs = &v[s.v_off..s.v_off + sv];
            let so = s.s_off - nu;
            let qo = s.q_off - nf;
            s.remainders(
                vs,
                &mut self.rs_new[so..so + ss_],
                &mut self.rq_new[qo..qo + sq],
                &mut self.js,
                &mut self.jq,
                &mut out,
                &mut jac,
            );
            if self.switch[sk] {
                hold(
                    s,
                    self.held[sk],
                    vs,
                    &mut self.rs_new[so..so + ss_],
                    &mut self.js,
                );
            }
            for q in 0..sq {
                self.iq[qo + q] = (self.rq_new[qo + q] - self.rq[qo + q]) / r.h;
            }
            let beta = self.beta[sk];
            for i in 0..nv {
                let mut acc = 0.0;
                for o in 0..ss_ {
                    let kk = (1.0 - beta) * r.kf[(i, s.s_off + o)] + beta * r.ka[(i, so + o)];
                    acc += kk * self.rs_new[so + o];
                }
                for q in 0..sq {
                    acc += r.kq[(i, qo + q)] * self.iq[qo + q];
                }
                g[i] -= acc;
                for c in 0..sv {
                    let mut d = 0.0;
                    for o in 0..ss_ {
                        let kk = (1.0 - beta) * r.kf[(i, s.s_off + o)] + beta * r.ka[(i, so + o)];
                        d += kk * self.js[o * sv + c];
                    }
                    for q in 0..sq {
                        d += r.kq[(i, qo + q)] * self.jq[q * sv + c] / r.h;
                    }
                    self.jg[(i, s.v_off + c)] -= d;
                }
            }
        }
        g.iter().fold(0.0f64, |m, x| m.max(x.abs()))
    }

    fn save(&mut self) {
        let s = &mut self.snap;
        s.z.clone_from(&self.z);
        s.wf.clone_from(&self.wf);
        s.rq.clone_from(&self.rq);
        s.v.clone_from(&self.v);
        s.y.clone_from(&self.y);
        s.held.clone_from(&self.held);
        s.beta.clone_from(&self.beta);
    }

    fn restore(&mut self) {
        let s = &self.snap;
        self.z.clone_from(&s.z);
        self.wf.clone_from(&s.wf);
        self.rq.clone_from(&s.rq);
        self.v.clone_from(&s.v);
        self.y.clone_from(&s.y);
        self.held.clone_from(&s.held);
        self.beta.clone_from(&s.beta);
    }

    /// One step; whether a switch turned in it.
    fn advance(&mut self, r: &Rate, u: &[f64]) -> bool {
        let nm = self.m.mu.len();
        let nf = self.nu + self.ns;
        let (nv, nu) = (self.nv, self.nu);
        // Each static current's share held at the step's end.
        for (sl, &b) in self.slots.iter().zip(&self.beta) {
            for o in 0..sl.ns {
                self.col_beta[sl.s_off - nu + o] = b;
            }
        }
        // The state's free part: the start's state and the first-order-held inputs' share.
        for j in 0..nm {
            let mut bw = C64::ZERO;
            for (k, &x) in self.wf.iter().enumerate() {
                if x != 0.0 {
                    let f = if k < nu {
                        1.0
                    } else {
                        1.0 - self.col_beta[k - nu]
                    };
                    if f != 0.0 {
                        bw = bw + self.m.b[j][k].scale(x * f);
                    }
                }
            }
            self.zhat[j] = r.e[j] * self.z[j] + r.g0[j] * bw;
        }
        // Every output's value with the step's devices' remainders at zero.
        for k in 0..nu {
            self.du[k] = u[k] - self.u0[k];
        }
        for o in 0..self.no {
            let mut s = self.y0[o];
            for j in 0..nm {
                s += (self.m.c[o][j] * self.zhat[j]).re;
            }
            for (k, &x) in self.du.iter().enumerate() {
                s += r.kf[(o, k)] * x;
            }
            self.p[o] = s;
        }
        // Newton on the devices' controls, the switches' targets held; then the targets
        // brought up to date, and again, until they agree with the solution.
        let mut v = self.v.clone();
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        let mut converged;
        self.step_prev.fill(0.0);
        let mut it = 0;
        let held_start = self.held.clone();
        let mut round = 0;
        loop {
            round += 1;
            converged = false;
            let it_round = it;
            let mut g = vec![0.0; nv];
            let mut gn = self.residual(r, &v, &mut g);
            if std::env::var_os("OLIFANT_NEWTON_FIRST").is_some() && self.counters.steps < 40 {
                let (mut w, mut wi) = (0.0f64, 0usize);
                for i in 0..nv {
                    if g[i].abs() > w {
                        w = g[i].abs();
                        wi = i;
                    }
                }
                let sl = self
                    .slots
                    .iter()
                    .find(|s| wi >= s.v_off && wi < s.v_off + s.nv)
                    .map(|s| format!("{} {} #{}", s.refdes, s.role, wi - s.v_off))
                    .unwrap_or_default();
                eprintln!(
                    "step {} round {round}: first residual {gn:.3e} at {sl} (v {:+.6}, p {:+.6})",
                    self.counters.steps, v[wi], self.p[wi]
                );
            }
            while it - it_round < MAX_ITER {
                it += 1;
                self.dv.copy_from_slice(&g);
                crate::la::solve_in_place(&mut self.jg.a, nv, &mut self.dv);
                let dv = &self.dv;
                let mut big = 0.0f64;
                let mut limited = false;
                let mut v_lim = std::mem::take(&mut self.v_lim);
                v_lim.copy_from_slice(&v);
                for s in &self.slots {
                    let lims = limits(&s.dev);
                    for c in 0..s.nv {
                        let i = s.v_off + c;
                        let step = damped(-dv[i], &mut self.step_prev[i]);
                        let (vn, l) = limit(lims[c], v[i] + step, v[i], self.vt);
                        limited |= l;
                        big = big.max((vn - v[i]).abs() / (1e-9 + 1e-9 * vn.abs()));
                        v_lim[i] = vn;
                    }
                }
                if !limited && big < 1.0 {
                    v.copy_from_slice(&v_lim);
                    self.v_lim = v_lim;
                    converged = true;
                    break;
                }
                if std::env::var_os("OLIFANT_NEWTON_DETAIL").is_some() && it > MAX_ITER - 6 {
                    let (mut w, mut wi) = (0.0f64, 0usize);
                    for i in 0..nv {
                        if (v_lim[i] - v[i]).abs() > w {
                            w = (v_lim[i] - v[i]).abs();
                            wi = i;
                        }
                    }
                    let sl = self
                        .slots
                        .iter()
                        .find(|s| wi >= s.v_off && wi < s.v_off + s.nv)
                        .map(|s| format!("{} {} #{}", s.refdes, s.role, wi - s.v_off))
                        .unwrap_or_default();
                    eprintln!(
                        "  it {it}: residual {gn:.3e}, step {w:.3e} at {sl} ({:+.6} -> {:+.6}), limited {limited}",
                        v[wi], v_lim[wi]
                    );
                }
                v.copy_from_slice(&v_lim);
                self.v_lim = v_lim;
                gn = self.residual(r, &v, &mut g);
            }
            // The switches' targets at the solution.
            let mut changed = false;
            for (k, sl) in self.slots.iter().enumerate() {
                if !self.switch[k] {
                    continue;
                }
                sl.dev
                    .eval(&v[sl.v_off..sl.v_off + sl.nv], &mut out, &mut jac);
                if (out[0] - self.held[k]).abs() > 1e-9 * sl.f0[0].abs().max(1e-6) {
                    self.held[k] = out[0];
                    changed = true;
                }
            }
            if !changed || round >= MAX_ROUNDS {
                if changed {
                    converged = false;
                }
                break;
            }
        }
        // Whether a switch turned: its target moved by a quarter of its swing or more.
        let turned = self.slots.iter().enumerate().any(|(k, sl)| {
            self.switch[k] && {
                let swing = match sl.dev {
                    Dev::Cmp(p) => 1.0 / p.r,
                    Dev::Schmitt(p) => 1.0 / p.r,
                    _ => 1.0,
                };
                (self.held[k] - held_start[k]).abs() > 0.25 * swing
            }
        });
        if it > 20 && std::env::var_os("OLIFANT_NEWTON_TRACE").is_some() {
            eprintln!("newton: {it} iterations, converged {converged}");
        }
        self.counters.steps += 1;
        self.counters.iterations += it as u64;
        if !converged {
            self.counters.capped += 1;
        }
        // The remainders at the solution.
        for (sk, s) in self.slots.iter().enumerate() {
            let so = s.s_off - nu;
            let qo = s.q_off - nf;
            s.remainders(
                &v[s.v_off..s.v_off + s.nv],
                &mut self.rs_new[so..so + s.ns],
                &mut self.rq_new[qo..qo + s.nq],
                &mut self.js,
                &mut self.jq,
                &mut out,
                &mut jac,
            );
            // The hold for the device's next step. Its own loop's stiffness under a
            // first-order hold is the diagonal of its block of K (J - J0): strongly negative,
            // -z, means it adds damping far beyond its linearisation (a junction turning on
            // into a small node), where a first-order hold's factor for that fast mode tends to
            // -1; positive means it takes away damping that the linearisation put into the
            // exact part, where holding at the end would leave the step's equation without a
            // slope. The share held at the end is z^2 / (z^2 + STIFF^2): none in the signal
            // paths (z of 1 to 1000, where the first-order hold is what keeps a voice exact),
            // all of it where a junction charges a large capacitor from a picofarad node (z of
            // 1e4 and more, where the first-order hold rings without end).
            if !self.switch[sk] {
                let mut lowest = f64::INFINITY;
                for i in 0..s.nv {
                    let mut m = 0.0;
                    for o in 0..s.ns {
                        m += r.kf[(s.v_off + i, s.s_off + o)]
                            * (jac[o * s.nv + i] - s.j0[o * s.nv + i]);
                    }
                    lowest = lowest.min(m);
                }
                let z = (-lowest).max(0.0);
                self.beta[sk] = match std::env::var("OLIFANT_HOLD").as_deref() {
                    Ok("foh") => 0.0,
                    Ok("be") => 1.0,
                    _ => z * z / (z * z + STIFF * STIFF),
                };
            }
            if self.switch[sk] {
                hold(
                    s,
                    self.held[sk],
                    &v[s.v_off..s.v_off + s.nv],
                    &mut self.rs_new[so..so + s.ns],
                    &mut self.js,
                );
            }
        }
        for q in 0..self.nq {
            self.iq[q] = (self.rq_new[q] - self.rq[q]) / r.h;
        }
        if let Some((wx, vx)) = self.extremes.as_mut() {
            for k in 0..nu {
                wx[k] = wx[k].max(self.du[k].abs());
            }
            for (k, x) in self.rs_new.iter().enumerate() {
                wx[nu + k] = wx[nu + k].max(x.abs());
            }
            for (k, x) in self.iq.iter().enumerate() {
                wx[nf + k] = wx[nf + k].max(x.abs());
            }
            for (k, x) in v.iter().enumerate() {
                vx[k] = vx[k].max((x - self.y0[k]).abs());
            }
        }
        if let Some(act) = self.activity.as_mut() {
            for (k, s) in self.slots.iter().enumerate() {
                let so = s.s_off - nu;
                for o in 0..s.ns {
                    act[k] = act[k].max(self.rs_new[so + o].abs());
                }
                let qo = s.q_off - nf;
                for q in 0..s.nq {
                    act[k] = act[k].max(self.iq[qo + q].abs());
                }
            }
        }
        // The step's end.
        self.wf[..nu].copy_from_slice(&self.du);
        self.wf[nu..].copy_from_slice(&self.rs_new);
        if let Some(rec) = self.record.as_mut() {
            let mut w = self.wf.clone();
            w.extend_from_slice(&self.iq);
            // After the inputs, the step's solution (the devices' controls): a played-back
            // strike's start for the live part's Newton.
            w.extend_from_slice(&v);
            rec.push((r.h, w));
        }
        for j in 0..nm {
            let mut bf = C64::ZERO;
            let mut bq = C64::ZERO;
            for (k, &x) in self.wf.iter().enumerate() {
                if x != 0.0 {
                    let b = if k < nu { 0.0 } else { self.col_beta[k - nu] };
                    if b != 1.0 {
                        bf = bf + self.m.b[j][k].scale(x * (1.0 - b));
                    }
                    if b != 0.0 {
                        bq = bq + self.m.b[j][k].scale(x * b);
                    }
                }
            }
            for (q, &x) in self.iq.iter().enumerate() {
                if x != 0.0 {
                    bq = bq + self.m.b[j][nf + q].scale(x);
                }
            }
            self.z[j] = self.zhat[j] + r.g1[j] * bf + r.ga[j] * bq;
        }
        self.rq.copy_from_slice(&self.rq_new);
        let mut w = self.wf.clone();
        w.extend_from_slice(&self.iq);
        for o in 0..self.no {
            let mut s = self.y0[o];
            for j in 0..nm {
                s += (self.m.c[o][j] * self.z[j]).re;
            }
            for (k, &x) in w.iter().enumerate() {
                s += self.m.d[(o, k)] * x;
            }
            self.y[o] = s;
        }
        self.v = v;
        turned
    }
}

/// How a device's control may move in one Newton iteration.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Lim {
    None,
    /// A junction: SPICE's limiting.
    Junction,
    /// A bounded step.
    Step(f64),
    /// A tanh's argument centred at `c` with width `w`: a step across the transition from
    /// saturation lands on its centre.
    Tanh(f64, f64),
}

/// A Newton step for a control whose last step was `prev` (then set to the step taken): halved
/// when it all but undoes that one. A tanh's control within a few widths of its centre can
/// swing from side to side across it, each step undoing the last (a comparator's output at its
/// rail, `vsat` wide); halved, it lands between the two.
pub fn damped(step: f64, prev: &mut f64) -> f64 {
    let s = if step * *prev < 0.0 && (step + *prev).abs() < 0.25 * step.abs() {
        0.5 * step
    } else {
        step
    };
    *prev = s;
    s
}

/// The limits of each control of a device.
pub fn limits(d: &Dev) -> Vec<Lim> {
    use Lim::*;
    match d {
        Dev::Bjt(_) | Dev::Jfet(_) => vec![Junction, Junction],
        Dev::Diode(_) => vec![Junction],
        Dev::Ota(p) => vec![Junction, Tanh(0.0, 1.0 / p.k)],
        Dev::OpIn(p) => vec![Tanh(0.0, p.imax / p.gm)],
        Dev::Cmp(p) => vec![
            Tanh(0.0, p.w),
            Tanh(-p.vcm, p.wcm),
            Tanh(-p.vcm, p.wcm),
            Step(0.5),
            Tanh(0.0, p.vsat),
        ],
        Dev::Schmitt(_) => vec![Step(0.1), Step(0.5), Step(0.25), Step(0.5)],
        Dev::Clamp(_) => vec![None, None],
        Dev::Cond(_) => vec![None],
    }
}

/// A control's new value under its limit: (value, whether it was limited).
pub fn limit(lim: Lim, vnew: f64, vold: f64, vt: f64) -> (f64, bool) {
    match lim {
        Lim::None => (vnew, false),
        Lim::Junction => {
            // pnjlim leaves a forward step of 2 VT or less alone: vcrit (a logarithm) only
            // for a bigger one.
            if vnew >= 0.0 && (vnew - vold).abs() <= vt + vt {
                return (vnew, false);
            }
            let vcrit = vt * libm::log(vt / (std::f64::consts::SQRT_2 * 1e-14));
            pnjlim(vnew, vold, vt, vcrit)
        }
        Lim::Step(d) => {
            if (vnew - vold).abs() > d {
                (vold + d.copysign(vnew - vold), true)
            } else {
                (vnew, false)
            }
        }
        Lim::Tanh(c, w) => {
            // A step that crosses the transition from saturation lands on its centre, where
            // the slope is: Newton linearised in saturation would jump from side to side.
            let k = 4.0 * w;
            let (xo, xn) = (vold - c, vnew - c);
            if xo.abs() > k && xo.signum() != xn.signum() {
                (c, true)
            } else {
                (vnew, false)
            }
        }
    }
}

/// ngspice's junction limiting (devsup.c `DEVpnjlim`), its reverse steps too.
pub fn pnjlim(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> (f64, bool) {
    if vnew > vcrit && (vnew - vold).abs() > vt + vt {
        let v = if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                vold + vt * libm::log(arg)
            } else {
                vcrit
            }
        } else {
            vt * libm::log(vnew / vt)
        };
        (v, true)
    } else if vnew < 0.0 {
        let arg = if vold > 0.0 {
            -vold - 1.0
        } else {
            2.0 * vold - 1.0
        };
        if vnew < arg {
            (arg, true)
        } else {
            (vnew, false)
        }
    } else {
        (vnew, false)
    }
}
