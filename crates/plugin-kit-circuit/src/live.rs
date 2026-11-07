//! The live part of a voice, stepped for speed: the reduced linear part in modal form, the
//! nonlinear devices that go on moving solved by Newton over their controls alone, and the
//! panel's pot sections folded in exactly (they are linear: a knob's change recomputes a few
//! small matrices, never the model). The same equations as [`crate::fast::Ring`], arranged so
//! that a sample costs as little as they allow.
//!
//! The modes are packed into reals: the real modes, then the complex pairs' real parts, then
//! their imaginary parts; an output is then one dot product with the state, an input one
//! scaled addition to it.
//!
//! The kept outputs are, in order: the nonlinear devices' controls (`n`), the pot sections'
//! controls (`p`), the probes. The kept inputs: the sources kept (`u`), the nonlinear devices'
//! static remainders (`s`), the pots' remainders (`p`), the nonlinear devices' charge currents
//! (`q`).

use crate::dev::Dev;
use crate::sim;

#[cfg(target_arch = "x86_64")]
type Step = unsafe fn(&mut Live, &Rate, &[f64], bool);

#[cfg(target_arch = "x86_64")]
fn select_step() -> Step {
    static STEP: std::sync::OnceLock<Step> = std::sync::OnceLock::new();
    *STEP.get_or_init(|| {
        if std::env::var_os("PLUGIN_KIT_CIRCUIT_FORCE_BASELINE").is_none() && v3_supported() {
            return step_v3;
        }
        step_baseline
    })
}

#[cfg(target_arch = "x86_64")]
fn v3_supported() -> bool {
    std::is_x86_feature_detected!("avx2")
        && std::is_x86_feature_detected!("fma")
        && std::is_x86_feature_detected!("bmi1")
        && std::is_x86_feature_detected!("bmi2")
        && std::is_x86_feature_detected!("lzcnt")
        && std::is_x86_feature_detected!("movbe")
        && std::is_x86_feature_detected!("popcnt")
}

#[cfg(target_arch = "x86_64")]
fn step_baseline(live: &mut Live, rate: &Rate, u: &[f64], sub: bool) {
    live.step_impl(rate, u, sub);
}

/// Called only through select_step's checked pointer. Arithmetic is unchanged;
/// in particular neither this copy nor the baseline asks for multiply-add fusion.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
#[target_feature(enable = "avx2,fma,bmi1,bmi2,lzcnt,movbe,popcnt")]
unsafe fn step_v3(live: &mut Live, rate: &Rate, u: &[f64], sub: bool) {
    live.step_impl(rate, u, sub);
}

/// Time spent in the live step's parts, ns (with the `prof` feature): the free state, the
/// kept outputs' free part, the devices' evaluations, Newton's solves and updates, the pots,
/// the state's end, the probes, Newton's new Jacobians.
#[cfg(feature = "prof")]
pub static PROF: [std::sync::atomic::AtomicU64; 8] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 8];

/// A study: steps of nine iterations or more reported (with `PLUGIN_KIT_CIRCUIT_REFRESH`).
pub static LONG_STEPS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets the parts' times to zero (with the `prof` feature).
pub fn prof_reset() {
    #[cfg(feature = "prof")]
    for p in &PROF {
        p.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The seconds in each part so far (zeros without the `prof` feature).
pub fn prof() -> [f64; 8] {
    #[cfg(feature = "prof")]
    {
        std::array::from_fn(|k| PROF[k].load(std::sync::atomic::Ordering::Relaxed) as f64 * 1e-9)
    }
    #[cfg(not(feature = "prof"))]
    {
        [0.0; 8]
    }
}

#[cfg(feature = "prof")]
macro_rules! tick {
    ($t:ident, $k:expr, end) => {
        PROF[$k].fetch_add(
            $t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    };
    ($t:ident, $k:expr) => {
        tick!($t, $k, end);
        $t = std::time::Instant::now();
    };
}
#[cfg(not(feature = "prof"))]
macro_rules! tick {
    ($t:ident, $k:expr $(, $e:ident)?) => {};
}
use crate::la::{C64, Mat};
use crate::run::{Counters, Lim, MAX_ITER, MAX_ROUNDS, damped, limit, limit_controls, limits};
use crate::ss::{Modal, Slot, System, weights};

/// `a · b` in eight partial sums (one running sum is a chain of dependent additions).
#[inline(always)]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let b = &b[..a.len()];
    let mut s = [0.0f64; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for k in 0..8 {
            s[k] += x[k] * y[k];
        }
    }
    let mut t = ((s[0] + s[1]) + (s[2] + s[3])) + ((s[4] + s[5]) + (s[6] + s[7]));
    for (x, y) in ra.iter().zip(rb) {
        t += x * y;
    }
    t
}

/// `a · b` for short vectors, in one running sum (the eight partial sums' reduction would
/// cost more than it saves).
#[inline(always)]
fn dot_short(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `y += a x`.
#[inline(always)]
fn axpy(y: &mut [f64], a: f64, x: &[f64]) {
    for (y, x) in y.iter_mut().zip(x) {
        *y += a * x;
    }
}

/// `y += w ⊙ x` over packed modes (a pair's weight and value multiply as complex numbers), `y`
/// the slow block or the whole state (the fast block from `nzs`, real).
#[inline(always)]
fn wmadd(y: &mut [f64], w: &[f64], x: &[f64], nr: usize, nc: usize, nzs: usize) {
    for ((y, w), x) in y[..nr].iter_mut().zip(&w[..nr]).zip(&x[..nr]) {
        *y += w * x;
    }
    for k in 0..nc {
        let (a, b) = (nr + k, nr + nc + k);
        let (wr, wi, xr, xi) = (w[a], w[b], x[a], x[b]);
        y[a] += wr * xr - wi * xi;
        y[b] += wr * xi + wi * xr;
    }
    if y.len() > nzs {
        for ((y, w), x) in y[nzs..].iter_mut().zip(&w[nzs..]).zip(&x[nzs..]) {
            *y += w * x;
        }
    }
}

/// The width of the blocks the modal products are taken in: their sums held in registers.
const W: usize = 16;

/// `out[..w] = Σ_j m_j[..w] x_j`, `m_j` the rows (each `stride` long) of a matrix stored by
/// rows, `w` a multiple of [`W`].
#[inline(always)]
fn sum_rows(out: &mut [f64], m: &[f64], x: &[f64], stride: usize, w: usize) {
    for oc in (0..w).step_by(W) {
        let mut acc = [0.0f64; W];
        for (row, &xj) in m.chunks_exact(stride).zip(x) {
            let r = &row[oc..oc + W];
            for k in 0..W {
                acc[k] += r[k] * xj;
            }
        }
        out[oc..oc + W].copy_from_slice(&acc);
    }
}

/// A real mode whose `μ h` is below this at the sample step keeps nothing of its state over a
/// sample (`e^{μh}` under 2.3e-16): in a whole sample's step it is algebraic.
const FAST: f64 = 36.0;

/// The loop gain at which a regenerative switch is taken as about to turn, at its controls
/// carried on for one sample ([`Live::switch_ahead`]): a Schmitt oscillator within 8 ppm of
/// its period at 10 ns sub-steps (0.5, or three samples ahead: twice the samples taken in
/// them for the same).
const REGEN: f64 = 0.7;

/// The least move of a switch's held target, re its swing, that may send it to a rail
/// ([`Live::update_held`]).
const JUMP: f64 = 1e-4;

/// The Jacobian's entry between two devices' controls under which Newton by blocks leaves it
/// out (`Live::factor_blocks`).
const BLOCK_TOL: f64 = 0.05;

/// Newton by blocks: the refactors after which the blocks are found again (and after any step
/// of more than BLOCK_SLOW iterations).
const BLOCK_REFIND: usize = 256;
const BLOCK_SLOW: usize = 8;

/// As [`sum_rows`] over two matrices of rows `stride` long and their vectors, into the same
/// sums.
#[inline(always)]
fn sum_rows2(out: &mut [f64], a: (&[f64], &[f64]), b: (&[f64], &[f64]), stride: usize, w: usize) {
    for oc in (0..w).step_by(W) {
        let mut acc = [0.0f64; W];
        for (m, x) in [a, b] {
            for (row, &xj) in m.chunks_exact(stride).zip(x) {
                let r = &row[oc..oc + W];
                for k in 0..W {
                    acc[k] += r[k] * xj;
                }
            }
        }
        out[oc..oc + W].copy_from_slice(&acc);
    }
}

/// `out[..w] = Σ_i m_i[..w] x_i` over the rows `i` in `ranges` (each `stride` long).
#[inline(always)]
fn sum_rows_in(
    out: &mut [f64],
    m: &[f64],
    x: &[f64],
    stride: usize,
    w: usize,
    ranges: &[(usize, usize)],
) {
    for oc in (0..w).step_by(W) {
        let mut acc = [0.0f64; W];
        for &(lo, hi) in ranges {
            for (row, &xj) in m[lo * stride..hi * stride]
                .chunks_exact(stride)
                .zip(&x[lo..hi])
            {
                let r = &row[oc..oc + W];
                for k in 0..W {
                    acc[k] += r[k] * xj;
                }
            }
        }
        out[oc..oc + W].copy_from_slice(&acc);
    }
}

/// The modes' order once packed: the slow block (the real modes, then the pairs' real parts,
/// then their imaginary parts, padded to a multiple of [`W`]: `nzs` long), then the fast block
/// (the real modes algebraic over a sample, padded).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Pack {
    real: Vec<usize>,
    pair: Vec<usize>,
    fast: Vec<usize>,
    nzs: usize,
}

impl Pack {
    fn new(m: &Modal, h: f64) -> Pack {
        let (mut real, mut pair, mut fast) = (Vec::new(), Vec::new(), Vec::new());
        for (j, mu) in m.mu.iter().enumerate() {
            if mu.im != 0.0 {
                pair.push(j);
            } else if mu.re * h < -FAST {
                fast.push(j);
            } else {
                real.push(j);
            }
        }
        let nzs = (real.len() + 2 * pair.len()).div_ceil(W) * W;
        Pack {
            real,
            pair,
            fast,
            nzs,
        }
    }

    /// The packed length.
    fn len(&self) -> usize {
        self.nzs + self.fast.len().div_ceil(W) * W
    }

    fn pack(&self, x: &[C64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.len());
        out.extend(self.real.iter().map(|&j| x[j].re));
        out.extend(self.pair.iter().map(|&j| x[j].re));
        out.extend(self.pair.iter().map(|&j| x[j].im));
        out.resize(self.nzs, 0.0);
        out.extend(self.fast.iter().map(|&j| x[j].re));
        out.resize(self.len(), 0.0);
        out
    }
}

/// The modes' weights over one step size and the kept outputs' direct response to the step's
/// final inputs, with the pots folded in for the knobs as they are.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Rate {
    h: f64,
    inv_h: f64,
    /// The modes' weights, packed: the free response, the held inputs' at the step's start
    /// and end, the charges'.
    e: Vec<f64>,
    g0: Vec<f64>,
    g1: Vec<f64>,
    ga: Vec<f64>,
    /// Rows: the kept outputs; columns: the kept inputs.
    k: Mat,
    /// The fold (`M = (I - K_pp ΔG)⁻¹`), flat: the nonlinear controls' response to their own
    /// remainders with the pots' through them, `K_n(s,q) + K_np ΔG M K_p(s,q)`, by columns
    /// (column `c` at `c * nn`); `K_np ΔG M` (n by p, by rows); `ΔG M` (p by p); `K_p(s,q)`
    /// (p by s + q); the sources' columns of the controls and pots (n + p by u).
    kn_t: Vec<f64>,
    knp_m: Vec<f64>,
    dg_m: Vec<f64>,
    kp_r: Vec<f64>,
    k_src: Vec<f64>,
    /// A whole sample's step (the fast modes algebraic): their part of the controls and pots
    /// from the held inputs at the step's start (by inputs: input `i`'s at `i * nop`), of the
    /// probes (probes by the held inputs), and the probes' direct part with theirs (probes by
    /// inputs).
    whole: bool,
    k0f: Vec<f64>,
    kp0f: Vec<f64>,
    dw: Vec<f64>,
}

/// A nonlinear device kept live.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LDev {
    slot: Slot,
    /// Its controls' offset among the `n` outputs, its statics' among `s`, its charges' among
    /// `q`.
    v_at: usize,
    s_at: usize,
    q_at: usize,
    lims: Vec<Lim>,
    /// Its operating point, as the slot's, in place: the controls, the outputs, the statics'
    /// Jacobian, and for each charge the control whose capacitance is in the linear part
    /// with that capacitance (zero if none).
    v0: [f64; 5],
    f0: [f64; 8],
    #[serde(with = "crate::serial::array")]
    j0: [f64; 40],
    cd: [(usize, f64); 8],
    /// Its last evaluation: at controls `v_ev`, its outputs less their linearisation (the
    /// statics' remainders, the charges' less their own capacitance's) and their Jacobian.
    v_ev: [f64; 5],
    out_ev: [f64; 8],
    #[serde(with = "crate::serial::array")]
    jac_ev: [f64; 40],
    evaluated: bool,
    /// A switching device (a comparator, a Schmitt trigger): its state's target current, held
    /// through each Newton solve and brought up to date between them (as the full model's,
    /// `run.rs`), its value at the step's start, and its swing (a quarter of which moved in a
    /// step is a switch turned).
    switch: bool,
    held: f64,
    held_start: f64,
    swing: f64,
    /// A regenerative switch (a Schmitt trigger): the control that is its own state, on which
    /// its target turns (its loop gain, the target's slope in it re the swing, reaches 1 at
    /// its fold).
    regen: Option<usize>,
    /// Its held target's last move in this step's rounds, and whether it has been sent to a
    /// rail in them.
    held_move: f64,
    jumped: bool,
    /// A regenerative switch's rounds in this step: the last held target and its target's
    /// move from it, and two held targets whose moves had opposite signs (each with its move,
    /// then which end the last round replaced: -1, 1, or 0 for neither).
    last: Option<(f64, f64)>,
    bracket: Option<[f64; 6]>,
    /// Its Jacobian columns to be made again (see `Live::step_at`).
    stale: bool,
}

/// The error a Newton correction may leave in the controls, V: on a junction a change of its
/// current by 4e-7 of itself (`docs/realtime/cost.md`).
const TOL_V: f64 = 1e-8;

/// The iteration at which Newton, not yet converged from the predicted start, starts again from
/// the last step's solution.
const RESTART: usize = 12;

/// The iterations after which a whole sample's Newton solve is given up and the sample taken
/// again in sub-steps, with the retry on ([`Live::set_retry`]): twice [`RESTART`].
const RETRY_AFTER: usize = 2 * RESTART;

/// How far a device's controls may move from its last evaluation before it is evaluated
/// again, its outputs taken to first order meanwhile: their error is then half their curvature
/// times its square (under 1e-12 A for a transistor at a milliampere), Newton's own tolerance.
const SKIP: f64 = 1e-6;

impl LDev {
    fn new(s: &Slot, v_at: usize, s_at: usize, q_at: usize) -> LDev {
        let mut d = LDev {
            slot: s.clone(),
            v_at,
            s_at,
            q_at,
            lims: limits(&s.dev),
            v0: [0.0; 5],
            f0: [0.0; 8],
            j0: [0.0; 40],
            cd: [(0, 0.0); 8],
            v_ev: [0.0; 5],
            out_ev: [0.0; 8],
            jac_ev: [0.0; 40],
            evaluated: false,
            switch: false,
            held: s.f0[0],
            held_start: s.f0[0],
            swing: 1.0,
            regen: None,
            held_move: 0.0,
            jumped: false,
            last: None,
            bracket: None,
            stale: false,
        };
        match s.dev {
            Dev::Cmp(p) => (d.switch, d.swing) = (true, 1.0 / p.r),
            Dev::Schmitt(p) => (d.switch, d.swing, d.regen) = (true, 1.0 / p.r, Some(2)),
            _ => {}
        }
        d.v0[..s.nv].copy_from_slice(&s.v0);
        d.f0[..s.ns + s.nq].copy_from_slice(&s.f0[..s.ns + s.nq]);
        d.j0[..s.ns * s.nv].copy_from_slice(&s.j0[..s.ns * s.nv]);
        for (q, c) in s.cdiag.iter().enumerate() {
            d.cd[q] = c.unwrap_or((0, 0.0));
        }
        d
    }
}

/// A regenerative switch's next held target in a step's rounds, `delta` its target's move
/// from the held one (`d.last` the round before's). Its rounds seek one unknown, `held =
/// T(held)`, whose map's slope nears 1 at the switch's fold and passes it while the switch
/// turns, where plain rounds crawl or run away (rounds capped at 10 ns a step). Once two
/// rounds' moves have had opposite signs, the root between them by the Illinois method (the
/// secant kept within the bracket, an end kept twice its move halved; halved instead where a
/// round left it more than half as wide: past a fold the target's map is flat on one side);
/// before that the secant through the last two rounds while they converge (within the rails),
/// else the plain round.
fn regen_round(d: &mut LDev, delta: f64) -> f64 {
    let (x, f) = (d.held, delta);
    let mut halve = false;
    if let Some(b) = d.bracket.as_mut() {
        let wide = (b[2] - b[0]).abs();
        if f * b[1] > 0.0 {
            (b[0], b[1]) = (x, f);
            if b[4] < 0.0 {
                b[3] *= 0.5;
            }
            b[4] = -1.0;
        } else {
            (b[2], b[3]) = (x, f);
            if b[4] > 0.0 {
                b[1] *= 0.5;
            }
            b[4] = 1.0;
        }
        halve = (b[2] - b[0]).abs() > 0.5 * b[5];
        b[5] = wide;
    } else if let Some((xl, fl)) = d.last
        && fl * f < 0.0
    {
        d.bracket = Some([xl, fl, x, f, 0.0, 2.0 * (x - xl).abs()]);
    }
    if let Some(b) = d.bracket {
        return if halve {
            0.5 * (b[0] + b[2])
        } else {
            b[0] - b[1] * (b[2] - b[0]) / (b[3] - b[1])
        };
    }
    if let Some((xl, fl)) = d.last
        && fl * f > 0.0
        && f.abs() < 0.9 * fl.abs()
    {
        return (x - f * (x - xl) / (f - fl)).clamp(0.0, d.swing);
    }
    x + f
}

/// A pot section kept live.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LPot {
    slot: Slot,
    /// Its conductance in the linear part, and now.
    g0: f64,
    g: f64,
}

/// The live part's stepper.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Live {
    #[cfg(target_arch = "x86_64")]
    #[serde(skip, default = "select_step")]
    step_fn: Step,
    rates: Vec<Rate>,
    pack: Pack,
    nr: usize,
    nc: usize,
    nz: usize,
    /// The slow block's length and the fast modes' number.
    nzs: usize,
    nf: usize,
    /// The modal state, packed (the fast block current only after sub-steps).
    pub z: Vec<f64>,
    /// The controls' and pots' rows of `C`, by modes (mode `j`'s at `j * nop`, `nop` their
    /// number padded to a multiple of [`W`]), and their products with the state.
    ct: Vec<f64>,
    nop: usize,
    at: Vec<f64>,
    devs: Vec<LDev>,
    pots: Vec<LPot>,
    nn: usize,
    np: usize,
    /// The devices live only while a strike plays (D41: the gates, the pulse shapers, the
    /// common trigger, which a strike on a ringing voice moves otherwise than one from rest)
    /// come after the others (`nd_r` devices; `nn_r` controls, `ns_r` statics, `nq_r` charges):
    /// between strikes they are at rest and out of the step. The outputs' lanes are the others'
    /// controls, the pots', theirs, the probes (`nop_r` lanes padded, without theirs).
    nd_r: usize,
    nn_r: usize,
    ns_r: usize,
    nq_r: usize,
    nop_r: usize,
    /// The ring's devices that a strike from rest's playback leaves live (those the noise
    /// drives, or whose strike a pot changes) come first (`nd_f` devices; `nn_f` controls,
    /// `ns_f` statics, `nq_f` charges): while such a strike plays (`frozen_left` samples) the
    /// others are played back whole ([`Live::freeze`], [`Live::handover`]).
    nd_f: usize,
    nn_f: usize,
    ns_f: usize,
    nq_f: usize,
    /// Samples left in which they are live, and the level of the last step (0: the ring's
    /// devices, 1: a strike played back whole, 2: every device, 3: a table ring awaiting
    /// handover to Newton); whether the devices entering
    /// the next step have been handed a state.
    active_left: usize,
    frozen_left: usize,
    was_level: usize,
    handed: bool,
    /// Each step's size and its inputs at its end, as the system orders them (its `w`: the
    /// sources' departures, the statics' remainders, the charges' currents; then its
    /// controls), when recording: the live model with every device live records a strike as
    /// the full model does (`run::Stepper::record`). The system's `w` and controls' lengths.
    pub record: Option<Vec<(f64, Vec<f64>)>>,
    sys_nw: usize,
    sys_nv: usize,
    nprobe: usize,
    nsrc: usize,
    ns: usize,
    nq: usize,
    /// The kept outputs' indices in the system's `y`.
    outs: Vec<usize>,
    srcs: Vec<(usize, f64)>,
    /// The kept inputs' columns of `B`, packed (input `i`'s at `i * nz`).
    b: Vec<f64>,
    /// The kept outputs' rows of `C`, packed and conjugated (output `o`'s at `o * nz`):
    /// `Re(c ζ)` is then a dot product.
    c: Vec<f64>,
    /// The probes' direct part (probe `p`'s row of the kept inputs at `p * ni`).
    d: Vec<f64>,
    y0: Vec<f64>,
    /// The first-order-held inputs' `B w` at the step's start, and the charges' (scratch).
    bw: Vec<f64>,
    bq: Vec<f64>,
    /// The kept inputs at the last step's end, and the devices' charges; the kept inputs at
    /// the ends of the two steps before (the fast modes' state, made again when sub-steps
    /// follow whole samples), and the fast modes' share of the last whole sample's strike.
    w: Vec<f64>,
    w_prev: Vec<f64>,
    w_prev2: Vec<f64>,
    fzf_last: Vec<f64>,
    last_sub: bool,
    rq: Vec<f64>,
    /// The nonlinear controls at the last step's end.
    pub v: Vec<f64>,
    /// The kept outputs at the last step's end: the nonlinear controls and the probes.
    pub y: Vec<f64>,
    pub counters: Counters,
    fz: Vec<f64>,
    fy: Vec<f64>,
    forced: bool,
    // Scratch.
    a: Vec<f64>,
    g: Vec<f64>,
    dv: Vec<f64>,
    /// Newton's Jacobian, factored (`lu_factor`), with its pivots: kept from step to step
    /// while the controls stay near those it was made at (`v_lu`, within `j_tol`), at the step
    /// size `lu_h`; `lu_ok` false when it must be made again.
    jg: Vec<f64>,
    /// The Jacobian unfactored (its columns made again device by device), and the controls'
    /// number it was factored at; an iteration on an older one left work.
    jraw: Vec<f64>,
    lu_na: usize,
    refresh_moved: bool,
    /// The Jacobian as factored (unfactored), the controls whose columns have changed since,
    /// and their correction (`woodbury`): `W` by columns, `S` factored and its pivots, scratch;
    /// the corrections made instead of factorisations.
    jbase: Vec<f64>,
    wd_cols: Vec<usize>,
    /// Newton by blocks (`Live::set_blocks`, a study): the controls in blocks of devices the
    /// Jacobian couples by more than BLOCK_TOL (each block's controls in `bperm` from
    /// `bstart[b]`), each block's Jacobian factored alone (`bjg`, `bpiv`, from `boff[b]`), the
    /// largest entry left out; whether in use.
    blocks: bool,
    bperm: Vec<usize>,
    bstart: Vec<usize>,
    boff: Vec<usize>,
    bjg: Vec<f64>,
    bpiv: Vec<usize>,
    bdrop: f64,
    btmp: Vec<f64>,
    /// Each control's block, refactors since the blocks were found, and whether to find them
    /// again at the next.
    bof: Vec<usize>,
    bage: usize,
    bfind: bool,
    /// This step finishing on the whole Jacobian (its blocks slow to converge).
    bdense: bool,
    wd_w: Vec<f64>,
    wd_s: Vec<f64>,
    wd_piv: Vec<usize>,
    wd_t: Vec<f64>,
    pub updates: u64,
    piv: Vec<usize>,
    v_lu: Vec<f64>,
    j_tol: Vec<f64>,
    /// Each control's op-amp output clamp's knee (0 for another device's control).
    knee: Vec<f64>,
    lu_h: f64,
    lu_ok: bool,
    /// Factorisations made, for the counters.
    pub factorisations: u64,
    r: Vec<f64>,
    jr: Vec<f64>,
    vp: Vec<f64>,
    /// The nonlinear controls' last two changes, and the steps they were over (the
    /// predictor).
    dv_last: Vec<f64>,
    dv_last2: Vec<f64>,
    v_prev: Vec<f64>,
    /// The remainders at the ends of the last steps, and their extrapolation (the predictor
    /// from the remainders: `predict_r`).
    r_prev_step: Vec<f64>,
    r_hist1: Vec<f64>,
    r_hist2: Vec<f64>,
    r_pred: Vec<f64>,
    pub predict_r: bool,
    h_last: f64,
    h_last2: f64,
    vt: f64,
    /// Whether a switch turned in the last step.
    pub turned: bool,
    /// The next step's start ([`Live::hint`]) and whether there is one; whether hints are taken.
    hint_v: Vec<f64>,
    hinted: bool,
    pub use_hints: bool,
    /// Iterations a step may take on an older Jacobian before a new one is made.
    chord: usize,
    /// How far a device's controls may move before it is evaluated again ([`SKIP`]; a study
    /// may set it, `PLUGIN_KIT_CIRCUIT_SKIP`).
    skip: f64,
    /// A study (`PLUGIN_KIT_CIRCUIT_TOL`): Newton's tolerances scaled; (`PLUGIN_KIT_CIRCUIT_TRACE=a,b`) the iterations
    /// of steps a to b printed.
    tol_scale: f64,
    /// The error a correction may leave (TOL_V; `PLUGIN_KIT_CIRCUIT_TOL_LEFT`, a study: so many times it).
    tol_v: f64,
    trace: Option<(u64, u64)>,
    /// The remainders' three extrapolations this step and their mean square errors (the
    /// order used, `PLUGIN_KIT_CIRCUIT_PRED_ORDER` 0, 1 or 2 for a study, else the best).
    pred_order: usize,
    /// Whether Newton starts from a step from the last solution (see `step_at`).
    pub lin_start: bool,
    /// Start Newton from the last solution, remake its Jacobian, and limit each correction
    /// by a common fraction with residual backtracking. The mixer opts in while slewing;
    /// voices retain their existing solver.
    #[serde(default)]
    pub coupled_limit: bool,
    /// Keep a coupled solve's Jacobian only while its corrections reduce the residual.
    /// Only the mixer selects this mode; voices keep their existing refresh strategy.
    #[serde(default)]
    pub coupled_reuse: bool,
    /// On a slew reversal, keep the existing tanh limit's landing on its slope, then
    /// rebuild the Jacobian there instead of backtracking into saturation.
    #[serde(default)]
    pub coupled_slew: bool,
    /// Opposing clamp controls share one output and fixed ideal rails. Eliminate each
    /// second correction algebraically, keeping both clamp currents and residuals.
    #[serde(default)]
    dependent: Vec<(usize, usize)>,
    #[serde(default)]
    independent: Vec<usize>,
    #[serde(default)]
    reduced_rhs: Vec<f64>,
    pred_all: Vec<f64>,
    pred_err: Vec<f64>,
    /// A study (`PLUGIN_KIT_CIRCUIT_REFRESH`): how often each control's move made a new Jacobian, and how
    /// many steps took each number of iterations.
    pub refresh_by: Vec<u64>,
    pub it_hist: Vec<u64>,
    /// (With it, how often each control moved more than `SKIP` over a whole sample.)
    pub moved_by: Vec<u64>,
    /// Whether any device is a switch; the sub-steps a sample; each mode's share of a sample's
    /// played-back increment over one sub-step, were its input constant over the sample
    /// (`(e^{μh/n} - 1) / (e^{μh} - 1)`, packed); the last step's played-back direct part with
    /// the fast modes' share taken out; scratch for a sample taken in sub-steps.
    has_switch: bool,
    sub: usize,
    /// Each packed mode's `μ`. A source's step inside the coming sample (its input, the
    /// sample's fraction before it, its value before): the sample is taken in sub-steps and the
    /// one holding the step corrected to it ([`Live::edge`]); the corrections for the coming
    /// step (the source's index, the step's time into it, its departures before and after).
    mu_pk: Vec<C64>,
    edges: Vec<(usize, f64, f64)>,
    corr: Vec<(usize, f64, f64, f64)>,
    /// In a strike played whole, its sub-steps a sample and their shares (the reference's: the
    /// few devices then live take them cheaply).
    sub_full: usize,
    split_full: Vec<f64>,
    /// In a strike on a ringing voice played back whole but for the ring's devices
    /// ([`Live::whole_ring`]): its samples left, its sub-steps a sample and their shares.
    ring_whole_left: usize,
    sub_ring: usize,
    split_ring: Vec<f64>,
    /// A sample split only for a source's step ([`Live::edge`]), nothing played back: its
    /// sub-steps (the fifth rate's).
    sub_edge: usize,
    /// Each control's last Newton step in this solve (`run::damped`).
    step_prev: Vec<f64>,
    split: Vec<f64>,
    fy_last: Vec<f64>,
    u_sub: Vec<f64>,
    w_start: Vec<f64>,
    fz_save: Vec<f64>,
    fy_save: Vec<f64>,
    /// Scratch, so that no step allocates: the fast modes' inputs ([`Live::fast_state`]); the
    /// pots' fold ([`Live::fold`]: the conductances' changes, `I - K_pp ΔG` factored and its
    /// pivots, a column); the blocks' finding ([`Live::factor_blocks`]: the devices' controls,
    /// the union's roots, the pairs' couplings, each root's group, the controls covered).
    fs_b0: Vec<f64>,
    fs_b1: Vec<f64>,
    fs_bq: Vec<f64>,
    fold_dg: Vec<f64>,
    fold_lu: Vec<f64>,
    fold_piv: Vec<usize>,
    fold_col: Vec<f64>,
    blk_devs: Vec<(usize, usize)>,
    blk_root: Vec<usize>,
    blk_pairs: Vec<(usize, usize, f64)>,
    blk_gid: Vec<usize>,
    blk_cov: Vec<bool>,
    /// Studies' switches, read once (`PLUGIN_KIT_CIRCUIT_JBLOCKS`, `PLUGIN_KIT_CIRCUIT_LIVE_DEBUG`,
    /// `PLUGIN_KIT_CIRCUIT_BLOCKS_DEBUG`): the environment is not read on the audio path.
    study_jblocks: Option<u64>,
    study_live_debug: bool,
    study_blocks_debug: bool,
    /// A sample whose Newton solve stopped at its cap taken again in sub-steps
    /// ([`Live::set_retry`]): the state saved at each sample's start (none when off).
    #[serde(skip)]
    saved: Option<Box<Saved>>,
}

/// The state a sample's step changes, saved at its start so that a sample whose Newton solve
/// stopped at its cap can be taken again in sub-steps ([`Live::set_retry`]). Newton's
/// Jacobian is not saved: it is made again after a restore, and each device evaluated again.
#[derive(Debug, Clone, Default)]
struct Saved {
    vecs: Vec<Vec<f64>>,
    scalars: [f64; 2],
    flags: [bool; 4],
    was_level: usize,
    counters: Counters,
    devs: Vec<SavedDev>,
}

/// A device's switch state saved ([`Saved`]): its held target, at the step's start, its last
/// move, whether it was sent to a rail, its last round and its bracket.
type SavedDev = (f64, f64, f64, bool, Option<(f64, f64)>, Option<[f64; 6]>);

impl Live {
    /// The live part of `sys` (in modal form `m`), at step `h` and `h / sub`: the devices whose
    /// `moving` flag is set (pot sections among them are folded in), the sources `sources` (by
    /// input index), and every probe.
    pub fn new(
        sys: &System,
        m: &Modal,
        h: f64,
        sub: usize,
        moving: &[bool],
        sources: &[usize],
    ) -> Live {
        Live::with_strike(sys, m, h, sub, moving, &vec![false; moving.len()], sources)
    }

    /// As [`Live::new`], the devices with `strike_only` set live only while a strike plays
    /// ([`Live::wake`]).
    pub fn with_strike(
        sys: &System,
        m: &Modal,
        h: f64,
        sub: usize,
        moving: &[bool],
        strike_only: &[bool],
        sources: &[usize],
    ) -> Live {
        let none = vec![false; moving.len()];
        Live::with_levels(
            sys,
            m,
            h,
            (sub, sub, sub, sub),
            moving,
            strike_only,
            &none,
            sources,
        )
    }

    /// As [`Live::with_strike`], the ring's devices with `frozen` set played back with the
    /// strike's devices while a strike from rest plays whole ([`Live::freeze`]); a device live
    /// only in strikes is played back then whatever its flag. `subs`: a sample's sub-steps,
    /// in a strike played whole, in one on a ringing voice played whole but for the ring, and
    /// in one split only for a source's step ([`Live::edge`]).
    #[allow(clippy::too_many_arguments)]
    pub fn with_levels(
        sys: &System,
        m: &Modal,
        h: f64,
        subs: (usize, usize, usize, usize),
        moving: &[bool],
        strike_only: &[bool],
        frozen: &[bool],
        sources: &[usize],
    ) -> Live {
        let pack = Pack::new(m, h);
        let (nr, nc, nz, nzs, nf) = (
            pack.real.len(),
            pack.pair.len(),
            pack.len(),
            pack.nzs,
            pack.fast.len(),
        );
        let mut mu_pk = vec![C64::ZERO; nz];
        for (p, &j) in pack.real.iter().enumerate() {
            mu_pk[p] = m.mu[j];
        }
        for (k, &j) in pack.pair.iter().enumerate() {
            mu_pk[nr + k] = m.mu[j];
        }
        for (f, &j) in pack.fast.iter().enumerate() {
            mu_pk[nzs + f] = m.mu[j];
        }
        let mut devs = Vec::new();
        let mut pots = Vec::new();
        let (mut nn, mut ns, mut nq) = (0, 0, 0);
        let (mut nd_r, mut nn_r, mut ns_r, mut nq_r) = (0, 0, 0, 0);
        let (mut nd_f, mut nn_f, mut ns_f, mut nq_f) = (0, 0, 0, 0);
        // The ring's devices left live in a strike played whole, the ring's others, the
        // strike's.
        for pass in 0..3 {
            for (((s, &mv), &so), &fr) in sys.slots.iter().zip(moving).zip(strike_only).zip(frozen)
            {
                let at = if so {
                    2
                } else if fr {
                    1
                } else {
                    0
                };
                if !mv || at != pass {
                    continue;
                }
                if let Dev::Cond(g0) = s.dev {
                    pots.push(LPot {
                        slot: s.clone(),
                        g0,
                        g: g0,
                    });
                } else {
                    devs.push(LDev::new(s, nn, ns, nq));
                    nn += s.nv;
                    ns += s.ns;
                    nq += s.nq;
                }
            }
            if pass == 0 {
                (nd_f, nn_f, ns_f, nq_f) = (devs.len(), nn, ns, nq);
            }
            if pass == 1 {
                (nd_r, nn_r, ns_r, nq_r) = (devs.len(), nn, ns, nq);
            }
        }
        let np = pots.len();
        let nprobe = sys.probes.len();
        // How far a control may move before Newton's Jacobian is made again: half the thermal
        // voltage at a junction (its conductance then changes by up to two thirds), a tenth of
        // a tanh's width; at once for the rest. The answer's tolerance is the same (a
        // correction under a microvolt); kept, the Jacobian changes the bass drum's output by
        // -122 dB re the hit at VT, -139 dB at a tenth of it (2026-10-07).
        let vt = crate::dev::vt();
        let jscale: f64 = std::env::var("PLUGIN_KIT_CIRCUIT_JTOL")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(1.0);
        // A switch's target is held through the solve (out of the Jacobian): the controls only
        // it reads (a comparator's inputs, a Schmitt trigger's) never call for a new one; its
        // state and output enter its output currents near linearly, so a fiftieth of their
        // limited step (1 to 2 % of those currents' slopes) is allowed. A conductance's slope
        // is constant; an op-amp's output clamp's (a softplus) moves by at most a fortieth of
        // its conductance over a tenth of its knee, as a tanh's over a tenth of its width (with
        // none, the voicing board's two clamps made a new Jacobian at nearly every iteration).
        let j_tol: Vec<f64> = devs
            .iter()
            .flat_map(|d| {
                let target_only: &[usize] = match d.slot.dev {
                    Dev::Cmp(_) => &[0, 1, 2],
                    Dev::Schmitt(_) => &[0],
                    Dev::Cond(_) => &[0],
                    _ => &[],
                };
                let knee = match d.slot.dev {
                    Dev::Clamp(p) => Some(p.knee),
                    _ => None,
                };
                d.lims.iter().enumerate().map(move |(c, l)| {
                    if target_only.contains(&c) {
                        return f64::INFINITY;
                    }
                    if let Some(k) = knee {
                        return 0.1 * k * jscale;
                    }
                    match *l {
                        Lim::Junction => 0.5 * vt * jscale,
                        Lim::Tanh(_, w) => 0.1 * w * jscale,
                        Lim::Step(st) => 0.02 * st * jscale,
                        Lim::None => 0.0,
                    }
                })
            })
            .collect();
        let knee: Vec<f64> = devs
            .iter()
            .flat_map(|d| {
                let k = match d.slot.dev {
                    Dev::Clamp(p) => p.knee,
                    _ => 0.0,
                };
                std::iter::repeat_n(k, d.lims.len())
            })
            .collect();
        let mut outs = Vec::new();
        for d in &devs[..nd_r] {
            outs.extend(d.slot.v_off..d.slot.v_off + d.slot.nv);
        }
        for p in &pots {
            outs.push(p.slot.v_off);
        }
        for d in &devs[nd_r..] {
            outs.extend(d.slot.v_off..d.slot.v_off + d.slot.nv);
        }
        outs.extend((0..nprobe).map(|k| sys.nv + k));
        let lane = |c: usize| if c < nn_r { c } else { c + np };
        let nop_r = (nn_r + np).div_ceil(W) * W;
        let mut ins: Vec<usize> = sources.to_vec();
        let srcs: Vec<(usize, f64)> = sources.iter().map(|&k| (k, sys.u0[k])).collect();
        for d in &devs {
            ins.extend(d.slot.s_off..d.slot.s_off + d.slot.ns);
        }
        for p in &pots {
            ins.push(p.slot.s_off);
        }
        for d in &devs {
            ins.extend(d.slot.q_off..d.slot.q_off + d.slot.nq);
        }
        let nsrc = srcs.len();
        let ni = ins.len();
        let no = outs.len();
        let mut b = Vec::with_capacity(ni * nz);
        for &k in &ins {
            let col: Vec<C64> = (0..m.mu.len()).map(|j| m.b[j][k]).collect();
            b.extend(pack.pack(&col));
        }
        let mut c = Vec::with_capacity(no * nz);
        for &yo in &outs {
            let row: Vec<C64> = m.c[yo].iter().map(|x| x.conj()).collect();
            c.extend(pack.pack(&row));
        }
        let nop = (nn + np).div_ceil(W) * W;
        let mut ct = vec![0.0; nz * nop];
        for o in 0..nn + np {
            for j in 0..nz {
                ct[j * nop + o] = c[o * nz + j];
            }
        }
        let mut d = Vec::with_capacity(nprobe * ni);
        for &yo in &outs[nn + np..] {
            d.extend(ins.iter().map(|&wi| m.d[(yo, wi)]));
        }
        let charge_from = nsrc + ns + np;
        let charge_cols: Vec<usize> = ins[charge_from..].to_vec();
        let ins_charge = |wi: usize| charge_cols.contains(&wi);
        let rate = |h: f64, whole: bool| {
            let mut r = Rate {
                h,
                inv_h: 1.0 / h,
                k: Mat::zeros(no, ni),
                whole,
                ..Default::default()
            };
            let wts: Vec<_> = m.mu.iter().map(|&mu| weights(mu, h)).collect();
            r.e = pack.pack(&wts.iter().map(|w| w.0).collect::<Vec<_>>());
            r.g0 = pack.pack(&wts.iter().map(|w| w.1).collect::<Vec<_>>());
            r.g1 = pack.pack(&wts.iter().map(|w| w.2).collect::<Vec<_>>());
            r.ga = pack.pack(&wts.iter().map(|w| w.3).collect::<Vec<_>>());
            for (o, &yo) in outs.iter().enumerate() {
                for (i, &wi) in ins.iter().enumerate() {
                    let mut s = 0.0;
                    for (j, w) in wts.iter().enumerate() {
                        let wgt = if i >= charge_from { w.3 } else { w.2 };
                        s += (m.c[yo][j] * wgt * m.b[j][wi]).re;
                    }
                    r.k[(o, i)] = s + m.d[(yo, wi)];
                }
            }
            if whole {
                // The fast modes: Re(c g b) summed over them, with the weight of the held
                // inputs at the step's start, or of those at its end and of the charges.
                let fast_k = |yo: usize, wi: usize, start: bool| -> f64 {
                    pack.fast
                        .iter()
                        .map(|&j| {
                            let w = &wts[j];
                            let g = if start {
                                w.1
                            } else if ins_charge(wi) {
                                w.3
                            } else {
                                w.2
                            };
                            (m.c[yo][j] * g * m.b[j][wi]).re
                        })
                        .sum()
                };
                r.k0f = vec![0.0; charge_from * nop];
                for (o, &yo) in outs[..nn + np].iter().enumerate() {
                    for (i, &wi) in ins[..charge_from].iter().enumerate() {
                        r.k0f[i * nop + o] = fast_k(yo, wi, true);
                    }
                }
                for (p, &yo) in outs[nn + np..].iter().enumerate() {
                    r.kp0f
                        .extend(ins[..charge_from].iter().map(|&wi| fast_k(yo, wi, true)));
                    r.dw.extend(
                        ins.iter()
                            .enumerate()
                            .map(|(i, &wi)| d[p * ni + i] + fast_k(yo, wi, false)),
                    );
                }
            }
            r
        };
        let (sub, sub_full, sub_ring, sub_edge) = subs;
        let rates = vec![
            rate(h, true),
            rate(h / sub as f64, false),
            rate(h / sub_full as f64, false),
            rate(h / sub_ring as f64, false),
            rate(h / sub_edge as f64, false),
        ];
        let y0: Vec<f64> = outs.iter().map(|&o| sys.y0[o]).collect();
        let mut live = Live {
            #[cfg(target_arch = "x86_64")]
            step_fn: select_step(),
            rates,
            pack,
            nr,
            nc,
            nz,
            nzs,
            nf,
            z: vec![0.0; nz],
            ct,
            nop,
            at: vec![0.0; nop],
            devs,
            pots,
            nn,
            np,
            nd_r,
            nn_r,
            ns_r,
            nq_r,
            nop_r,
            nd_f,
            nn_f,
            ns_f,
            nq_f,
            active_left: 0,
            frozen_left: 0,
            was_level: 0,
            handed: false,
            record: None,
            sys_nw: sys.nw(),
            sys_nv: sys.nv,
            nprobe,
            nsrc,
            ns,
            nq,
            outs,
            srcs,
            b,
            c,
            d,
            v: (0..nn).map(|c| y0[lane(c)]).collect(),
            y: y0.clone(),
            y0,
            bw: vec![0.0; nz],
            bq: vec![0.0; nz],
            w: vec![0.0; ni],
            w_prev: vec![0.0; ni],
            w_prev2: vec![0.0; ni],
            fzf_last: vec![0.0; nz - nzs],
            last_sub: false,
            rq: vec![0.0; nq],
            counters: Counters::default(),
            fz: vec![0.0; nz],
            fy: vec![0.0; no],
            forced: false,
            a: vec![0.0; no],
            g: vec![0.0; nn],
            dv: vec![0.0; nn],
            jg: vec![0.0; nn * nn],
            jraw: vec![0.0; nn * nn],
            lu_na: 0,
            refresh_moved: false,
            jbase: vec![0.0; nn * nn],
            wd_cols: Vec::with_capacity(nn),
            blocks: false,
            bperm: Vec::new(),
            bstart: Vec::new(),
            boff: Vec::new(),
            bjg: Vec::new(),
            bpiv: Vec::new(),
            bdrop: 0.0,
            btmp: vec![0.0; nn],
            bof: vec![0; nn],
            bage: 0,
            bfind: true,
            bdense: false,
            wd_w: vec![0.0; nn * nn],
            wd_s: vec![0.0; nn * nn],
            wd_piv: vec![0; nn],
            wd_t: vec![0.0; nn],
            updates: 0,
            piv: vec![0; nn],
            v_lu: vec![0.0; nn],
            j_tol,
            knee,
            lu_h: 0.0,
            lu_ok: false,
            factorisations: 0,
            r: vec![0.0; ns + nq],
            jr: vec![0.0; (ns + nq) * nn],
            vp: vec![0.0; np],
            dv_last: vec![0.0; nn],
            dv_last2: vec![0.0; nn],
            v_prev: vec![0.0; nn],
            r_prev_step: vec![0.0; ns + nq],
            r_hist1: vec![0.0; ns + nq],
            r_hist2: vec![0.0; ns + nq],
            r_pred: vec![0.0; ns + nq],
            predict_r: std::env::var("PLUGIN_KIT_CIRCUIT_PRED").map_or(true, |x| x != "v"),
            h_last: h,
            h_last2: h,
            vt: crate::dev::vt(),
            turned: false,
            hint_v: vec![0.0; nn],
            hinted: false,
            use_hints: std::env::var_os("PLUGIN_KIT_CIRCUIT_HINTS").is_some(),
            pred_order: std::env::var("PLUGIN_KIT_CIRCUIT_PRED_ORDER")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(3),
            lin_start: false,
            coupled_limit: false,
            coupled_reuse: false,
            coupled_slew: false,
            dependent: Vec::new(),
            independent: Vec::new(),
            reduced_rhs: Vec::new(),
            pred_all: vec![0.0; 3 * (ns + nq)],
            pred_err: vec![0.0; 3 * (ns + nq)],
            trace: std::env::var("PLUGIN_KIT_CIRCUIT_TRACE")
                .ok()
                .and_then(|x| {
                    let v: Vec<u64> = x.split(',').filter_map(|y| y.parse().ok()).collect();
                    (v.len() == 2).then(|| (v[0], v[1]))
                }),
            tol_v: TOL_V
                * std::env::var("PLUGIN_KIT_CIRCUIT_TOL_LEFT")
                    .ok()
                    .and_then(|x| x.parse::<f64>().ok())
                    .unwrap_or(1.0),
            tol_scale: std::env::var("PLUGIN_KIT_CIRCUIT_TOL")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(1.0),
            skip: std::env::var("PLUGIN_KIT_CIRCUIT_SKIP")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(SKIP),
            chord: std::env::var("PLUGIN_KIT_CIRCUIT_CHORD")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(1),
            refresh_by: if std::env::var_os("PLUGIN_KIT_CIRCUIT_REFRESH").is_some() {
                vec![0; nn]
            } else {
                Vec::new()
            },
            moved_by: if std::env::var_os("PLUGIN_KIT_CIRCUIT_REFRESH").is_some() {
                vec![0; nn]
            } else {
                Vec::new()
            },
            it_hist: if std::env::var_os("PLUGIN_KIT_CIRCUIT_REFRESH").is_some() {
                vec![0; 16]
            } else {
                Vec::new()
            },
            has_switch: false,
            sub,
            mu_pk,
            edges: Vec::new(),
            corr: Vec::new(),
            sub_full,
            split_full: Vec::new(),
            ring_whole_left: 0,
            sub_ring,
            sub_edge,
            split_ring: Vec::new(),
            step_prev: vec![0.0; nn],
            split: vec![0.0; nz],
            fy_last: vec![0.0; no],
            // (Full length from the start, so that a clone or a build read back has the room.)
            u_sub: vec![0.0; sys.u0.len()],
            w_start: vec![0.0; nsrc],
            fz_save: vec![0.0; nz],
            fy_save: vec![0.0; no],
            fs_b0: Vec::new(),
            fs_b1: Vec::new(),
            fs_bq: Vec::new(),
            fold_dg: Vec::new(),
            fold_lu: Vec::new(),
            fold_piv: Vec::new(),
            fold_col: Vec::new(),
            blk_devs: Vec::new(),
            blk_root: Vec::new(),
            blk_pairs: Vec::new(),
            blk_gid: Vec::new(),
            blk_cov: Vec::new(),
            study_jblocks: std::env::var("PLUGIN_KIT_CIRCUIT_JBLOCKS")
                .ok()
                .and_then(|x| x.parse().ok()),
            study_live_debug: std::env::var_os("PLUGIN_KIT_CIRCUIT_LIVE_DEBUG").is_some(),
            study_blocks_debug: std::env::var_os("PLUGIN_KIT_CIRCUIT_BLOCKS_DEBUG").is_some(),
            saved: None,
        };
        live.has_switch = live.devs.iter().any(|d| d.switch);
        live.split = split_shares(&live.rates[0].e, &live.rates[1].e, nr, nc, nzs, sub);
        live.split_full = split_shares(&live.rates[0].e, &live.rates[2].e, nr, nc, nzs, sub_full);
        live.split_ring = split_shares(&live.rates[0].e, &live.rates[3].e, nr, nc, nzs, sub_ring);
        live.fold();
        live.reserve_rt();
        live
    }

    /// The scratch at its largest, so that its first use on the audio path allocates nothing:
    /// made at the live part's making, and again after a clone or a read back (which keep a
    /// buffer's contents, not its room).
    pub fn reserve_rt(&mut self) {
        let (nf, nd, nn) = (self.nz - self.nzs, self.devs.len(), self.nn);
        for x in [&mut self.fs_b0, &mut self.fs_b1, &mut self.fs_bq] {
            x.reserve(nf);
        }
        self.blk_devs.reserve(nd);
        self.blk_root.reserve(nd);
        self.blk_gid.reserve(nd);
        self.blk_pairs.reserve(nd * nd / 2 + 1);
        self.blk_cov.reserve(nn);
        self.wd_cols.reserve(nn);
        self.bperm.reserve(nn);
        self.bstart.reserve(nn + 1);
        self.boff.reserve(nn);
        self.bpiv.reserve(nn);
        self.bjg.reserve(nn * nn);
        self.edges.reserve(8);
        self.corr.reserve(8);
    }

    /// Whether a strike now would find the devices a strike from rest's playback freezes at
    /// rest (within `tol`, V): no strike playing, the ring's such devices' controls within it of
    /// their rest, and the strike's devices' controls too (their slow modes' part: they are out
    /// of the step).
    pub fn at_rest(&self, tol: f64) -> bool {
        if self.level() != 0 || self.ring_whole_left > 0 {
            return false;
        }
        (self.nn_f..self.nn_r).all(|c| (self.v[c] - self.y0[c]).abs() < tol)
            && (self.nn_r..self.nn).all(|c| {
                let o = self.lane(c);
                let x: f64 = (0..self.nzs)
                    .map(|j| self.ct[j * self.nop + o] * self.z[j])
                    .sum();
                x.abs() < tol
            })
    }

    /// Whether the step is the ring's alone: no strike's devices live, none played back.
    pub fn idle(&self) -> bool {
        self.level() == 0 && self.ring_whole_left == 0
    }

    /// Slow modal state for the optional sleep study. The caller allocates off-thread.
    pub fn sleep_state(&self) -> &[f64] {
        &self.z[..self.nzs]
    }

    /// Movement of slow device-control voltages over a quiet window. Project before
    /// taking the absolute value: modal coordinates can be ill-conditioned and have
    /// large cancelling contributions. This includes the strike-only devices' bias.
    /// The next-hit margin is measured against a never-sleeping reference, rather
    /// than inferred from an individual modal coordinate's apparent decay time.
    pub fn sleep_drift(&self, previous: &[f64], seconds: f64) -> f64 {
        if !seconds.is_finite()
            || seconds <= 0.
            || self
                .sleep_state()
                .iter()
                .chain(previous)
                .any(|x| !x.is_finite())
        {
            return f64::INFINITY;
        }
        (0..self.nn)
            .map(|c| {
                let o = self.lane(c);
                (0..self.nzs)
                    .filter(|&j| {
                        let m = self.mu_pk[j];
                        m.im == 0. && m.re < 0. && m.re > -50.
                    })
                    .map(|j| self.ct[j * self.nop + o] * (self.z[j] - previous[j]))
                    .sum::<f64>()
                    .abs()
            })
            .fold(0., f64::max)
    }

    /// The step's level (see `was_level`).
    #[inline(always)]
    fn level(&self) -> usize {
        if self.active_left > 0 {
            2
        } else if self.frozen_left > 0 {
            1
        } else {
            0
        }
    }

    /// Offline description of the device prefixes in a ring, a whole strike,
    /// and a live or coupled strike. Never called inside the callback.
    pub fn strike_layout(&self) -> [Vec<String>; 3] {
        std::array::from_fn(|level| {
            self.devs[..self.prefix(level).1]
                .iter()
                .map(|d| {
                    format!("{} {}", d.slot.refdes, d.slot.role)
                        .trim()
                        .to_string()
                })
                .collect()
        })
    }

    /// The controls, devices, statics and charges in a step of level `l`.
    #[inline(always)]
    fn prefix(&self, l: usize) -> (usize, usize, usize, usize) {
        match l {
            0 => (self.nn_r, self.nd_r, self.ns_r, self.nq_r),
            1 => (self.nn_f, self.nd_f, self.ns_f, self.nq_f),
            _ => (self.nn, self.devs.len(), self.ns, self.nq),
        }
    }

    /// A strike from rest played back whole for the next `samples` samples (its played-back
    /// form then carries every device but the ring's left live, `nd_f`); 0 ends it.
    /// A strike on a ringing voice played back whole but for the ring's devices for the next
    /// `samples` samples (the strike's devices out of the step, as between strikes: its sub-steps
    /// after an edge its own); 0 ends it.
    pub fn whole_ring(&mut self, samples: usize) {
        self.ring_whole_left = samples;
    }

    /// The sub-step rate (its index in `rates`), the sub-steps a sample and their shares, for a
    /// sample taken in sub-steps now.
    fn sub_rate(&self) -> (usize, usize) {
        if self.level() == 1 {
            (2, self.sub_full)
        } else if self.ring_whole_left > 0 && self.level() == 0 {
            (3, self.sub_ring)
        } else {
            (1, self.sub)
        }
    }

    pub fn freeze(&mut self, samples: usize) {
        self.frozen_left = samples;
    }

    /// A step's devices' state from a recording (`w` as the system orders it, of `nw` inputs,
    /// then its controls; see `record`), as [`Live::handover`] takes it: the controls, the
    /// statics' remainders, the charges' currents, each in this live part's order.
    pub fn state_of(&self, w: &[f64], nw: usize) -> Vec<f64> {
        let (nn, ns) = (self.nn, self.ns);
        let mut st = vec![0.0; nn + ns + self.nq];
        for d in &self.devs {
            let s = &d.slot;
            for c in 0..s.nv {
                st[d.v_at + c] = w[nw + s.v_off + c];
            }
            for o in 0..s.ns {
                st[nn + d.s_at + o] = w[s.s_off + o];
            }
            for q in 0..s.nq {
                st[nn + ns + d.q_at + q] = w[s.q_off + q];
            }
        }
        st
    }

    /// The end of a strike played back whole: the devices it froze take the full model's state
    /// at the last sample played, `st` (as [`Live::state_of`] gives it; `p1`, `p2` the two
    /// samples' before, for the prediction), the ring's, and the strike's too if `strike` (a
    /// strike follows at once, on the voice ringing), and the next step is the ring's (or
    /// every device's).
    pub fn handover(&mut self, st: &[f64], p1: &[f64], p2: &[f64], strike: bool) {
        self.leave_curves();
        let (nn, ns, nz, nsrc, np) = (self.nn, self.ns, self.nz, self.nsrc, self.np);
        let to = if strike { self.devs.len() } else { self.nd_r };
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        for d in self.devs[self.nd_f..to].iter_mut() {
            let s = &d.slot;
            for c in d.v_at..d.v_at + s.nv {
                self.v[c] = st[c];
                self.dv_last[c] = st[c] - p1[c];
                self.dv_last2[c] = p1[c] - p2[c];
            }
            // The remainders, as inputs at the step's end too (the next step's start: their
            // `B w`, which the step's end left out, added).
            let statics = d.s_at..d.s_at + s.ns;
            let charges = ns + d.q_at..ns + d.q_at + s.nq;
            for k in statics.clone().chain(charges.clone()) {
                self.r[k] = st[nn + k];
                self.r_prev_step[k] = st[nn + k];
                self.r_hist1[k] = p1[nn + k];
                self.r_hist2[k] = p2[nn + k];
            }
            for k in statics {
                self.w[nsrc + k] = st[nn + k];
                axpy(
                    &mut self.bw,
                    st[nn + k],
                    &self.b[(nsrc + k) * nz..(nsrc + k + 1) * nz],
                );
            }
            for k in charges {
                self.w[nsrc + np + k] = st[nn + k];
            }
            // The charges at the controls, and a switch's target there.
            let x = &self.v[d.v_at..d.v_at + s.nv];
            s.dev.eval(x, &mut out, &mut jac);
            for q in 0..s.nq {
                let o = s.ns + q;
                let (cc, c0) = d.cd[q];
                self.rq[d.q_at + q] = out[o] - d.f0[o] - c0 * (x[cc] - d.v0[cc]);
            }
            if d.switch {
                d.held = out[0];
            }
            d.evaluated = false;
        }
        self.frozen_left = 0;
        self.handed = true;
    }

    /// Each nonlinear control's index among its system's controls.
    pub fn control_slots(&self) -> Vec<usize> {
        let mut out = vec![0; self.nn];
        for d in &self.devs {
            for c in 0..d.slot.nv {
                out[d.v_at + c] = d.slot.v_off + c;
            }
        }
        out
    }

    /// The next step's Newton starts from `v` (its controls, as [`Live::control_slots`] orders
    /// them: a played-back strike's, as the full model solved it).
    pub fn hint(&mut self, v: &[f64]) {
        if self.hint_v.len() == v.len() {
            self.hint_v.copy_from_slice(v);
            self.hinted = true;
        }
    }

    /// Each nonlinear control's device and index (`refdes role #c`).
    pub fn control_names(&self) -> Vec<String> {
        let mut out = vec![String::new(); self.nn];
        for d in &self.devs {
            for c in 0..d.slot.nv {
                out[d.v_at + c] = format!("{} {} #{c}", d.slot.refdes, d.slot.role);
            }
        }
        out
    }

    pub fn shape(&self) -> String {
        format!(
            "{} modes ({} real, {} of them algebraic over a sample), {} controls ({} only in strikes, {} live in a strike played whole), {} pot sections, {} statics, {} charges, {} probes, {} sources",
            self.nr + self.nc + self.nf,
            self.nr + self.nf,
            self.nf,
            self.nn,
            self.nn - self.nn_r,
            self.nn_f,
            self.np,
            self.ns,
            self.nq,
            self.nprobe,
            self.nsrc
        )
    }

    /// The kept outputs' indices in the system's `y`.
    pub fn outs(&self) -> &[usize] {
        &self.outs
    }

    /// The index of probe `p` among the kept outputs.
    pub fn probe_at(&self, p: usize) -> usize {
        self.nn + self.np + p
    }

    /// Modal values (one per mode of the model this was built from) packed as the state is.
    pub fn pack(&self, x: &[C64]) -> Vec<f64> {
        self.pack.pack(x)
    }

    /// The devices live only in strikes made live for the next `samples` samples (a strike's
    /// length), or longer if they already are.
    pub fn wake(&mut self, samples: usize) {
        self.active_left = self.active_left.max(samples);
    }

    /// A sample done: the strike devices' time counted down.
    pub fn end_sample(&mut self) {
        self.active_left = self.active_left.saturating_sub(1);
        self.frozen_left = self.frozen_left.saturating_sub(1);
        self.ring_whole_left = self.ring_whole_left.saturating_sub(1);
    }

    /// The output lane of control `c`.
    #[inline(always)]
    fn lane(&self, c: usize) -> usize {
        if c < self.nn_r { c } else { c + self.np }
    }

    /// The step's level, and the controls and devices in it.
    #[inline(always)]
    fn active(&self) -> (usize, usize, usize) {
        let l = self.level();
        let (nn, nd, _, _) = self.prefix(l);
        (l, nn, nd)
    }

    /// Sets a panel pot's knob (0 to 1), acting from the next step.
    pub fn set_pot(&mut self, refdes: &str, pos: f64) {
        self.set_pot_scaled(refdes, pos, 1.0);
    }

    /// Whether applying this pot value would leave every conductance unchanged.
    pub fn pot_is_set(&self, refdes: &str, pos: f64, scale: f64) -> bool {
        self.pots
            .iter()
            .filter(|p| p.slot.refdes == refdes)
            .all(|p| p.slot.pot_g(pos).is_none_or(|g| p.g == g / scale))
    }

    /// As [`Live::set_pot`], the pot's resistance `scale` times its value (a part's tolerance:
    /// ENTROPY, D73); at 1 exactly as [`Live::set_pot`].
    pub fn set_pot_scaled(&mut self, refdes: &str, pos: f64, scale: f64) {
        let mut any = false;
        for p in &mut self.pots {
            if p.slot.refdes == refdes
                && let Some(g) = p.slot.pot_g(pos)
            {
                p.g = g / scale;
                any = true;
            }
        }
        if any {
            self.fold();
        }
    }

    /// Recomputes the pots' fold for the knobs as they are.
    fn fold(&mut self) {
        self.lu_ok = false;
        let (nn, np, nsrc, ns, nn_r) = (self.nn, self.np, self.nsrc, self.ns, self.nn_r);
        let nsq = ns + self.nq;
        let p_in = nsrc + ns;
        // (In buffers kept from fold to fold: a knob moved mid-note allocates nothing.)
        let mut dg = std::mem::take(&mut self.fold_dg);
        let mut lu = std::mem::take(&mut self.fold_lu);
        let mut piv = std::mem::take(&mut self.fold_piv);
        let mut col = std::mem::take(&mut self.fold_col);
        dg.clear();
        dg.extend(self.pots.iter().map(|p| p.g - p.g0));
        lu.resize(np * np, 0.0);
        piv.resize(np, 0);
        col.resize(np, 0.0);
        let lane = |c: usize| if c < nn_r { c } else { c + np };
        // The nonlinear remainders' columns among the kept inputs: the statics, then the
        // charges (after the pots).
        let rcol = |c: usize| {
            if c < ns {
                nsrc + c
            } else {
                nsrc + ns + np + (c - ns)
            }
        };
        for r in &mut self.rates {
            // M = (I - K_pp ΔG)⁻¹, column by column; ΔG M into `dg_m`.
            for i in 0..np {
                for j in 0..np {
                    lu[i * np + j] =
                        f64::from(u8::from(i == j)) - r.k[(nn_r + i, p_in + j)] * dg[j];
                }
            }
            r.dg_m.resize(np * np, 0.0);
            if np > 0 && lu_factor(&mut lu, np, &mut piv) {
                for j in 0..np {
                    col.fill(0.0);
                    col[j] = 1.0;
                    lu_solve(&lu, np, &piv, &mut col);
                    for i in 0..np {
                        r.dg_m[i * np + j] = dg[i] * col[i];
                    }
                }
            } else {
                r.dg_m.fill(0.0);
            }
            r.knp_m.resize(nn * np, 0.0);
            for i in 0..nn {
                for j in 0..np {
                    r.knp_m[i * np + j] = (0..np)
                        .map(|l| r.k[(lane(i), p_in + l)] * r.dg_m[l * np + j])
                        .sum();
                }
            }
            r.kp_r.resize(np * nsq, 0.0);
            for x in 0..np * nsq {
                r.kp_r[x] = r.k[(nn_r + x / nsq, rcol(x % nsq))];
            }
            r.kn_t.resize(nsq * nn, 0.0);
            for x in 0..nsq * nn {
                let (c, i) = (x / nn, x % nn);
                r.kn_t[x] = r.k[(lane(i), rcol(c))]
                    + (0..np)
                        .map(|l| r.knp_m[i * np + l] * r.kp_r[l * nsq + c])
                        .sum::<f64>();
            }
            r.k_src.resize((nn + np) * nsrc, 0.0);
            for x in 0..(nn + np) * nsrc {
                r.k_src[x] = r.k[(x / nsrc, x % nsrc)];
            }
        }
        self.fold_dg = dg;
        self.fold_lu = lu;
        self.fold_piv = piv;
        self.fold_col = col;
    }

    /// The fast modes' state at the end of the last whole sample, where it is algebraic (`r`
    /// that sample's rate): from the held inputs at its start and end and its charges, and its
    /// strike's share; and their `B w` there, for sub-steps that follow.
    fn fast_state(&mut self, r: &Rate) {
        let (nz, nzs) = (self.nz, self.nzs);
        let nf = nz - nzs;
        let nh = self.nsrc + self.ns + self.np;
        let mut b0 = std::mem::take(&mut self.fs_b0);
        let mut b1 = std::mem::take(&mut self.fs_b1);
        let mut bq = std::mem::take(&mut self.fs_bq);
        for x in [&mut b0, &mut b1, &mut bq] {
            x.clear();
            x.resize(nf, 0.0);
        }
        for (i, col) in self.b.chunks_exact(nz).enumerate() {
            let f = &col[nzs..];
            if i < nh {
                axpy(&mut b0, self.w_prev[i], f);
                axpy(&mut b1, self.w[i], f);
            } else {
                axpy(&mut bq, self.w[i], f);
            }
        }
        for j in 0..nf {
            let k = nzs + j;
            self.z[k] = r.g0[k] * b0[j] + r.g1[k] * b1[j] + r.ga[k] * bq[j] + self.fzf_last[j];
            self.bw[k] = b1[j];
        }
        self.fs_b0 = b0;
        self.fs_b1 = b1;
        self.fs_bq = bq;
    }

    /// Adds the fast modes' share of a played-back strike's increment `dz` (packed) to its
    /// direct part of the kept outputs `dy`, for a whole sample's step (where they are
    /// algebraic and their state is not kept).
    pub fn fold_fast(&self, dz: &[f64], dy: &mut [f64]) {
        let (nz, nzs) = (self.nz, self.nzs);
        for (o, y) in dy.iter_mut().enumerate() {
            *y += dot(&self.c[o * nz + nzs..(o + 1) * nz], &dz[nzs..]);
        }
    }

    /// Adds to the next step a played-back strike's increment of the modes (packed, see
    /// [`Live::pack`]) and its direct part of the kept outputs (for a whole sample's step, with
    /// the fast modes' share folded in, [`Live::fold_fast`]).
    pub fn force(&mut self, dz: &[f64], dy: &[f64]) {
        self.fz.copy_from_slice(dz);
        self.fy.copy_from_slice(dy);
        self.forced = true;
    }

    /// One sample's step, the kept sources at `u` (the system's full input vector). A sample
    /// in which a switch is about to turn is taken in sub-steps (its sources on the line between
    /// their values at its start and end, a played-back strike's increment shared among them).
    pub fn step(&mut self, u: &[f64]) {
        self.leave_curves();
        let rates = std::mem::take(&mut self.rates);
        let ahead = self.has_switch && self.switch_ahead(&rates[0]);
        if ahead || !self.edges.is_empty() {
            self.counters.resteps += 1;
            self.step_split(&rates, u, !ahead);
        } else {
            if self.saved.is_some() {
                self.save();
            }
            let capped = self.counters.capped;
            self.step_at(&rates[0], u, false);
            if self.counters.capped > capped && self.saved.is_some() {
                // Taken again from the sample's start in sub-steps, its sources on the line
                // between their values at its start and end, as a source's step is.
                self.restore();
                self.counters.resteps += 1;
                self.step_split(&rates, u, true);
            } else if self.turned {
                self.counters.missed += 1;
            }
        }
        self.rates = rates;
    }

    /// A sample whose Newton solve stops at its iteration cap taken again, from the state at
    /// its start, in the sub-steps of a source's step (`sub` of [`Live::new`]), as SPICE takes
    /// a time step again shorter when Newton fails: for a network that switches on its own
    /// state without a switch device (a latch of CMOS gates, its regeneration faster than a
    /// sample). Off by default; on, each sample's state is copied at its start (no allocation
    /// on the audio path: the copy's room is made here).
    pub fn set_retry(&mut self, on: bool) {
        self.saved = on.then(|| {
            let mut s = Box::<Saved>::default();
            self.save_into(&mut s);
            s
        });
    }

    fn save(&mut self) {
        let mut s = self.saved.take().expect("retry on");
        self.save_into(&mut s);
        self.saved = Some(s);
    }

    /// The vectors a step changes, in one order (`save_into`, `restore`).
    fn state_vecs(&mut self) -> [&mut Vec<f64>; 23] {
        [
            &mut self.z,
            &mut self.w,
            &mut self.w_prev,
            &mut self.w_prev2,
            &mut self.fzf_last,
            &mut self.rq,
            &mut self.v,
            &mut self.y,
            &mut self.r,
            &mut self.r_prev_step,
            &mut self.r_hist1,
            &mut self.r_hist2,
            &mut self.r_pred,
            &mut self.pred_err,
            &mut self.dv_last,
            &mut self.dv_last2,
            &mut self.v_prev,
            &mut self.step_prev,
            &mut self.fz,
            &mut self.fy,
            &mut self.fy_last,
            &mut self.hint_v,
            &mut self.pred_all,
        ]
    }

    fn save_into(&mut self, s: &mut Saved) {
        let vecs = self.state_vecs();
        s.vecs.resize(vecs.len(), Vec::new());
        for (to, from) in s.vecs.iter_mut().zip(vecs) {
            to.clear();
            to.extend_from_slice(from);
        }
        s.scalars = [self.h_last, self.h_last2];
        s.flags = [self.last_sub, self.turned, self.hinted, self.forced];
        s.was_level = self.was_level;
        s.counters.clone_from(&self.counters);
        s.devs.clear();
        s.devs.extend(self.devs.iter().map(|d| {
            (
                d.held,
                d.held_start,
                d.held_move,
                d.jumped,
                d.last,
                d.bracket,
            )
        }));
    }

    fn restore(&mut self) {
        let s = self.saved.take().expect("retry on");
        for (to, from) in self.state_vecs().into_iter().zip(&s.vecs) {
            to.copy_from_slice(from);
        }
        [self.h_last, self.h_last2] = s.scalars;
        [self.last_sub, self.turned, self.hinted, self.forced] = s.flags;
        self.was_level = s.was_level;
        self.counters.clone_from(&s.counters);
        for (d, x) in self.devs.iter_mut().zip(&s.devs) {
            (
                d.held,
                d.held_start,
                d.held_move,
                d.jumped,
                d.last,
                d.bracket,
            ) = *x;
            d.evaluated = false;
            d.stale = false;
        }
        self.lu_ok = false;
        self.wd_cols.clear();
        self.saved = Some(s);
    }

    /// Whether a switch's target, at its controls carried on at their last rate for three
    /// samples, has moved a twentieth of its swing from where it is held, or a regenerative
    /// switch's loop gain, a sample on, has reached [`REGEN`]: it turns on its own state, which
    /// the controls' extrapolation does not foresee (held through a step, its target turns it
    /// too early: a Schmitt oscillator's half-cycles 400 ppm short at 0.65 us a step).
    fn switch_ahead(&self, r: &Rate) -> bool {
        let (_, _, nd_a) = self.active();
        let scale = 3.0 * r.h / self.h_last;
        let mut x = [0.0; 5];
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        let scale_r = r.h / self.h_last;
        self.devs[..nd_a].iter().filter(|d| d.switch).any(|d| {
            let nv = d.slot.nv;
            for (c, xc) in x[..nv].iter_mut().enumerate() {
                *xc = self.v[d.v_at + c] + self.dv_last[d.v_at + c] * scale;
            }
            d.slot.dev.eval(&x[..nv], &mut out, &mut jac);
            if (out[0] - d.held).abs() > 0.05 * d.swing {
                return true;
            }
            let Some(c) = d.regen else {
                return false;
            };
            for (c, xc) in x[..nv].iter_mut().enumerate() {
                *xc = self.v[d.v_at + c] + self.dv_last[d.v_at + c] * scale_r;
            }
            d.slot.dev.eval(&x[..nv], &mut out, &mut jac);
            jac[c] > REGEN * d.swing
        })
    }

    /// A sample taken in sub-steps (see [`Live::step`]).
    fn step_split(&mut self, rates: &[Rate], u: &[f64], edge_only: bool) {
        // (In a strike played whole, its own sub-steps; for a source's step alone, nothing
        // played back, the edges'.)
        let (ri, n) = match self.sub_rate() {
            (1, _) if edge_only && !self.forced => (4, self.sub_edge),
            x => x,
        };
        // (A hint is the sample's end's: not a sub-step's.)
        self.hinted = false;
        self.u_sub.clear();
        self.u_sub.extend_from_slice(u);
        self.w_start.copy_from_slice(&self.w[..self.nsrc]);
        let forced = self.forced;
        if forced {
            // The played-back direct part at the sample's end, the fast modes' share (folded
            // in for a whole sample's step) taken out: in sub-steps they are states.
            let (nz, nzs) = (self.nz, self.nzs);
            self.fz_save.copy_from_slice(&self.fz);
            for (o, y) in self.fy_save.iter_mut().enumerate() {
                *y = self.fy[o] - dot(&self.c[o * nz + nzs..(o + 1) * nz], &self.fz[nzs..]);
            }
        }
        let edges = std::mem::take(&mut self.edges);
        for j in 1..=n {
            let f = j as f64 / n as f64;
            let f0 = (j - 1) as f64 / n as f64;
            for (i, &(k, u0)) in self.srcs.iter().enumerate() {
                let (a, b) = (self.w_start[i], u[k] - u0);
                self.u_sub[k] = u0 + a + (b - a) * f;
                // A source's step: its value before it until the sub-step that holds it, which
                // is corrected to it.
                if let Some(&(_, fe, old)) = edges.iter().find(|e| e.0 == k) {
                    if f < fe {
                        self.u_sub[k] = old;
                    } else if f0 < fe {
                        self.corr
                            .push((i, (fe - f0) * rates[0].h, old - u0, u[k] - u0));
                    }
                }
            }
            if forced {
                self.fz.fill(0.0);
                wmadd(
                    &mut self.fz,
                    match ri {
                        2 => &self.split_full,
                        3 => &self.split_ring,
                        _ => &self.split,
                    },
                    &self.fz_save,
                    self.nr,
                    self.nc,
                    self.nzs,
                );
                for ((y, a), b) in self.fy.iter_mut().zip(&self.fy_last).zip(&self.fy_save) {
                    *y = a + (b - a) * f;
                }
                self.forced = true;
            }
            if !self.last_sub && self.nf > 0 {
                self.fast_state(&rates[0]);
            }
            let u_sub = std::mem::take(&mut self.u_sub);
            self.step_at(&rates[ri], &u_sub, true);
            self.u_sub = u_sub;
        }
        // (The buffer kept: the next sample's edges go into it without allocating.)
        let mut edges = edges;
        edges.clear();
        self.edges = edges;
    }

    /// Newton's tolerances, so many times their own (`TOL_V` and the smallest step).
    pub fn set_tol_scale(&mut self, x: f64) {
        self.tol_scale = x;
    }

    /// The blocks Newton by blocks last found (each one's controls), and the controls.
    pub fn block_sizes(&self) -> (Vec<usize>, usize) {
        let (_, na, _) = self.active();
        (self.bstart.windows(2).map(|w| w[1] - w[0]).collect(), na)
    }

    /// Newton by blocks on or off: the Jacobian made again next.
    pub fn set_blocks(&mut self, on: bool) {
        self.blocks = on;
        self.lu_ok = false;
        self.bfind = true;
        self.wd_cols.clear();
    }

    /// Choose this only for clamps across fixed ideal rails, at construction. Their two
    /// residuals sum to the output-independent rail relation, so the second equation is
    /// imposed directly while factoring the first and all other equations.
    pub fn reduce_clamps(&mut self) {
        assert!(self.coupled_limit && !self.blocks);
        self.dependent = self
            .devs
            .iter()
            .filter_map(|d| matches!(d.slot.dev, Dev::Clamp(_)).then_some((d.v_at + 1, d.v_at)))
            .collect();
        for &(drop, keep) in &self.dependent {
            for r in &self.rates {
                for col in r.kn_t.chunks_exact(self.nn) {
                    assert!(
                        (col[drop] + col[keep]).abs()
                            < 1e-9 * (1.0 + col[drop].abs() + col[keep].abs()),
                        "the opposing clamps must share fixed ideal rails"
                    );
                }
            }
        }
        self.independent = (0..self.nn)
            .filter(|i| !self.dependent.iter().any(|&(drop, _)| drop == *i))
            .collect();
        self.reduced_rhs.resize(self.nn, 0.0);
        self.lu_ok = false;
    }

    /// A whole sample's step, s.
    pub fn h(&self) -> f64 {
        self.rates.first().map_or(0.0, |r| r.h)
    }

    /// A source's step inside the coming sample (`Live::step`): input `k` at `old` until the
    /// sample's fraction `f` (in (0, 1]), then at its value at the sample's end. The sample is
    /// taken in sub-steps, the one that holds the step corrected to it (as the metal bank's
    /// square waves come to the cowbell: their edges' times kept within the sample).
    pub fn edge(&mut self, k: usize, f: f64, old: f64) {
        self.edges.push((k, f, old));
    }

    /// For the coming sub-step ([`Live::step_sub`], `h` long): input `k`'s step from `old` to
    /// `new` `delta` into it (the sub-step itself takes it at `old` at its start and `new` at
    /// its end).
    pub fn edge_in_sub(&mut self, k: usize, delta: f64, old: f64, new: f64) {
        if let Some(i) = self.srcs.iter().position(|s| s.0 == k) {
            let u0 = self.srcs[i].1;
            self.corr.push((i, delta, old - u0, new - u0));
        }
    }

    /// The state's correction for source `i`'s step `delta` into a step of `hs` (its
    /// departures `old` before and `new` after), which the step took as first-order held from
    /// `old` at its start to `new` at its end: each mode's weights of the two values, exact
    /// for the step (`∫ e^{μ(hs - s)}` over each stretch) less the hold's.
    fn src_step(&mut self, i: usize, hs: f64, delta: f64, old: f64, new: f64) {
        let (nz, nr, nc, nzs, nf) = (self.nz, self.nr, self.nc, self.nzs, self.nf);
        let b = &self.b[i * nz..(i + 1) * nz];
        let coef = |mu: C64| -> C64 {
            let (_, g0, g1, ga) = weights(mu, hs);
            let gb = weights(mu, hs - delta).3;
            (ga - gb - g0).scale(old) + (gb - g1).scale(new)
        };
        for p in (0..nr).chain(nzs..nzs + nf) {
            self.z[p] += coef(self.mu_pk[p]).re * b[p];
        }
        for k in 0..nc {
            let (a, bi) = (nr + k, nr + nc + k);
            let d = coef(self.mu_pk[a]) * C64::new(b[a], b[bi]);
            self.z[a] += d.re;
            self.z[bi] += d.im;
        }
    }

    /// One of a sample's sub-steps.
    pub fn step_sub(&mut self, u: &[f64]) {
        let (_, n) = self.sub_rate();
        self.step_sub_in(u, n);
    }

    /// As [`Live::step_sub`], the sample taken in `n` sub-steps: the step's own, or another of
    /// its rates' (a strike played whole and another voice's strike in one sample, each in its
    /// own sub-steps: the sample in the finer, `voice::LiveVoice`).
    pub fn step_sub_in(&mut self, u: &[f64], n: usize) {
        self.leave_curves();
        let rates = std::mem::take(&mut self.rates);
        if !self.last_sub && self.nf > 0 {
            self.fast_state(&rates[0]);
        }
        let (own, own_n) = self.sub_rate();
        let ri = if n == own_n {
            own
        } else {
            [
                (1, self.sub),
                (2, self.sub_full),
                (3, self.sub_ring),
                (4, self.sub_edge),
            ]
            .into_iter()
            .find(|&(_, m)| m == n)
            .map_or(own, |(i, _)| i)
        };
        debug_assert_eq!(
            rates[ri].h,
            rates[0].h / n as f64,
            "no rate of {n} sub-steps"
        );
        self.step_at(&rates[ri], u, true);
        self.rates = rates;
    }

    /// The table ring omits nonlinear charge and Newton prediction. Before Newton
    /// takes over, its charge integral must agree with the present controls, and
    /// its predictors must hold that state rather than extrapolate across the ring.
    /// Level 3 is kept in the serialised state, so cloning or saving a ringing
    /// voice cannot lose this pending handover. HI never enters that level.
    fn leave_curves(&mut self) {
        if self.was_level != 3 {
            return;
        }
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        for d in &mut self.devs {
            let s = &d.slot;
            let x = &self.v[d.v_at..d.v_at + s.nv];
            s.dev.eval(x, &mut out, &mut jac);
            for q in 0..s.nq {
                let o = s.ns + q;
                let (cc, c0) = d.cd[q];
                self.rq[d.q_at + q] = out[o] - d.f0[o] - c0 * (x[cc] - d.v0[cc]);
            }
            if d.switch {
                d.held = out[0];
            }
            d.evaluated = false;
        }
        self.dv_last.fill(0.0);
        self.dv_last2.fill(0.0);
        // A whole playback may now freeze part of the ring. Its departing
        // remainders become zero in step_at; hold zero in their predictors too,
        // including when a study explicitly selects linear or quadratic prediction.
        let (_, _, ns, nq) = self.prefix(self.level());
        self.r[ns..self.ns].fill(0.0);
        self.r[self.ns + nq..].fill(0.0);
        self.r_prev_step.copy_from_slice(&self.r);
        self.r_hist1.copy_from_slice(&self.r);
        self.r_hist2.copy_from_slice(&self.r);
        self.r_pred.copy_from_slice(&self.r);
        self.pred_err.fill(0.0);
        self.lu_ok = false;
        self.was_level = 0;
    }

    fn step_at(&mut self, r: &Rate, u: &[f64], sub: bool) {
        // Selection and environment access happen during construction/decoding,
        // on the preparing thread. The selected ISA is supported for this process.
        #[cfg(target_arch = "x86_64")]
        #[allow(unsafe_code)]
        // SAFETY: `step_fn` is `select_step`'s: the x86-64-v3 copy only where this process's
        // CPU has every feature it is compiled for, else the baseline.
        unsafe {
            (self.step_fn)(self, r, u, sub);
        }
        #[cfg(not(target_arch = "x86_64"))]
        self.step_impl(r, u, sub);
    }

    #[inline(always)]
    fn step_impl(&mut self, r: &Rate, u: &[f64], sub: bool) {
        #[cfg(feature = "prof")]
        let mut t = std::time::Instant::now();
        let (level, nn_a, _) = self.active();
        if level != self.was_level {
            self.lu_ok = false;
            let (nn_o, nd_o, ns_o, nq_o) = self.prefix(self.was_level);
            let (nn_n, nd_n, ns_n, nq_n) = self.prefix(level);
            let ns = self.ns;
            if nd_n < nd_o {
                // Out of the step (at rest, or played back): their departures out.
                self.r[ns_n..ns_o].fill(0.0);
                self.r[ns + nq_n..ns + nq_o].fill(0.0);
            } else if nd_n > nd_o {
                for d in &mut self.devs[nd_o..nd_n] {
                    d.evaluated = false;
                }
                if !self.handed {
                    // Waking: at rest.
                    for c in nn_o..nn_n {
                        self.v[c] = self.y0[self.lane(c)];
                        self.dv_last[c] = 0.0;
                        self.dv_last2[c] = 0.0;
                    }
                    for d in &mut self.devs[nd_o..nd_n] {
                        d.held = d.f0[0];
                    }
                    self.rq[nq_o..nq_n].fill(0.0);
                }
            }
            self.was_level = level;
        }
        self.handed = false;
        std::mem::swap(&mut self.w_prev2, &mut self.w_prev);
        self.w_prev.copy_from_slice(&self.w);
        // A whole sample's step keeps the slow block; a sub-step every mode.
        let nzk = if r.whole { self.nzs } else { self.nz };
        free_state(
            &mut self.z[..nzk],
            r,
            &self.bw,
            self.forced.then_some(&self.fz[..]),
            self.nr,
            self.nc,
            self.nzs,
        );
        if !self.corr.is_empty() {
            let mut corr = std::mem::take(&mut self.corr);
            for &(i, delta, old, new) in &corr {
                self.src_step(i, r.h, delta, old, new);
            }
            corr.clear();
            self.corr = corr;
        }
        tick!(t, 0);
        for (i, &(k, u0)) in self.srcs.iter().enumerate() {
            self.w[i] = u[k] - u0;
        }
        self.free_outputs(r);
        tick!(t, 1);
        // Newton on the nonlinear controls, from the last steps' solutions extrapolated: by a
        // parabola through the last three whole samples, else (in a sample's sub-steps, where
        // a strike's edge makes a kink a parabola would double) by the last change scaled to
        // this step.
        let mut v = std::mem::take(&mut self.v);
        self.v_prev.copy_from_slice(&v);
        let parabola = !sub && self.h_last == r.h && self.h_last2 == r.h;
        if self.predict_r {
            // The devices' remainders extrapolated (the same parabola, or line), and the
            // controls they and this step's free response give: a source the free response
            // carries (the noise) is in the start, and only the devices' part is predicted.
            // Each remainder by the extrapolation that has predicted it best (the last value,
            // the line, the parabola: a noise-driven device's is best held, a smooth one's
            // carried on; their mean square errors kept over the last steps).
            let scale = r.h / self.h_last;
            for k in 0..self.r.len() {
                let (r0, r1, r2) = (self.r[k], self.r_hist1[k], self.r_hist2[k]);
                let p = [
                    r0,
                    r0 + (r0 - r1) * scale,
                    if parabola {
                        3.0 * r0 - 3.0 * r1 + r2
                    } else {
                        r0 + (r0 - r1) * scale
                    },
                ];
                let e = &self.pred_err[3 * k..3 * k + 3];
                let best = match self.pred_order {
                    o @ 0..=2 => o,
                    _ => (0..3).fold(0, |b, o| if e[o] < e[b] { o } else { b }),
                };
                self.r_pred[k] = p[best];
                self.pred_all[3 * k..3 * k + 3].copy_from_slice(&p);
            }
            let nn = self.nn;
            for c in 0..nn_a {
                v[c] = self.a[self.lane(c)];
            }
            for (k, &x) in self.r_pred.iter().enumerate() {
                if x != 0.0 {
                    let col = &r.kn_t[k * nn..k * nn + nn_a];
                    for (vc, kc) in v[..nn_a].iter_mut().zip(col) {
                        *vc += kc * x;
                    }
                }
            }
        } else if parabola {
            for ((x, d1), d2) in v.iter_mut().zip(&self.dv_last).zip(&self.dv_last2) {
                *x += 2.0 * d1 - d2;
            }
        } else {
            let scale = r.h / self.h_last;
            for (x, d1) in v.iter_mut().zip(&self.dv_last) {
                *x += d1 * scale;
            }
        }
        // Or the start a Newton step from the last solution, its remainders and factored
        // Jacobian as they are (no evaluation, no factorisation): better for a device the noise
        // drives (its slopes taken into account), not taken while a switch is in the step (near
        // its threshold a hysteretic switch has two solutions, and the start would choose).
        let (_, _, nd_now) = self.active();
        if self.lin_start
            && self.lu_ok
            && self.lu_h == r.h
            && self.lu_na == nn_a
            && !self.devs[..nd_now].iter().any(|d| d.switch)
        {
            let nn = self.nn;
            for c in 0..nn_a {
                self.g[c] = self.v_prev[c] - self.a[self.lane(c)];
            }
            for (k, &x) in self.r[..self.ns].iter().enumerate() {
                if x != 0.0 {
                    axpy(&mut self.g[..nn_a], -x, &r.kn_t[k * nn..k * nn + nn_a]);
                }
            }
            self.dv[..nn_a].copy_from_slice(&self.g[..nn_a]);
            self.solve_j(nn_a);
            for c in 0..nn_a {
                v[c] = self.v_prev[c] - self.dv[c];
            }
        }
        // A played-back strike's controls, the full model's solution, start the devices live
        // only in strikes (the trigger's path, the same whatever rings) as they are; the
        // ring's devices (near a solution of the full model's, not of this step's, which may
        // have another one near: the rim shot's accented kick) the prediction limited as a
        // Newton step from the last solution would be (a junction predicted far forward would
        // be walked back a thermal voltage an iteration).
        let hinted = self.hinted && self.use_hints;
        let from = self.nn_r;
        for d in &self.devs {
            let at = d.v_at..d.v_at + d.slot.nv;
            if hinted && at.start >= from && at.start < nn_a {
                v[at.clone()].copy_from_slice(&self.hint_v[at]);
            } else {
                limit_controls(
                    &d.slot.dev,
                    &d.lims,
                    &mut v[at.clone()],
                    &self.v_prev[at],
                    self.vt,
                );
            }
        }
        self.hinted = false;
        if self.coupled_limit {
            v.copy_from_slice(&self.v_prev);
        }
        let mut it = 0;
        let mut converged = false;
        let (_, _, nd_a) = self.active();
        for d in self.devs[..nd_a].iter_mut() {
            d.held_start = d.held;
            d.held_move = 0.0;
            d.jumped = false;
            d.last = None;
            d.bracket = None;
        }
        let mut round = 0;
        let mut it_round = 0;
        let mut coupled_fresh = false;
        let mut coupled_ready = false;
        self.step_prev.fill(0.0);
        loop {
            it += 1;
            // Slow to converge from the prediction: once, from the last solution instead.
            if it - it_round == RESTART && round == 0 {
                v.copy_from_slice(&self.v_prev);
                self.lu_ok = false;
                coupled_ready = false;
            }
            // A coupled line search already evaluated its accepted controls and residual.
            // Keep those values until a restart or a switching target changes them.
            if !coupled_ready {
                self.eval_devices(r, &v);
            }
            tick!(t, 2);
            // A new Jacobian: whole when the step size or the devices in the step changed (or
            // a restart); else the columns of the devices whose controls moved past their
            // tolerance (or, after an iteration on an older one that left work, at all): a
            // device's slopes reach only its own controls' columns.
            // (Newton by blocks slow to converge: this step on the whole Jacobian.)
            if self.blocks && !self.bdense && it - it_round > BLOCK_SLOW {
                self.bdense = true;
                self.lu_ok = false;
            }
            let whole = (self.coupled_limit
                && (!self.coupled_reuse || coupled_fresh || self.refresh_moved))
                || !self.lu_ok
                || self.lu_h != r.h
                || self.lu_na != nn_a;
            let mut fresh = whole;
            if !whole && !(self.coupled_limit && self.coupled_reuse) {
                let all_moved = self.refresh_moved;
                for i in 0..nd_a {
                    let (a, b) = (self.devs[i].v_at, self.devs[i].v_at + self.devs[i].slot.nv);
                    let stale = (a..b).any(|c| {
                        self.jac_move(c, v[c]) > 1.0 || (all_moved && v[c] != self.v_lu[c])
                    });
                    self.devs[i].stale = stale;
                    fresh |= stale;
                }
            }
            self.refresh_moved = false;
            if fresh && self.lu_ok && self.lu_h == r.h && !self.refresh_by.is_empty() {
                for c in 0..nn_a {
                    if self.jac_move(c, v[c]) > 1.0 {
                        self.refresh_by[c] += 1;
                    }
                }
            }
            if fresh {
                tick!(t, 3);
                let mut refactor = whole;
                if whole {
                    self.newton_jacobian(r);
                    self.v_lu.copy_from_slice(&v);
                } else {
                    self.jacobian_columns(r);
                    for d in self.devs[..nd_a].iter().filter(|d| d.stale) {
                        let (a, b) = (d.v_at, d.v_at + d.slot.nv);
                        self.v_lu[a..b].copy_from_slice(&v[a..b]);
                        for c in a..b {
                            if !self.wd_cols.contains(&c) {
                                self.wd_cols.push(c);
                            }
                        }
                    }
                    // A few columns changed: the factored Jacobian kept, corrected by them
                    // (Woodbury); more than a third, factored again. (By blocks: factored
                    // again, a block's factorisation costing next to nothing.)
                    refactor = (self.blocks && !self.bdense) || 3 * self.wd_cols.len() > nn_a;
                    if !refactor {
                        self.woodbury(nn_a);
                    }
                }
                if refactor && self.blocks && !self.bdense {
                    if whole || self.bfind || self.bage >= BLOCK_REFIND {
                        self.factor_blocks(nn_a);
                        self.bage = 0;
                        self.bfind = false;
                    } else {
                        self.refactor_stale_blocks(nn_a);
                        self.bage += 1;
                    }
                    self.wd_cols.clear();
                    self.factorisations += 1;
                } else if refactor {
                    self.jblocks_study();
                    self.factor_j(nn_a);
                    self.jbase[..nn_a * nn_a].copy_from_slice(&self.jraw[..nn_a * nn_a]);
                    self.wd_cols.clear();
                    self.factorisations += 1;
                } else {
                    self.updates += 1;
                }
                self.lu_na = nn_a;
                self.lu_h = r.h;
                tick!(t, 7);
            }
            if coupled_ready {
                self.dv[..nn_a].copy_from_slice(&self.g[..nn_a]);
                self.solve_j(nn_a);
            } else {
                self.newton_solve(r, &v);
            }
            let (big, limited) = if self.coupled_limit {
                let (mut big, mut limited, serves) = self.newton_coupled_update(r, &mut v, fresh);
                if !serves && !fresh {
                    // A cached correction that makes little progress is discarded before
                    // it can walk into a saturated input's flat tail. Retry at the same
                    // controls with their current Jacobian, within the same iteration cap.
                    coupled_fresh = true;
                    self.eval_devices(r, &v);
                    self.newton_jacobian(r);
                    self.v_lu.copy_from_slice(&v);
                    self.factor_j(nn_a);
                    self.jbase[..nn_a * nn_a].copy_from_slice(&self.jraw[..nn_a * nn_a]);
                    self.wd_cols.clear();
                    self.factorisations += 1;
                    if self.blocks {
                        self.bdense = true;
                    }
                    fresh = true;
                    self.newton_solve(r, &v);
                    (big, limited, _) = self.newton_coupled_update(r, &mut v, true);
                }
                (big, limited)
            } else {
                self.newton_update(&mut v)
            };
            coupled_ready = self.coupled_limit;
            tick!(t, 3);
            // Converged when the error the last correction leaves is under TOL_V: of the order
            // of its square over a junction's 2 VT with a Jacobian made at this iteration's
            // start, and with an older one at most the correction times its relative error,
            // which the controls' moves since it was made over VT bound.
            // (The Jacobian's relative error: each control's move over twice its tolerance, a
            // junction's VT.)
            let age = if fresh {
                0.0
            } else {
                (0..nn_a)
                    .map(|c| 0.5 * self.jac_move(c, v[c]))
                    .fold(0.0, f64::max)
            } + if self.blocks && !self.bdense {
                self.bdrop
            } else {
                0.0
            };
            let left = (big * big / (2.0 * self.vt)).max(big * age);
            if !limited && (big < 1e-6 * self.tol_scale || left < self.tol_v * self.tol_scale) {
                // The switches' targets at the solution: held again, and solved again, until
                // they agree with it.
                let moved = self.update_held(&v);
                if moved <= 1e-7 {
                    converged = true;
                    break;
                }
                round += 1;
                // (A regenerative switch's rounds bracketing its target past a fold: twice
                // as many.)
                let rounds = if self.devs[..nd_a].iter().any(|d| d.bracket.is_some()) {
                    2 * MAX_ROUNDS
                } else {
                    MAX_ROUNDS
                };
                if round >= rounds {
                    // Near a fold (a switch about to turn) the rounds converge only linearly:
                    // a target still moving by a ten-thousandth of its swing is taken as found.
                    converged = moved < 1e-4;
                    if converged {
                        self.counters.rounds_capped += 1;
                    }
                    if !converged
                        && (self.study_live_debug
                            || LONG_STEPS.load(std::sync::atomic::Ordering::Relaxed))
                    {
                        eprintln!(
                            "step {} (h {:.3e}): rounds capped, the last move {moved:.3e} of a swing",
                            self.counters.steps, r.h
                        );
                    }
                    break;
                }
                it_round = it;
                coupled_ready = false;
                continue;
            }
            // An iteration on an older Jacobian that leaves work to do: a new one next.
            if !fresh && it - it_round >= self.chord {
                self.refresh_moved = true;
            }
            if (it > MAX_ITER - 8 && self.study_live_debug)
                || (LONG_STEPS.load(std::sync::atomic::Ordering::Relaxed)
                    && self
                        .trace
                        .is_some_and(|(a, b)| self.counters.steps >= a && self.counters.steps < b))
            {
                let mut ix: Vec<usize> = (0..nn_a).collect();
                ix.sort_by(|&a, &b| self.dv[b].abs().total_cmp(&self.dv[a].abs()));
                let top: Vec<String> = ix[..3.min(nn_a)]
                    .iter()
                    .map(|&i| {
                        let d = self
                            .devs
                            .iter()
                            .find(|d| i >= d.v_at && i < d.v_at + d.slot.nv)
                            .map(|d| format!("{} {} #{}", d.slot.refdes, d.slot.role, i - d.v_at))
                            .unwrap_or_default();
                        format!("{d}: v {:+.5} dv {:+.3e}", v[i], self.dv[i])
                    })
                    .collect();
                eprintln!(
                    "step {} it {it} h {:.3e} fresh {fresh} limited {limited} big {big:.3e}; {}",
                    self.counters.steps,
                    r.h,
                    top.join("; ")
                );
            }
            // A whole sample to be taken again in sub-steps if it fails (`set_retry`): given up
            // once the restart from the last solution has had as long as the prediction.
            if it - it_round >= MAX_ITER
                || (!sub && self.saved.is_some() && it - it_round >= RETRY_AFTER)
            {
                break;
            }
        }
        // A switch turned in the step: its target moved by a quarter of its swing or more.
        self.turned = self.devs[..nd_a]
            .iter()
            .any(|d| d.switch && (d.held - d.held_start).abs() > 0.25 * d.swing);
        if !self.moved_by.is_empty() && !sub {
            for (c, (x, p)) in v.iter().zip(&self.v_prev).enumerate() {
                if (x - p).abs() > SKIP {
                    self.moved_by[c] += 1;
                }
            }
        }
        if !self.it_hist.is_empty() {
            let k = it.min(self.it_hist.len() - 1);
            self.it_hist[k] += 1;
            if it >= 9 && LONG_STEPS.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("long step {}: {it} iterations", self.counters.steps);
            }
        }
        self.counters.steps += 1;
        self.counters.iterations += it as u64;
        if self.blocks && (it > BLOCK_SLOW || self.bdense) {
            self.bfind = true;
        }
        if self.bdense {
            // Back to blocks, found again.
            self.bdense = false;
            self.lu_ok = false;
            self.wd_cols.clear();
        }
        if !converged {
            self.counters.capped += 1;
        }
        // The charges at the step's end (their currents' integrals).
        for (q, x) in self.rq.iter_mut().zip(&self.r[self.ns..]) {
            *q += x * r.h;
        }
        self.end_inputs(r);
        tick!(t, 4);
        self.end_sums(nzk);
        wmadd(
            &mut self.z[..nzk],
            &r.g1,
            &self.bw,
            self.nr,
            self.nc,
            self.nzs,
        );
        wmadd(
            &mut self.z[..nzk],
            &r.ga,
            &self.bq,
            self.nr,
            self.nc,
            self.nzs,
        );
        tick!(t, 5);
        self.end_outputs(r, &v, nzk);
        if r.whole {
            if self.forced {
                self.fzf_last.copy_from_slice(&self.fz[self.nzs..]);
            } else {
                self.fzf_last.fill(0.0);
            }
        }
        self.last_sub = sub;
        for (((d1, d2), x), p) in self
            .dv_last
            .iter_mut()
            .zip(&mut self.dv_last2)
            .zip(&v)
            .zip(&self.v_prev)
        {
            *d2 = *d1;
            *d1 = x - p;
        }
        self.h_last2 = self.h_last;
        self.h_last = r.h;
        if self.predict_r && !sub {
            for (k, &x) in self.r.iter().enumerate() {
                for o in 0..3 {
                    let d = self.pred_all[3 * k + o] - x;
                    let e = &mut self.pred_err[3 * k + o];
                    *e += 0.1 * (d * d - *e);
                }
            }
        }
        std::mem::swap(&mut self.r_hist2, &mut self.r_hist1);
        self.r_hist1.copy_from_slice(&self.r_prev_step);
        self.r_prev_step.copy_from_slice(&self.r);
        if self.record.is_some() {
            self.record_step(r.h, &v);
        }
        self.v = v;
        if self.forced {
            // The played-back direct part at the step's end, without the fast modes' share
            // (for a sample taken in sub-steps next).
            if r.whole && self.nf > 0 {
                let (nz, nzs) = (self.nz, self.nzs);
                for (o, y) in self.fy_last.iter_mut().enumerate() {
                    *y = self.fy[o] - dot(&self.c[o * nz + nzs..(o + 1) * nz], &self.fz[nzs..]);
                }
            } else {
                self.fy_last.copy_from_slice(&self.fy);
            }
        } else if self.has_switch {
            self.fy_last.fill(0.0);
        }
        self.forced = false;
        tick!(t, 6, end);
    }

    /// The step just made (of size `h`, its controls `v`) recorded (see `record`).
    fn record_step(&mut self, h: f64, v: &[f64]) {
        let (nw, nsrc, ns) = (self.sys_nw, self.nsrc, self.ns);
        let mut w = vec![0.0; nw + self.sys_nv];
        for (i, &(k, _)) in self.srcs.iter().enumerate() {
            w[k] = self.w[i];
        }
        for d in &self.devs {
            let s = &d.slot;
            for o in 0..s.ns {
                w[s.s_off + o] = self.r[d.s_at + o];
            }
            for q in 0..s.nq {
                w[s.q_off + q] = self.r[ns + d.q_at + q];
            }
            for c in 0..s.nv {
                w[nw + s.v_off + c] = v[d.v_at + c];
            }
        }
        for (l, p) in self.pots.iter().enumerate() {
            w[p.slot.s_off] = self.w[nsrc + ns + l];
        }
        if let Some(rec) = self.record.as_mut() {
            rec.push((h, w));
        }
    }

    /// The controls and the pots without this step's remainders (`a`), the pots' part of the
    /// nonlinear controls folded in: `a_n + K_np ΔG M a_p`.
    #[inline(always)]
    fn free_outputs(&mut self, r: &Rate) {
        let (np, nsrc, nn_r) = (self.np, self.nsrc, self.nn_r);
        let (level, nn_a, _) = self.active();
        let lanes = if level == 2 { self.nop } else { self.nop_r };
        let nzk = if r.whole { self.nzs } else { self.nz };
        let nh = nsrc + self.ns + np;
        if r.whole && self.nf > 0 {
            // The fast modes' part, from the held inputs at the step's start, in the same sums.
            sum_rows2(
                &mut self.at,
                (&self.ct[..nzk * self.nop], &self.z[..nzk]),
                (&r.k0f, &self.w_prev[..nh]),
                self.nop,
                lanes,
            );
        } else {
            sum_rows(
                &mut self.at,
                &self.ct[..nzk * self.nop],
                &self.z[..nzk],
                self.nop,
                lanes,
            );
        }
        // (The pots' lanes follow the ring's controls: in a strike played whole, past the
        // frozen ones' too.)
        let top = if level == 2 { nn_a + np } else { nn_r + np };
        for o in 0..top {
            let mut s = self.y0[o] + self.at[o];
            if nsrc > 0 {
                s += dot_short(&r.k_src[o * nsrc..(o + 1) * nsrc], &self.w[..nsrc]);
            }
            if self.forced {
                s += self.fy[o];
            }
            self.a[o] = s;
        }
        if np > 0 {
            self.vp.copy_from_slice(&self.a[nn_r..nn_r + np]);
            for c in 0..nn_a {
                let o = self.lane(c);
                self.a[o] += dot_short(&r.knp_m[c * np..(c + 1) * np], &self.vp);
            }
        }
    }

    /// The devices' remainders (`r`) and their Jacobians' own blocks (`jr`) at `v`.
    #[inline(always)]
    fn eval_devices(&mut self, r: &Rate, v: &[f64]) {
        let (nn, ns_all) = (self.nn, self.ns);
        let skip = self.skip;
        let (_, _, nd_a) = self.active();
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        for d in self.devs[..nd_a].iter_mut() {
            let (nv, ns, nq) = (d.slot.nv, d.slot.ns, d.slot.nq);
            let x = &v[d.v_at..d.v_at + nv];
            let mut dv = [0.0; 5];
            let mut moved = !d.evaluated;
            for c in 0..nv {
                dv[c] = x[c] - d.v_ev[c];
                moved |= dv[c].abs() > skip;
            }
            if moved {
                d.slot.dev.eval(x, &mut out, &mut jac);
                if d.switch {
                    out[0] = d.held;
                    jac[..nv].fill(0.0);
                }
                let mut dx = [0.0; 5];
                for c in 0..nv {
                    dx[c] = x[c] - d.v0[c];
                }
                // The statics less their linearisation; the charges less their own
                // capacitance's.
                for o in 0..ns {
                    let mut y = out[o] - d.f0[o];
                    for c in 0..nv {
                        let j0 = d.j0[o * nv + c];
                        y -= j0 * dx[c];
                        d.jac_ev[o * nv + c] = jac[o * nv + c] - j0;
                    }
                    d.out_ev[o] = y;
                }
                for q in 0..nq {
                    let o = ns + q;
                    let (cc, c0) = d.cd[q];
                    d.out_ev[o] = out[o] - d.f0[o] - c0 * dx[cc];
                    d.jac_ev[o * nv..(o + 1) * nv].copy_from_slice(&jac[o * nv..(o + 1) * nv]);
                    d.jac_ev[o * nv + cc] -= c0;
                }
                d.v_ev[..nv].copy_from_slice(x);
                d.evaluated = true;
                dv = [0.0; 5];
            }
            // The remainders from the evaluation, to first order in the controls' change since;
            // the charges' as currents over the step.
            for o in 0..ns {
                let k = d.s_at + o;
                let jrow = &mut self.jr[k * nn + d.v_at..k * nn + d.v_at + nv];
                let mut y = d.out_ev[o];
                for c in 0..nv {
                    let j = d.jac_ev[o * nv + c];
                    y += j * dv[c];
                    jrow[c] = j;
                }
                self.r[k] = y;
            }
            for q in 0..nq {
                let (o, k) = (ns + q, ns_all + d.q_at + q);
                let jrow = &mut self.jr[k * nn + d.v_at..k * nn + d.v_at + nv];
                let mut y = d.out_ev[o];
                for c in 0..nv {
                    let j = d.jac_ev[o * nv + c];
                    y += j * dv[c];
                    jrow[c] = j * r.inv_h;
                }
                self.r[k] = (y - self.rq[d.q_at + q]) * r.inv_h;
            }
        }
    }

    /// A control's move to `x` from where the factored Jacobian was made, re its tolerance
    /// (over 1: its device's columns are made again). An op-amp's output clamp's by its slope's
    /// own move (the logistic of the control over the knee, re a fortieth of the clamp's
    /// conductance): nil far from a rail, however far the output swings.
    fn jac_move(&self, c: usize, x: f64) -> f64 {
        let (l, tol, k) = (self.v_lu[c], self.j_tol[c], self.knee[c]);
        let d = if k > 0.0 {
            (sim::softplus_d(x, k) - sim::softplus_d(l, k)).abs() * 4.0 * k
        } else {
            (x - l).abs()
        };
        if tol == f64::INFINITY || d == 0.0 {
            0.0
        } else {
            d / tol
        }
    }

    /// Brings the switching devices' held targets up to date with the controls `v`: the
    /// largest move, re its device's swing.
    fn update_held(&mut self, v: &[f64]) -> f64 {
        let (_, _, nd_a) = self.active();
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        let mut moved: f64 = 0.0;
        for d in self.devs[..nd_a].iter_mut().filter(|d| d.switch) {
            d.slot
                .dev
                .eval(&v[d.v_at..d.v_at + d.slot.nv], &mut out, &mut jac);
            let delta = out[0] - d.held;
            let m = delta.abs() / d.swing;
            moved = moved.max(m);
            if m > 1e-7 {
                // Moves that keep their way and grow: the rounds are leaving an unstable point
                // (a regenerative switch turning slowly near its fold); the switch goes to the
                // rail it is heading for, once a step, and the rounds confirm it. (Moves of
                // the solve's own noise, a millionth of a swing, are no sign of it: before its
                // fold a Schmitt oscillator was sent over at 40 ns a step, its half-cycles 7 us
                // short.)
                let target = if delta * d.held_move > 0.0
                    && delta.abs() > d.held_move.abs()
                    && m > JUMP
                    && !d.jumped
                    && d.bracket.is_none()
                {
                    d.jumped = true;
                    if delta > 0.0 { d.swing } else { 0.0 }
                } else if d.regen.is_some() {
                    regen_round(d, delta)
                } else {
                    out[0]
                };
                if d.regen.is_some() {
                    d.last = Some((d.held, delta));
                }
                d.held_move = delta;
                out[0] = target;
                if self.study_live_debug
                    || (LONG_STEPS.load(std::sync::atomic::Ordering::Relaxed)
                        && self.trace.is_some_and(|(a, b)| {
                            self.counters.steps >= a && self.counters.steps < b
                        }))
                {
                    eprintln!(
                        "step {} round: {} {} held {:.6e} -> {:.6e} (move {delta:+.3e}, swing {:.3e}), v {:?}",
                        self.counters.steps,
                        d.slot.refdes,
                        d.slot.role,
                        d.held,
                        out[0],
                        d.swing,
                        &v[d.v_at..d.v_at + d.slot.nv]
                    );
                }
                d.held = out[0];
                d.evaluated = false;
            }
        }
        moved
    }

    /// Newton's Jacobian `J = I - K jr` (jr being each device's own block), into `jg`.
    #[inline(always)]
    fn newton_jacobian(&mut self, r: &Rate) {
        self.newton_jacobian_made(r);
    }

    /// (`PLUGIN_KIT_CIRCUIT_JBLOCKS=step`, a study: at that step's factorisation, each pair of devices'
    /// largest entry of the Jacobian between their controls.)
    fn jblocks_study(&self) {
        if self.study_jblocks == Some(self.counters.steps) {
            let (_, na, nd_a) = self.active();
            let names: Vec<String> = self.devs[..nd_a]
                .iter()
                .map(|d| format!("{}{}", d.slot.refdes, d.slot.role).replace(' ', ""))
                .collect();
            eprintln!(
                "the Jacobian's largest entries between devices, x1000 (rows: whose control moves):"
            );
            eprintln!(
                "{:>10} {}",
                "",
                names
                    .iter()
                    .map(|n| format!("{n:>8.8}"))
                    .collect::<String>()
            );
            for (a, da) in self.devs[..nd_a].iter().enumerate() {
                let mut line = format!("{:>10.10}", names[a]);
                for db in &self.devs[..nd_a] {
                    let mut m: f64 = 0.0;
                    for i in da.v_at..da.v_at + da.slot.nv {
                        for j in db.v_at..db.v_at + db.slot.nv {
                            if i < na && j < na && i != j {
                                m = m.max(self.jraw[i * na + j].abs());
                            }
                        }
                    }
                    line.push_str(&format!("{:>8.1}", m * 1e3));
                }
                eprintln!("{line}");
            }
        }
    }

    fn newton_jacobian_made(&mut self, r: &Rate) {
        let (nn, ns) = (self.nn, self.ns);
        let (_, na, nd_a) = self.active();
        self.jraw[..na * na].fill(0.0);
        for i in 0..na {
            self.jraw[i * na + i] = 1.0;
        }
        for d in &self.devs[..nd_a] {
            let s = &d.slot;
            for (k0, kn) in [(d.s_at, s.ns), (ns + d.q_at, s.nq)] {
                for k in k0..k0 + kn {
                    let col = &r.kn_t[k * nn..k * nn + na];
                    for c in d.v_at..d.v_at + s.nv {
                        let jv = self.jr[k * nn + c];
                        if jv == 0.0 {
                            continue;
                        }
                        for (i, x) in col.iter().enumerate() {
                            self.jraw[i * na + c] -= x * jv;
                        }
                    }
                }
            }
        }
    }

    /// Factor the full Jacobian, or impose the fixed rails' dependent corrections first.
    fn factor_j(&mut self, na: usize) {
        if na == self.nn && !self.dependent.is_empty() {
            let n = self.independent.len();
            for (i, &row) in self.independent.iter().enumerate() {
                for (j, &col) in self.independent.iter().enumerate() {
                    let mut x = self.jraw[row * na + col];
                    for &(drop, keep) in &self.dependent {
                        if col == keep {
                            x -= self.jraw[row * na + drop];
                        }
                    }
                    self.jg[i * n + j] = x;
                }
            }
            self.lu_ok = lu_factor(&mut self.jg[..n * n], n, &mut self.piv[..n]);
        } else {
            self.jg[..na * na].copy_from_slice(&self.jraw[..na * na]);
            self.lu_ok = lu_factor(&mut self.jg[..na * na], na, &mut self.piv[..na]);
        }
    }

    /// `dv = J⁻¹ dv` (its first `na`): the factored Jacobian, corrected for the columns that
    /// changed since it was factored (`woodbury`).
    #[inline(always)]
    fn solve_j(&mut self, na: usize) {
        if na == self.nn && !self.dependent.is_empty() {
            let n = self.independent.len();
            for (j, &(drop, keep)) in self.dependent.iter().enumerate() {
                self.reduced_rhs[n + j] = self.dv[drop] + self.dv[keep];
            }
            for (i, &row) in self.independent.iter().enumerate() {
                let mut x = self.dv[row];
                for (j, &(drop, _)) in self.dependent.iter().enumerate() {
                    x -= self.jraw[row * na + drop] * self.reduced_rhs[n + j];
                }
                self.reduced_rhs[i] = x;
            }
            lu_solve(
                &self.jg[..n * n],
                n,
                &self.piv[..n],
                &mut self.reduced_rhs[..n],
            );
            for (i, &row) in self.independent.iter().enumerate() {
                self.dv[row] = self.reduced_rhs[i];
            }
            for (j, &(drop, keep)) in self.dependent.iter().enumerate() {
                self.dv[drop] = self.reduced_rhs[n + j] - self.dv[keep];
            }
            return;
        }
        if self.blocks && !self.bdense {
            for b in 0..self.bstart.len() - 1 {
                let (b0, b1) = (self.bstart[b], self.bstart[b + 1]);
                let n = b1 - b0;
                for (j, &c) in self.bperm[b0..b1].iter().enumerate() {
                    self.btmp[j] = self.dv[c];
                }
                let o = self.boff[b];
                lu_solve(
                    &self.bjg[o..o + n * n],
                    n,
                    &self.bpiv[b0..b1],
                    &mut self.btmp[..n],
                );
                for (j, &c) in self.bperm[b0..b1].iter().enumerate() {
                    self.dv[c] = self.btmp[j];
                }
            }
            return;
        }
        lu_solve(&self.jg[..na * na], na, &self.piv[..na], &mut self.dv[..na]);
        let k = self.wd_cols.len();
        if k > 0 {
            for (i, &c) in self.wd_cols.iter().enumerate() {
                self.wd_t[i] = self.dv[c];
            }
            lu_solve(
                &self.wd_s[..k * k],
                k,
                &self.wd_piv[..k],
                &mut self.wd_t[..k],
            );
            for j in 0..k {
                let t = self.wd_t[j];
                for (x, w) in self.dv[..na]
                    .iter_mut()
                    .zip(&self.wd_w[j * na..(j + 1) * na])
                {
                    *x -= w * t;
                }
            }
        }
    }

    /// The Jacobian (`jraw`, its first `na` controls) in blocks: its devices joined where an
    /// entry between their controls exceeds BLOCK_TOL, each block factored alone, the largest
    /// entry left out kept (`bdrop`: Newton's convergence counts it as the Jacobian's error).
    fn factor_blocks(&mut self, na: usize) {
        let (_, _, nd_a) = self.active();
        // (In buffers kept from call to call: no allocation.)
        let mut devs = std::mem::take(&mut self.blk_devs);
        let mut root = std::mem::take(&mut self.blk_root);
        let mut pairs = std::mem::take(&mut self.blk_pairs);
        let mut gid = std::mem::take(&mut self.blk_gid);
        let mut cov = std::mem::take(&mut self.blk_cov);
        devs.clear();
        devs.extend(
            self.devs[..nd_a]
                .iter()
                .map(|d| (d.v_at, (d.v_at + d.slot.nv).min(na)))
                .filter(|(a, b)| a < b),
        );
        let nd = devs.len();
        root.clear();
        root.extend(0..nd);
        fn find(root: &mut [usize], mut i: usize) -> usize {
            while root[i] != i {
                root[i] = root[root[i]];
                i = root[i];
            }
            i
        }
        let coupling = |a: (usize, usize), b: (usize, usize)| -> f64 {
            let mut m: f64 = 0.0;
            for i in a.0..a.1 {
                for j in b.0..b.1 {
                    m = m
                        .max(self.jraw[i * na + j].abs())
                        .max(self.jraw[j * na + i].abs());
                }
            }
            m
        };
        let mut drop: f64 = 0.0;
        pairs.clear();
        for x in 0..nd {
            for y in x + 1..nd {
                pairs.push((x, y, coupling(devs[x], devs[y])));
            }
        }
        for &(x, y, m) in &pairs {
            if m > BLOCK_TOL {
                let (rx, ry) = (find(&mut root, x), find(&mut root, y));
                root[rx] = ry;
            }
        }
        for &(x, y, m) in &pairs {
            if find(&mut root, x) != find(&mut root, y) {
                drop = drop.max(m);
            }
        }
        self.bperm.clear();
        self.bstart.clear();
        self.boff.clear();
        // The groups in the order of their first devices, each's devices in order.
        gid.clear();
        gid.resize(nd, usize::MAX);
        let mut ng = 0;
        for x in 0..nd {
            let r0 = find(&mut root, x);
            if gid[r0] == usize::MAX {
                gid[r0] = ng;
                ng += 1;
            }
        }
        let mut off = 0;
        for g in 0..ng {
            self.bstart.push(self.bperm.len());
            self.boff.push(off);
            let n0 = self.bperm.len();
            for x in 0..nd {
                if gid[find(&mut root, x)] == g {
                    self.bperm.extend(devs[x].0..devs[x].1);
                }
            }
            let n = self.bperm.len() - n0;
            off += n * n;
        }
        // (Controls of no device: alone.)
        cov.clear();
        cov.resize(na, false);
        for &c in &self.bperm {
            cov[c] = true;
        }
        for (c, &covered) in cov.iter().enumerate() {
            if !covered {
                self.bstart.push(self.bperm.len());
                self.boff.push(off);
                self.bperm.push(c);
                off += 1;
            }
        }
        self.bstart.push(self.bperm.len());
        for b in 0..self.bstart.len() - 1 {
            for i in self.bstart[b]..self.bstart[b + 1] {
                self.bof[self.bperm[i]] = b;
            }
        }
        self.bjg.resize(off, 0.0);
        self.bpiv.resize(self.bperm.len(), 0);
        let mut ok = true;
        for b in 0..self.bstart.len() - 1 {
            let (b0, b1) = (self.bstart[b], self.bstart[b + 1]);
            let n = b1 - b0;
            let o = self.boff[b];
            for (i, &ci) in self.bperm[b0..b1].iter().enumerate() {
                for (j, &cj) in self.bperm[b0..b1].iter().enumerate() {
                    self.bjg[o + i * n + j] = self.jraw[ci * na + cj];
                }
            }
            ok &= lu_factor(&mut self.bjg[o..o + n * n], n, &mut self.bpiv[b0..b1]);
        }
        self.bdrop = drop;
        self.lu_ok = ok;
        self.blk_devs = devs;
        self.blk_root = root;
        self.blk_pairs = pairs;
        self.blk_gid = gid;
        self.blk_cov = cov;
        if self.study_blocks_debug && self.counters.steps.is_multiple_of(5000) {
            eprintln!(
                "step {}: {} blocks of {:?} controls, the largest left out {drop:.3}",
                self.counters.steps,
                self.bstart.len() - 1,
                self.bstart
                    .windows(2)
                    .map(|w| w[1] - w[0])
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The blocks holding a stale device's controls factored again, the blocks as they were.
    fn refactor_stale_blocks(&mut self, na: usize) {
        let (_, _, nd_a) = self.active();
        let mut ok = self.lu_ok;
        let nb = self.bstart.len() - 1;
        // (At most 64 blocks marked: more, every one.)
        let mut mark: u64 = 0;
        for d in self.devs[..nd_a].iter().filter(|d| d.stale) {
            for c in d.v_at..(d.v_at + d.slot.nv).min(na) {
                mark |= 1u64 << self.bof[c].min(63);
            }
        }
        for b in 0..nb {
            if nb <= 64 && mark & (1u64 << b) == 0 {
                continue;
            }
            let (b0, b1) = (self.bstart[b], self.bstart[b + 1]);
            let n = b1 - b0;
            let o = self.boff[b];
            for i in 0..n {
                let ci = self.bperm[b0 + i];
                for j in 0..n {
                    self.bjg[o + i * n + j] = self.jraw[ci * na + self.bperm[b0 + j]];
                }
            }
            ok &= lu_factor(&mut self.bjg[o..o + n * n], n, &mut self.bpiv[b0..b1]);
        }
        self.lu_ok = ok;
    }

    /// The correction for the changed columns (`wd_cols`): `W = J₀⁻¹ U` (U the columns' changes
    /// from the factored `J₀`), `S = I + Wᵀ`'s rows at them, factored.
    fn woodbury(&mut self, na: usize) {
        let k = self.wd_cols.len();
        for (j, &c) in self.wd_cols.iter().enumerate() {
            let w = &mut self.wd_w[j * na..(j + 1) * na];
            for i in 0..na {
                w[i] = self.jraw[i * na + c] - self.jbase[i * na + c];
            }
            lu_solve(&self.jg[..na * na], na, &self.piv[..na], w);
        }
        for (i, &c) in self.wd_cols.iter().enumerate() {
            for j in 0..k {
                self.wd_s[i * k + j] = f64::from(i == j) + self.wd_w[j * na + c];
            }
        }
        if !lu_factor(&mut self.wd_s[..k * k], k, &mut self.wd_piv[..k]) {
            // A singular correction: the Jacobian factored afresh next.
            self.lu_ok = false;
            self.wd_cols.clear();
        }
    }

    /// The unfactored Jacobian's columns of the devices marked stale, made again.
    fn jacobian_columns(&mut self, r: &Rate) {
        let (nn, ns) = (self.nn, self.ns);
        let (_, na, nd_a) = self.active();
        for d in self.devs[..nd_a].iter().filter(|d| d.stale) {
            let s = &d.slot;
            for c in d.v_at..d.v_at + s.nv {
                for i in 0..na {
                    self.jraw[i * na + c] = if i == c { 1.0 } else { 0.0 };
                }
            }
            for (k0, kn) in [(d.s_at, s.ns), (ns + d.q_at, s.nq)] {
                for k in k0..k0 + kn {
                    let col = &r.kn_t[k * nn..k * nn + na];
                    for c in d.v_at..d.v_at + s.nv {
                        let jv = self.jr[k * nn + c];
                        if jv == 0.0 {
                            continue;
                        }
                        for (i, x) in col.iter().enumerate() {
                            self.jraw[i * na + c] -= x * jv;
                        }
                    }
                }
            }
        }
    }

    /// Newton's step (`dv`) at `v`: `g = v - a - K r`, `J dv = g` with `J` as factored.
    #[inline(always)]
    fn newton_solve(&mut self, r: &Rate, v: &[f64]) {
        self.newton_residual(r, v);
        let (_, na, _) = self.active();
        self.dv[..na].copy_from_slice(&self.g[..na]);
        self.solve_j(na);
    }

    #[inline(always)]
    fn newton_residual(&mut self, r: &Rate, v: &[f64]) -> f64 {
        let nn = self.nn;
        let (_, na, _) = self.active();
        for c in 0..na {
            self.g[c] = v[c] - self.a[self.lane(c)];
        }
        for (c, &x) in self.r.iter().enumerate() {
            if x != 0.0 {
                axpy(&mut self.g[..na], -x, &r.kn_t[c * nn..c * nn + na]);
            }
        }
        self.g[..na].iter().map(|x| x * x).sum()
    }

    /// A common correction preserves the controls' network relations. Backtracking on the
    /// actual residual avoids cycling across a narrow clamp knee with a saturated input.
    fn newton_coupled_update(&mut self, r: &Rate, v: &mut [f64], fresh: bool) -> (f64, bool, bool) {
        let (_, na, nd_a) = self.active();
        let norm: f64 = self.g[..na].iter().map(|x| x * x).sum();
        // The existing correction tolerance also ends a line search at round-off: the
        // residual need not decrease once no control can change by a representable amount.
        let correction = self.dv[..na].iter().map(|x| x.abs()).fold(0.0, f64::max);
        let mut fraction = 1.0f64;
        for d in &self.devs[..nd_a] {
            if matches!(d.slot.dev, Dev::Mos(_)) {
                // A MOS transistor's four controls limited together (`dev::MosP::limit`).
                let mut vn = [0.0; 4];
                for c in 0..4 {
                    vn[c] = v[d.v_at + c] - self.dv[d.v_at + c];
                }
                let free = vn;
                let at = d.v_at..d.v_at + 4;
                limit_controls(&d.slot.dev, &d.lims, &mut vn, &v[at], self.vt);
                for c in 0..4 {
                    let (i, step) = (d.v_at + c, -self.dv[d.v_at + c]);
                    if vn[c] != free[c] && step != 0.0 {
                        fraction = fraction.min(((vn[c] - v[i]) / step).clamp(0.0, 1.0));
                    }
                }
                continue;
            }
            for c in 0..d.slot.nv {
                let i = d.v_at + c;
                let step = -self.dv[i];
                let (vn, l) = limit(d.lims[c], v[i] + step, v[i], self.vt);
                if l && step != 0.0 {
                    fraction = fraction.min(((vn - v[i]) / step).clamp(0.0, 1.0));
                }
            }
        }
        let mut trial = std::mem::take(&mut self.hint_v);
        trial.copy_from_slice(v);
        for backtrack in 0..16 {
            for i in 0..na {
                trial[i] = v[i] - fraction * self.dv[i];
            }
            let slew = self.coupled_slew
                && backtrack == 0
                && self.devs[..nd_a].iter().any(|d| {
                    if let Dev::OpIn(input) = d.slot.dev {
                        let width = input.imax / input.gm;
                        v[d.v_at].abs() > 4.0 * width && trial[d.v_at].abs() < 0.01 * width
                    } else {
                        false
                    }
                });
            self.eval_devices(r, &trial);
            let next = self.newton_residual(r, &trial);
            if self.coupled_reuse
                && !fresh
                && !slew
                && correction >= 1e-6 * self.tol_scale
                && (backtrack >= 2 || next > 0.5 * norm)
            {
                self.hint_v = trial;
                return (0.0, true, false);
            }
            if slew
                || next <= norm * (1.0 - 1e-4 * fraction)
                || correction < 1e-6 * self.tol_scale
                || backtrack == 15
            {
                self.refresh_moved |= slew;
                break;
            }
            fraction *= 0.5;
        }
        let big = v[..na]
            .iter()
            .zip(&trial[..na])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        v.copy_from_slice(&trial);
        self.hint_v = trial;
        (big, fraction < 1.0, true)
    }

    /// Newton's step taken, each control limited; the remainders follow it to first order (no
    /// new evaluation). The largest change, and whether any was limited.
    #[inline(always)]
    fn newton_update(&mut self, v: &mut [f64]) -> (f64, bool) {
        let (nn, ns) = (self.nn, self.ns);
        let (_, _, nd_a) = self.active();
        let mut big = 0.0f64;
        let mut limited = false;
        for d in &self.devs[..nd_a] {
            let s = &d.slot;
            let mut vn = [0.0; 5];
            for c in 0..s.nv {
                let i = d.v_at + c;
                vn[c] = v[i] + damped(-self.dv[i], &mut self.step_prev[i]);
            }
            let at = d.v_at..d.v_at + s.nv;
            limited |= limit_controls(&s.dev, &d.lims, &mut vn[..s.nv], &v[at], self.vt);
            for c in 0..s.nv {
                let i = d.v_at + c;
                let vn = vn[c];
                let step = vn - v[i];
                big = big.max(step.abs());
                if step != 0.0 {
                    for (k0, kn) in [(d.s_at, s.ns), (ns + d.q_at, s.nq)] {
                        for k in k0..k0 + kn {
                            self.r[k] += self.jr[k * nn + i] * step;
                        }
                    }
                }
                v[i] = vn;
            }
        }
        (big, limited)
    }

    /// The kept inputs at the step's end: the pots' remainders, `w_p = ΔG M (a_p + K_p(s,q) r)`,
    /// and the devices'.
    #[inline(always)]
    fn end_inputs(&mut self, r: &Rate) {
        let (nn_r, np, nsrc, ns) = (self.nn_r, self.np, self.nsrc, self.ns);
        let nsq = ns + self.nq;
        for l in 0..np {
            self.vp[l] = self.a[nn_r + l] + dot_short(&r.kp_r[l * nsq..(l + 1) * nsq], &self.r);
        }
        let p_in = nsrc + ns;
        for l in 0..np {
            self.w[p_in + l] = dot_short(&r.dg_m[l * np..(l + 1) * np], &self.vp);
        }
        self.w[nsrc..nsrc + ns].copy_from_slice(&self.r[..ns]);
        self.w[nsrc + ns + np..].copy_from_slice(&self.r[ns..]);
    }

    /// `B w` at the step's end for the first-order-held inputs (the next step's start too), and
    /// for the charges.
    #[inline(always)]
    fn end_sums(&mut self, nzk: usize) {
        let nz = self.nz;
        let (nsrc, ns, np) = (self.nsrc, self.ns, self.np);
        let ch = nsrc + ns + np;
        let (_, _, ns_l, nq_l) = self.prefix(self.level());
        if ns_l == ns && nq_l == self.nq {
            let (bh, bq) = self.b.split_at(ch * nz);
            let (wh, wq) = self.w.split_at(ch);
            sum_rows(&mut self.bw, bh, wh, nz, nzk);
            sum_rows(&mut self.bq, bq, wq, nz, nzk);
        } else {
            // The devices out of the step (at rest, or played back): their columns left out.
            let held = [(0, nsrc + ns_l), (nsrc + ns, ch)];
            let charges = [(ch, ch + nq_l)];
            sum_rows_in(&mut self.bw, &self.b, &self.w, nz, nzk, &held);
            sum_rows_in(&mut self.bq, &self.b, &self.w, nz, nzk, &charges);
        }
    }

    /// The kept outputs at the step's end: the controls are the solution `v`; the probes from
    /// the state.
    #[inline(always)]
    fn end_outputs(&mut self, r: &Rate, v: &[f64], nzk: usize) {
        let (nz, nn, np) = (self.nz, self.nn, self.np);
        for (c, x) in v.iter().enumerate() {
            let o = self.lane(c);
            self.y[o] = *x;
        }
        let ni = self.w.len();
        let nh = self.nsrc + self.ns + np;
        for p in 0..self.nprobe {
            let o = nn + np + p;
            let mut s = self.y0[o] + dot(&self.c[o * nz..o * nz + nzk], &self.z[..nzk]);
            if r.whole && self.nf > 0 {
                // The fast modes: their part of the direct response, and from the held inputs
                // at the step's start.
                s += dot_short(&r.dw[p * ni..(p + 1) * ni], &self.w)
                    + dot_short(&r.kp0f[p * nh..(p + 1) * nh], &self.w_prev[..nh]);
            } else {
                s += dot_short(&self.d[p * ni..(p + 1) * ni], &self.w);
            }
            if self.forced {
                s += self.fy[o];
            }
            self.y[o] = s;
        }
    }

    /// Each part of a step timed alone at the present state, `reps` times over (ns each): the
    /// free state, the free outputs, the devices' evaluations, Newton's solve, Newton's update,
    /// the step's end inputs, its sums, its outputs. The state is left as it was.
    #[cfg(feature = "prof")]
    pub fn bench_parts(&mut self, reps: usize) -> [f64; 8] {
        use std::hint::black_box;
        use std::time::Instant;
        let rates = std::mem::take(&mut self.rates);
        let r = &rates[0];
        let mut ns = [0.0; 8];
        let per = |t: Instant| t.elapsed().as_secs_f64() * 1e9 / reps as f64;
        let mut zs = self.z.clone();
        let t = Instant::now();
        for _ in 0..reps {
            zs.copy_from_slice(&self.z);
            free_state(
                &mut zs[..self.nzs],
                r,
                &self.bw,
                None,
                self.nr,
                self.nc,
                self.nzs,
            );
            black_box(&zs);
        }
        ns[0] = per(t);
        let t = Instant::now();
        for _ in 0..reps {
            self.free_outputs(r);
            black_box(&self.a);
        }
        ns[1] = per(t);
        let v = self.v.clone();
        let t = Instant::now();
        for _ in 0..reps {
            self.eval_devices(r, black_box(&v));
            black_box(&self.r);
        }
        ns[2] = per(t);
        {
            let mut out = [0.0; 8];
            let mut jac = [0.0; 40];
            let t = Instant::now();
            for _ in 0..reps {
                for d in &self.devs {
                    let s = &d.slot;
                    s.dev
                        .eval(black_box(&v[d.v_at..d.v_at + s.nv]), &mut out, &mut jac);
                    black_box(&out);
                }
            }
            eprintln!(
                "    (of the devices, their bare evaluations: {:.1} ns)",
                per(t)
            );
        }
        let t = Instant::now();
        for _ in 0..reps {
            self.newton_solve(r, black_box(&v));
            black_box(&self.dv);
        }
        ns[3] = per(t);
        let mut jc = self.jg.clone();
        let mut pc = self.piv.clone();
        let t = Instant::now();
        for _ in 0..reps {
            self.newton_jacobian(r);
            jc.copy_from_slice(&self.jraw);
            black_box(lu_factor(&mut jc, self.nn, &mut pc));
        }
        eprintln!("    (a new Jacobian, made and factored: {:.1} ns)", per(t));
        let (r0, mut vs) = (self.r.clone(), v.clone());
        let t = Instant::now();
        for _ in 0..reps {
            vs.copy_from_slice(&v);
            black_box(self.newton_update(&mut vs));
        }
        ns[4] = per(t);
        self.r.copy_from_slice(&r0);
        let w0 = self.w.clone();
        let t = Instant::now();
        for _ in 0..reps {
            self.end_inputs(r);
            black_box(&self.w);
        }
        ns[5] = per(t);
        self.w.copy_from_slice(&w0);
        let bw0 = self.bw.clone();
        let t = Instant::now();
        for _ in 0..reps {
            self.end_sums(self.nzs);
            black_box(&self.bw);
        }
        ns[6] = per(t);
        self.bw.copy_from_slice(&bw0);
        let y0 = self.y.clone();
        let t = Instant::now();
        for _ in 0..reps {
            self.end_outputs(r, black_box(&v), self.nzs);
            black_box(&self.y);
        }
        ns[7] = per(t);
        self.y.copy_from_slice(&y0);
        self.rates = rates;
        ns
    }
}

/// The free part of the state: `z ← e ⊙ z + g0 ⊙ bw`, and a played-back increment; `z` the
/// slow block or the whole state (the fast block from `nzs`, real).
#[inline(always)]
/// Each mode's share of a sample's increment from an input constant over it, over one of `n`
/// sub-steps: `(e^{μh/n} - 1) / (e^{μh} - 1)` (`e1`, `e0` the packed `e^{μh/n}`, `e^{μh}`; the
/// pairs complex), `1/n` for a mode at rest.
fn split_shares(e0: &[f64], e1: &[f64], nr: usize, nc: usize, nzs: usize, n: usize) -> Vec<f64> {
    let mut out = vec![0.0; e0.len()];
    let real = |a: f64, b: f64| {
        if (a - 1.0).abs() < 1e-12 {
            1.0 / n as f64
        } else {
            (b - 1.0) / (a - 1.0)
        }
    };
    for k in (0..nr).chain(nzs..e0.len()) {
        out[k] = real(e0[k], e1[k]);
    }
    for k in 0..nc {
        let (a, b) = (nr + k, nr + nc + k);
        let (pr, pi) = (e1[a] - 1.0, e1[b]);
        let (qr, qi) = (e0[a] - 1.0, e0[b]);
        let d = qr * qr + qi * qi;
        if d < 1e-24 {
            out[a] = 1.0 / n as f64;
        } else {
            out[a] = (pr * qr + pi * qi) / d;
            out[b] = (pi * qr - pr * qi) / d;
        }
    }
    out
}

fn free_state(
    z: &mut [f64],
    r: &Rate,
    bw: &[f64],
    fz: Option<&[f64]>,
    nr: usize,
    nc: usize,
    nzs: usize,
) {
    let real = |z: &mut [f64], lo: usize| {
        for (((z, e), g), b) in z.iter_mut().zip(&r.e[lo..]).zip(&r.g0[lo..]).zip(&bw[lo..]) {
            *z = e * *z + g * b;
        }
    };
    real(&mut z[..nr], 0);
    for k in 0..nc {
        let (a, b) = (nr + k, nr + nc + k);
        let (zr, zi) = (z[a], z[b]);
        z[a] = r.e[a] * zr - r.e[b] * zi + r.g0[a] * bw[a] - r.g0[b] * bw[b];
        z[b] = r.e[a] * zi + r.e[b] * zr + r.g0[a] * bw[b] + r.g0[b] * bw[a];
    }
    if z.len() > nzs {
        real(&mut z[nzs..], nzs);
    }
    if let Some(fz) = fz {
        for (z, f) in z.iter_mut().zip(fz) {
            *z += f;
        }
    }
}

/// LU factorisation with partial pivoting, in place (row-major, `n` by `n`): `L` below the
/// diagonal, `U` above it, the reciprocals of `U`'s diagonal on it; the rows exchanged at each
/// step into `piv`. False if singular.
fn lu_factor(a: &mut [f64], n: usize, piv: &mut [usize]) -> bool {
    for k in 0..n {
        let mut p = k;
        let mut best = a[k * n + k].abs();
        for i in k + 1..n {
            let v = a[i * n + k].abs();
            if v > best {
                best = v;
                p = i;
            }
        }
        if best == 0.0 {
            return false;
        }
        piv[k] = p;
        if p != k {
            for j in 0..n {
                a.swap(k * n + j, p * n + j);
            }
        }
        let inv = 1.0 / a[k * n + k];
        a[k * n + k] = inv;
        for i in k + 1..n {
            let f = a[i * n + k] * inv;
            a[i * n + k] = f;
            if f != 0.0 {
                for j in k + 1..n {
                    a[i * n + j] -= f * a[k * n + j];
                }
            }
        }
    }
    true
}

/// Solves `A x = b` in place with `A` as `lu_factor` left it.
#[inline(always)]
fn lu_solve(a: &[f64], n: usize, piv: &[usize], b: &mut [f64]) {
    for (k, &p) in piv.iter().enumerate() {
        b.swap(k, p);
    }
    for k in 0..n {
        let bk = b[k];
        for i in k + 1..n {
            b[i] -= a[i * n + k] * bk;
        }
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for j in i + 1..n {
            s -= a[i * n + j] * b[j];
        }
        b[i] = s * a[i * n + i];
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod path_tests {
    use super::*;

    #[test]
    fn baseline_and_v3_live_steps_match_every_bit() {
        if !v3_supported() {
            return;
        }
        use crate::devices::{Bjt, Library, fixtures};
        use crate::part::{Kind, Part};
        static BJTS: [&Bjt; 1] = [&fixtures::Q2SC945];
        let part = |refdes: String, kind: Kind, nodes: [&str; 3]| Part {
            refdes,
            kind,
            nodes: nodes
                .iter()
                .map(|n| match n.strip_prefix('@') {
                    Some(g) => g.to_string(),
                    None if *n == "0" => "0".to_string(),
                    None => format!("cb_{n}"),
                })
                .collect(),
            section: "cb".to_string(),
            block: "",
            pins: "",
            note: "",
        };
        let mut ps = Vec::new();
        for i in 1..=8 {
            let (c, b) = (format!("c{i}"), format!("b{i}"));
            ps.push(part(
                format!("RC{i}"),
                Kind::R(10_000.0 + 100.0 * i as f64),
                ["@vb1", &c, ""],
            ));
            ps.push(part(
                format!("RB{i}"),
                Kind::R(47_000.0),
                ["@drive", &b, ""],
            ));
            ps.push(part(
                format!("Q{i}"),
                Kind::Q(fixtures::Q2SC945.name),
                [&c, &b, "0"],
            ));
            ps.push(part(
                format!("C{i}"),
                Kind::C(1e-8 * (1.0 + i as f64 * 0.1)),
                [&c, "0", ""],
            ));
        }
        for p in &mut ps {
            p.nodes.retain(|n| n != "cb_");
        }
        let net = crate::net::Net::build(
            &ps,
            &crate::net::Opts {
                sources: vec!["drive".to_string()],
                ..Default::default()
            },
            &crate::net::Circuit {
                library: Library {
                    bjts: &BJTS,
                    ..Default::default()
                },
                rails: vec![("vb1".to_string(), 15.0)],
            },
        );
        let u = [1.0, 0.55];
        let op =
            crate::dc::operating_point(&net, &u).expect("small test circuit's operating point");
        let sys = crate::ss::System::new(&net, &op, &u, &["cb_c1", "cb_c2"]);
        let modal = crate::ss::Modal::new(&sys);
        let mut scalar = Live::new(
            &sys,
            &modal,
            1.0 / 48_000.0,
            4,
            &vec![true; sys.slots.len()],
            &[1],
        );
        assert_eq!(scalar.devs.len(), 8);
        assert_eq!(scalar.nn, 16, "sixteen nonlinear controls are exercised");
        assert!(scalar.y.iter().any(|x| x.abs() > 1.0));
        scalar.step_fn = step_baseline;
        let mut v3 = scalar.clone();
        v3.step_fn = step_v3;
        for i in 0..4096 {
            let u = [1.0, 0.55 + 0.2 * libm::sin(i as f64 * 0.017)];
            scalar.step(&u);
            v3.step(&u);
            for (a, b) in [
                (&scalar.z, &v3.z),
                (&scalar.v, &v3.v),
                (&scalar.r, &v3.r),
                (&scalar.y, &v3.y),
            ] {
                assert_eq!(
                    a.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    b.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    "sample {i}"
                );
            }
        }
    }
}
