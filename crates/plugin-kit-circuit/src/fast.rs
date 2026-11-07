//! The ring: a reduced system stepped at the sample rate with only the devices that still move
//! (the others at their operating point, their departures zero), its modes in structure-of-
//! arrays form. What a voice runs once its strike has settled.

use crate::la::{C64, Mat};
use crate::run::{Counters, MAX_ITER};
use crate::ss::{Modal, Slot, System, weights};

/// The modes' exact weights over one step size, and the kept outputs' direct response to the
/// step's final kept inputs.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct RingRate {
    h: f64,
    e_re: Vec<f64>,
    e_im: Vec<f64>,
    g0_re: Vec<f64>,
    g0_im: Vec<f64>,
    g1_re: Vec<f64>,
    g1_im: Vec<f64>,
    ga_re: Vec<f64>,
    ga_im: Vec<f64>,
    k: Mat,
}

/// The ring's stepper.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ring {
    /// The step sizes it takes: the sample, and its `sub`th where a played-back strike has an
    /// edge.
    rates: Vec<RingRate>,
    nm: usize,
    pub z_re: Vec<f64>,
    pub z_im: Vec<f64>,
    /// The inputs it keeps: their index in the system's `w`, and whether each is a charge
    /// current (held at the step's end) rather than first-order held.
    ins: Vec<usize>,
    charge: Vec<bool>,
    /// Inputs by modes.
    b_re: Vec<f64>,
    b_im: Vec<f64>,
    /// The first-order-held inputs' part of `B w` at the step's start.
    bw_re: Vec<f64>,
    bw_im: Vec<f64>,
    /// The outputs it keeps (the moving devices' controls, then the probes): their index in
    /// the system's `y`, by modes; their direct response to the step's final inputs; their
    /// operating point.
    outs: Vec<usize>,
    c_re: Vec<f64>,
    c_im: Vec<f64>,
    d: Mat,
    y0: Vec<f64>,
    /// The moving devices, their controls' offsets among the kept outputs, their inputs'
    /// among the kept inputs.
    slots: Vec<Slot>,
    v_at: Vec<usize>,
    s_at: Vec<usize>,
    q_at: Vec<usize>,
    /// The number of kept controls (the kept outputs before the probes).
    nv: usize,
    /// The sources it keeps (their index in `u`), with their operating-point values.
    srcs: Vec<(usize, f64)>,
    w: Vec<f64>,
    rq: Vec<f64>,
    pub v: Vec<f64>,
    pub y: Vec<f64>,
    pub counters: Counters,
    /// A played-back strike's contribution to the next step: the modes' increment and the
    /// kept outputs' direct part (`force`).
    fz_re: Vec<f64>,
    fz_im: Vec<f64>,
    fy: Vec<f64>,
    forced: bool,
    // Scratch.
    p: Vec<f64>,
    jg: Mat,
    g: Vec<f64>,
    dv: Vec<f64>,
    vt: f64,
}

impl Ring {
    /// A ring stepper of `sys` (in modal form `m`) at step `h` and `h / sub`, keeping the
    /// devices whose
    /// `moving` flag is set, the sources `sources` (by input index) and every probe.
    pub fn new(
        sys: &System,
        m: &Modal,
        h: f64,
        sub: usize,
        moving: &[bool],
        sources: &[usize],
    ) -> Ring {
        let nm = m.mu.len();
        let mut ins = Vec::new();
        let mut charge = Vec::new();
        let mut srcs = Vec::new();
        for &k in sources {
            ins.push(k);
            charge.push(false);
            srcs.push((k, sys.u0[k]));
        }
        let mut outs = Vec::new();
        let mut slots = Vec::new();
        let (mut v_at, mut s_at, mut q_at) = (vec![], vec![], vec![]);
        for (s, &mv) in sys.slots.iter().zip(moving) {
            if !mv {
                continue;
            }
            v_at.push(outs.len());
            for c in 0..s.nv {
                outs.push(s.v_off + c);
            }
            s_at.push(ins.len());
            for o in 0..s.ns {
                ins.push(s.s_off + o);
                charge.push(false);
            }
            slots.push(s.clone());
        }
        // The charge inputs after the static ones.
        for (s, &mv) in sys.slots.iter().zip(moving) {
            if !mv {
                continue;
            }
            q_at.push(ins.len());
            for q in 0..s.nq {
                ins.push(s.q_off + q);
                charge.push(true);
            }
        }
        let nv = outs.len();
        for p in 0..sys.probes.len() {
            outs.push(sys.nv + p);
        }
        let ni = ins.len();
        let no = outs.len();
        let mut b_re = vec![0.0; ni * nm];
        let mut b_im = vec![0.0; ni * nm];
        for (i, &k) in ins.iter().enumerate() {
            for j in 0..nm {
                b_re[i * nm + j] = m.b[j][k].re;
                b_im[i * nm + j] = m.b[j][k].im;
            }
        }
        let mut c_re = vec![0.0; no * nm];
        let mut c_im = vec![0.0; no * nm];
        let mut d = Mat::zeros(no, ni);
        for (o, &yo) in outs.iter().enumerate() {
            for j in 0..nm {
                c_re[o * nm + j] = m.c[yo][j].re;
                c_im[o * nm + j] = m.c[yo][j].im;
            }
            for (i, &wi) in ins.iter().enumerate() {
                d[(o, i)] = m.d[(yo, wi)];
            }
        }
        let rate = |h: f64| {
            let mut r = RingRate {
                h,
                k: Mat::zeros(no, ni),
                ..Default::default()
            };
            let mut g1 = Vec::new();
            let mut ga = Vec::new();
            for &mu in &m.mu {
                let (e, a0, a1, aa) = weights(mu, h);
                r.e_re.push(e.re);
                r.e_im.push(e.im);
                r.g0_re.push(a0.re);
                r.g0_im.push(a0.im);
                r.g1_re.push(a1.re);
                r.g1_im.push(a1.im);
                r.ga_re.push(aa.re);
                r.ga_im.push(aa.im);
                g1.push(a1);
                ga.push(aa);
            }
            for (o, &yo) in outs.iter().enumerate() {
                for (i, &wi) in ins.iter().enumerate() {
                    let wgt = if charge[i] { &ga } else { &g1 };
                    let mut s = 0.0;
                    for j in 0..nm {
                        s += (m.c[yo][j] * wgt[j] * m.b[j][wi]).re;
                    }
                    r.k[(o, i)] = s + m.d[(yo, wi)];
                }
            }
            r
        };
        let rates = vec![rate(h), rate(h / sub as f64)];
        let y0: Vec<f64> = outs.iter().map(|&o| sys.y0[o]).collect();
        let nq: usize = slots.iter().map(|s| s.nq).sum();
        Ring {
            rates,
            nm,
            z_re: vec![0.0; nm],
            z_im: vec![0.0; nm],
            ins,
            charge,
            b_re,
            b_im,
            bw_re: vec![0.0; nm],
            bw_im: vec![0.0; nm],
            outs,
            c_re,
            c_im,
            d,
            v: y0[..nv].to_vec(),
            y: y0.clone(),
            y0,
            slots,
            v_at,
            s_at,
            q_at,
            nv,
            srcs,
            w: vec![0.0; ni],
            rq: vec![0.0; nq],
            counters: Counters::default(),
            fz_re: vec![0.0; nm],
            fz_im: vec![0.0; nm],
            fy: vec![0.0; no],
            forced: false,
            p: vec![0.0; no],
            jg: Mat::zeros(nv, nv),
            g: vec![0.0; nv],
            dv: vec![0.0; nv],
            vt: crate::dev::vt(),
        }
    }

    /// Takes over a modal state (the ring's own modes) and the held inputs, charges and
    /// controls of the system's full input and output vectors.
    pub fn import(&mut self, z: &[C64], wf: &[f64], rq_full: &[f64], v_full: &[f64], nf: usize) {
        for j in 0..self.nm {
            self.z_re[j] = z[j].re;
            self.z_im[j] = z[j].im;
        }
        let mut qi = 0;
        for (i, &wi) in self.ins.iter().enumerate() {
            if self.charge[i] {
                self.w[i] = 0.0;
                self.rq[qi] = rq_full[wi - nf];
                qi += 1;
            } else {
                self.w[i] = wf[wi];
            }
        }
        for (o, &yo) in self.outs[..self.nv].iter().enumerate() {
            self.v[o] = v_full[yo];
        }
        self.refresh_bw();
    }

    /// The kept outputs' indices in the system's `y` (the kept controls, then the probes).
    pub fn outs(&self) -> &[usize] {
        &self.outs
    }

    /// Sets a panel pot's knob (0 to 1): every kept section of the part `refdes`.
    pub fn set_pot(&mut self, refdes: &str, pos: f64) {
        for s in &mut self.slots {
            if s.refdes == refdes
                && let Some(g) = s.pot_g(pos)
            {
                s.dev = crate::dev::Dev::Cond(g);
            }
        }
    }

    /// Adds to the next step a played-back strike's increment of the modes and its direct
    /// part of the kept outputs.
    pub fn force(&mut self, dz: &[C64], dy: &[f64]) {
        for j in 0..self.nm {
            self.fz_re[j] = dz[j].re;
            self.fz_im[j] = dz[j].im;
        }
        self.fy.copy_from_slice(dy);
        self.forced = true;
    }

    fn refresh_bw(&mut self) {
        let nm = self.nm;
        self.bw_re.iter_mut().for_each(|x| *x = 0.0);
        self.bw_im.iter_mut().for_each(|x| *x = 0.0);
        for (i, &x) in self.w.iter().enumerate() {
            if self.charge[i] || x == 0.0 {
                continue;
            }
            let (br, bi) = (
                &self.b_re[i * nm..(i + 1) * nm],
                &self.b_im[i * nm..(i + 1) * nm],
            );
            for j in 0..nm {
                self.bw_re[j] += br[j] * x;
                self.bw_im[j] += bi[j] * x;
            }
        }
    }

    /// One sample's step, the kept sources at `u` (the system's full input vector).
    pub fn step(&mut self, u: &[f64]) {
        let rates = std::mem::take(&mut self.rates);
        self.step_at(&rates[0], u);
        self.rates = rates;
    }

    /// One of a sample's sub-steps (the `sub` it was made with).
    pub fn step_sub(&mut self, u: &[f64]) {
        let rates = std::mem::take(&mut self.rates);
        self.step_at(&rates[1], u);
        self.rates = rates;
    }

    fn step_at(&mut self, r: &RingRate, u: &[f64]) {
        let nm = self.nm;
        let nv = self.nv;
        // The free part of the state.
        for j in 0..nm {
            let (zr, zi) = (self.z_re[j], self.z_im[j]);
            let (br, bi) = (self.bw_re[j], self.bw_im[j]);
            self.z_re[j] = r.e_re[j] * zr - r.e_im[j] * zi + r.g0_re[j] * br - r.g0_im[j] * bi;
            self.z_im[j] = r.e_re[j] * zi + r.e_im[j] * zr + r.g0_re[j] * bi + r.g0_im[j] * br;
        }
        if self.forced {
            for j in 0..nm {
                self.z_re[j] += self.fz_re[j];
                self.z_im[j] += self.fz_im[j];
            }
        }
        // The kept sources' departures at the step's end.
        for (i, &(k, u0)) in self.srcs.iter().enumerate() {
            self.w[i] = u[k] - u0;
        }
        let ns = self.srcs.len();
        for o in 0..self.outs.len() {
            let (cr, ci) = (
                &self.c_re[o * nm..(o + 1) * nm],
                &self.c_im[o * nm..(o + 1) * nm],
            );
            let mut s = self.y0[o];
            for j in 0..nm {
                s += cr[j] * self.z_re[j] - ci[j] * self.z_im[j];
            }
            for i in 0..ns {
                s += r.k[(o, i)] * self.w[i];
            }
            if self.forced {
                s += self.fy[o];
            }
            self.p[o] = s;
        }
        // Newton on the kept controls.
        let mut v = std::mem::take(&mut self.v);
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        let mut js = [0.0; 16];
        let mut jq = [0.0; 16];
        let mut it = 0;
        let mut converged = false;
        while it < MAX_ITER {
            it += 1;
            for i in 0..nv {
                self.g[i] = v[i] - self.p[i];
            }
            self.jg.a.iter_mut().for_each(|x| *x = 0.0);
            for i in 0..nv {
                self.jg[(i, i)] = 1.0;
            }
            let mut qi = 0;
            for (si, s) in self.slots.iter().enumerate() {
                let (va, sa, qa) = (self.v_at[si], self.s_at[si], self.q_at[si]);
                let mut rs = [0.0; 4];
                let mut rq = [0.0; 4];
                s.remainders(
                    &v[va..va + s.nv],
                    &mut rs[..s.ns],
                    &mut rq[..s.nq],
                    &mut js,
                    &mut jq,
                    &mut out,
                    &mut jac,
                );
                self.w[sa..sa + s.ns].copy_from_slice(&rs[..s.ns]);
                for q in 0..s.nq {
                    self.w[qa + q] = (rq[q] - self.rq[qi + q]) / r.h;
                }
                for i in 0..nv {
                    let mut acc = 0.0;
                    for o in 0..s.ns {
                        acc += r.k[(i, sa + o)] * rs[o];
                    }
                    for q in 0..s.nq {
                        acc += r.k[(i, qa + q)] * self.w[qa + q];
                    }
                    self.g[i] -= acc;
                    for c in 0..s.nv {
                        let mut d = 0.0;
                        for o in 0..s.ns {
                            d += r.k[(i, sa + o)] * js[o * s.nv + c];
                        }
                        for q in 0..s.nq {
                            d += r.k[(i, qa + q)] * jq[q * s.nv + c] / r.h;
                        }
                        self.jg[(i, va + c)] -= d;
                    }
                }
                qi += s.nq;
            }
            let mut dv = std::mem::take(&mut self.dv);
            dv.copy_from_slice(&self.g);
            crate::la::solve_in_place(&mut self.jg.a, nv, &mut dv);
            let mut big = 0.0f64;
            let mut limited = false;
            for (si, s) in self.slots.iter().enumerate() {
                let lims = crate::run::limits(&s.dev);
                let at = self.v_at[si]..self.v_at[si] + s.nv;
                let mut vn = [0.0; 5];
                for (c, i) in at.clone().enumerate() {
                    vn[c] = v[i] - dv[i];
                }
                limited |= crate::run::limit_controls(
                    &s.dev,
                    &lims,
                    &mut vn[..s.nv],
                    &v[at.clone()],
                    self.vt,
                );
                for (c, i) in at.enumerate() {
                    big = big.max((vn[c] - v[i]).abs() / (1e-9 + 1e-9 * vn[c].abs()));
                    v[i] = vn[c];
                }
            }
            self.dv = dv;
            if !limited && big < 1.0 {
                converged = true;
                break;
            }
        }
        self.counters.steps += 1;
        self.counters.iterations += it as u64;
        if !converged {
            self.counters.capped += 1;
        }
        // The remainders at the solution, the charges' currents, and the step's end.
        let mut qi = 0;
        for (si, s) in self.slots.iter().enumerate() {
            let (va, sa, qa) = (self.v_at[si], self.s_at[si], self.q_at[si]);
            let mut rs = [0.0; 4];
            let mut rq = [0.0; 4];
            s.remainders(
                &v[va..va + s.nv],
                &mut rs[..s.ns],
                &mut rq[..s.nq],
                &mut js,
                &mut jq,
                &mut out,
                &mut jac,
            );
            self.w[sa..sa + s.ns].copy_from_slice(&rs[..s.ns]);
            for q in 0..s.nq {
                self.w[qa + q] = (rq[q] - self.rq[qi + q]) / r.h;
                self.rq[qi + q] = rq[q];
            }
            qi += s.nq;
        }
        self.v = v;
        self.refresh_bw();
        let mut bq_re = [0.0f64; 64];
        let mut bq_im = [0.0f64; 64];
        let nmq = nm.min(64);
        for (i, &x) in self.w.iter().enumerate() {
            if !self.charge[i] || x == 0.0 {
                continue;
            }
            for j in 0..nmq {
                bq_re[j] += self.b_re[i * nm + j] * x;
                bq_im[j] += self.b_im[i * nm + j] * x;
            }
        }
        for j in 0..nm {
            let (br, bi) = (self.bw_re[j], self.bw_im[j]);
            let (qr, qi_) = if j < 64 {
                (bq_re[j], bq_im[j])
            } else {
                (0.0, 0.0)
            };
            self.z_re[j] += r.g1_re[j] * br - r.g1_im[j] * bi + r.ga_re[j] * qr - r.ga_im[j] * qi_;
            self.z_im[j] += r.g1_re[j] * bi + r.g1_im[j] * br + r.ga_re[j] * qi_ + r.ga_im[j] * qr;
        }
        for o in 0..self.outs.len() {
            let (cr, ci) = (
                &self.c_re[o * nm..(o + 1) * nm],
                &self.c_im[o * nm..(o + 1) * nm],
            );
            let mut s = self.y0[o];
            for j in 0..nm {
                s += cr[j] * self.z_re[j] - ci[j] * self.z_im[j];
            }
            for (i, &x) in self.w.iter().enumerate() {
                s += self.d[(o, i)] * x;
            }
            if self.forced {
                s += self.fy[o];
            }
            self.y[o] = s;
        }
        self.forced = false;
    }

    /// The `p`-th probe's value after the last step.
    pub fn probe(&self, p: usize) -> f64 {
        self.y[self.nv + p]
    }
}
