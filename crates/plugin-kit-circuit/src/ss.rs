//! The network as a linear state-space system driven by its devices' departures from their
//! operating point.
//!
//! Each device is linearised at the operating point: its conductances join `G`, each
//! junction's own capacitance joins `C`, and what is left of it (its current less that
//! linearisation, its charge less the capacitance's) becomes an input. The capacitors' node
//! voltages are split into the states and the algebraic rest, which is eliminated:
//!
//! `dz/dt = A z + B w`,   `y = Cy z + D w`,
//!
//! with `w = [u; static remainders; charge-remainder currents]` and `y = [every device's
//! controls; the probes]`. [`Modal`] is its eigen-decomposition, from which the exact
//! discrete-time update for any step follows mode by mode.

use crate::dc::Op;
use crate::dev::Dev;
use crate::la::{self, C64, Lu, Mat};
use crate::net::Net;

/// A device's place in the system: where its controls are among the outputs, where its
/// static and charge remainders are among the inputs, and its linearisation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Slot {
    pub refdes: String,
    pub role: crate::net::Role,
    pub dev: Dev,
    /// For a panel pot's section: its pot's resistance, taper and section (`net::Elem::pot`).
    pub pot: Option<(f64, crate::part::Taper, bool)>,
    pub nv: usize,
    pub ns: usize,
    pub nq: usize,
    /// Offsets of its controls in `y`, its static remainders and its charge currents in `w`.
    pub v_off: usize,
    pub s_off: usize,
    pub q_off: usize,
    /// The operating point: controls, outputs (static, then charges), Jacobian.
    pub v0: Vec<f64>,
    pub f0: Vec<f64>,
    pub j0: Vec<f64>,
    /// For each charge, the control whose own capacitance at the operating point is in `C`,
    /// and that capacitance.
    pub cdiag: Vec<Option<(usize, f64)>>,
}

impl Slot {
    /// A panel pot's section's conductance at the knob's position `pos` (0 to 1).
    pub fn pot_g(&self, pos: f64) -> Option<f64> {
        let (r, taper, upper) = self.pot?;
        let (up, lo) = crate::sim::pot_halves(r, taper.frac(pos));
        Some(1.0 / if upper { up } else { lo })
    }

    /// The device's remainders at the controls `v`: the static currents' departure from the
    /// linearisation (into `rs`), the charges' (into `rq`), and their Jacobians (`js`, `jq`,
    /// row-major, outputs by controls); `scratch` holds the device's outputs and Jacobian.
    // (Five slices a solve owns: gathered in a struct, they would be borrowed again at every
    // evaluation.)
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn remainders(
        &self,
        v: &[f64],
        rs: &mut [f64],
        rq: &mut [f64],
        js: &mut [f64],
        jq: &mut [f64],
        out: &mut [f64],
        jac: &mut [f64],
    ) {
        let nv = self.nv;
        self.dev.eval(v, out, jac);
        for o in 0..self.ns {
            let mut r = out[o] - self.f0[o];
            for c in 0..nv {
                r -= self.j0[o * nv + c] * (v[c] - self.v0[c]);
                js[o * nv + c] = jac[o * nv + c] - self.j0[o * nv + c];
            }
            rs[o] = r;
        }
        for q in 0..self.nq {
            let o = self.ns + q;
            let mut r = out[o] - self.f0[o];
            for c in 0..nv {
                jq[q * nv + c] = jac[o * nv + c];
            }
            if let Some((c, c0)) = self.cdiag[q] {
                r -= c0 * (v[c] - self.v0[c]);
                jq[q * nv + c] -= c0;
            }
            rq[q] = r;
        }
    }
}

/// The linear system.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct System {
    pub a: Mat,
    pub b: Mat,
    pub cy: Mat,
    pub d: Mat,
    pub nu: usize,
    pub ns: usize,
    pub nq: usize,
    pub nv: usize,
    pub probes: Vec<String>,
    pub slots: Vec<Slot>,
    /// The inputs' values at the operating point.
    pub u0: Vec<f64>,
    /// The outputs at the operating point: the devices' controls and the probes.
    pub y0: Vec<f64>,
    pub input_names: Vec<String>,
    /// The states from the nodes' voltages (`Td' C`: an initial condition's state from its
    /// nodes' departures from the operating point).
    pub xt: Mat,
}

impl System {
    pub fn nw(&self) -> usize {
        self.nu + self.ns + self.nq
    }

    /// Builds the system of `net` linearised at `op` (inputs `u0`), with the node voltages
    /// `probes` as outputs after the devices' controls.
    pub fn new(net: &Net, op: &Op, u0: &[f64], probes: &[&str]) -> System {
        System::with_options(net, op, u0, probes, false)
    }

    /// As [`System::new`]; with `linear_junctions`, each junction's own capacitance at the
    /// operating point joins `C`, the exact linear part. Without (the default), every junction
    /// and transit charge stays whole in its device, conserved step by step: a node that only
    /// junctions hold then has no fast mode against a conducting device, which a first-order
    /// hold of that device's current would make ring (the hand clap's Q69 and D68, which charge
    /// 1 uF from such a node).
    pub fn with_options(
        net: &Net,
        op: &Op,
        u0: &[f64],
        probes: &[&str],
        linear_junctions: bool,
    ) -> System {
        let n = net.n();
        let nu = net.inputs.len();
        let x0 = &op.x;
        let mut g = net.g.clone();
        let mut c = net.c.clone();
        let mut bu = net.bu.clone();
        let mut slots = Vec::new();
        let (mut nv, mut ns, mut nq) = (0, 0, 0);
        let mut out = [0.0; 8];
        let mut jac = [0.0; 40];
        for e in &net.elems {
            let (env, ens, enq) = e.dev.shape();
            let v0: Vec<f64> = e.ctrl.iter().map(|l| l.eval(x0, u0)).collect();
            e.dev.eval(&v0, &mut out, &mut jac);
            let f0 = out[..ens + enq].to_vec();
            let mut j0 = jac[..(ens + enq) * env].to_vec();
            // A Schmitt trigger at its operating point sits on its threshold, where its state
            // node's target is regenerative: linearised there, the bank's oscillators are modes
            // at +2e8/s, which a step's exponential overflows. Its target is left out of the
            // linear part (linearised as saturated, where its slope is nil), whole in its
            // departure; the state node keeps its own resistance and capacitance.
            if let Dev::Schmitt(_) = e.dev {
                for x in j0[..env].iter_mut() {
                    *x = 0.0;
                }
            }
            // Static outputs: conductances into G, the linearisation's constant and its input
            // terms into Bu.
            for o in 0..ens {
                let mut k0 = f0[o];
                for ci in 0..env {
                    k0 -= j0[o * env + ci] * v0[ci];
                }
                for &(node, s) in &e.inj[o] {
                    bu[(node, 0)] += s * k0;
                    for ci in 0..env {
                        let jv = j0[o * env + ci];
                        if jv == 0.0 {
                            continue;
                        }
                        for &(xi, xc) in &e.ctrl[ci].x {
                            g[(node, xi)] -= s * jv * xc;
                        }
                        for &(ui, uc) in &e.ctrl[ci].u {
                            bu[(node, ui)] += s * jv * uc;
                        }
                    }
                }
            }
            // Charges: with `linear_junctions`, a junction's own capacitance at the operating
            // point joins C when the charge sits across the very voltage it is controlled by.
            let mut cdiag = vec![None; enq];
            for q in 0..enq {
                let o = ens + q;
                let inj = &e.inj[o];
                for ci in 0..env {
                    let ctrl = &e.ctrl[ci];
                    let paired = ctrl.u.iter().all(|&(k, _)| k == 0)
                        && inj.len() == ctrl.x.len()
                        && inj
                            .iter()
                            .all(|&(i, s)| ctrl.x.iter().any(|&(j, c)| j == i && c == -s));
                    let c0 = j0[o * env + ci];
                    if paired && c0 > 0.0 && linear_junctions {
                        cdiag[q] = Some((ci, c0));
                        for &(i, ci_) in &ctrl.x {
                            for &(j, cj) in &ctrl.x {
                                c[(i, j)] += c0 * ci_ * cj;
                            }
                        }
                        break;
                    }
                }
            }
            slots.push(Slot {
                refdes: e.refdes.clone(),
                role: e.role,
                dev: e.dev,
                pot: e.pot,
                nv: env,
                ns: ens,
                nq: enq,
                v_off: nv,
                s_off: ns,
                q_off: nq,
                v0,
                f0,
                j0,
                cdiag,
            });
            nv += env;
            ns += ens;
            nq += enq;
        }
        for s in &mut slots {
            s.s_off += nu;
            s.q_off += nu + ns;
        }
        let nw = nu + ns + nq;
        // W: each input's injection into the nodes.
        let mut w = Mat::zeros(n, nw);
        for i in 0..n {
            for k in 0..nu {
                w[(i, k)] = bu[(i, k)];
            }
        }
        for (e, s) in net.elems.iter().zip(&slots) {
            for o in 0..s.ns {
                for &(node, sg) in &e.inj[o] {
                    w[(node, s.s_off + o)] += sg;
                }
            }
            for q in 0..s.nq {
                for &(node, sg) in &e.inj[s.ns + q] {
                    w[(node, s.q_off + q)] += sg;
                }
            }
        }
        // Y: the outputs' rows over the nodes, and their direct input terms.
        let np = probes.len();
        let mut y = Mat::zeros(nv + np, n);
        let mut yu = Mat::zeros(nv + np, nw);
        for (e, s) in net.elems.iter().zip(&slots) {
            for (ci, l) in e.ctrl.iter().enumerate() {
                for &(xi, xc) in &l.x {
                    y[(s.v_off + ci, xi)] += xc;
                }
                for &(ui, uc) in &l.u {
                    yu[(s.v_off + ci, ui)] += uc;
                }
            }
        }
        for (k, p) in probes.iter().enumerate() {
            let i = net
                .node(p)
                .unwrap_or_else(|| panic!("no node {p} to probe"));
            y[(nv + k, i)] = 1.0;
        }
        let (a, b, cy, d, xt) = descriptor_to_ode(&c, &g, &w, &y);
        let mut y0: Vec<f64> = slots.iter().flat_map(|s| s.v0.iter().copied()).collect();
        for p in probes {
            y0.push(x0[net.node(p).expect("probe")]);
        }
        System {
            a,
            b,
            cy,
            d: d.add(&yu),
            nu,
            ns,
            nq,
            nv,
            probes: probes.iter().map(|s| s.to_string()).collect(),
            slots,
            u0: u0.to_vec(),
            y0,
            input_names: net.inputs.clone(),
            xt,
        }
    }
}

/// `C dx/dt + G x = W w`, `y = Y x` as `dz/dt = A z + B w`, `y = Cy z + D w`: the capacitors'
/// nodes split, group by group of capacitively connected nodes, into the states (scaled so
/// that the capacitance is the identity) and the null directions, which join the nodes
/// without capacitance as algebraic unknowns and are eliminated. Also the states from the
/// nodes' voltages, `Td' C`.
fn descriptor_to_ode(c: &Mat, g: &Mat, w: &Mat, y: &Mat) -> (Mat, Mat, Mat, Mat, Mat) {
    let n = c.rows;
    // Groups of nodes joined by capacitance (union-find).
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut j = i;
        while p[j] != r {
            let next = p[j];
            p[j] = r;
            j = next;
        }
        r
    }
    for i in 0..n {
        for j in 0..n {
            if i != j && c[(i, j)] != 0.0 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        if c[(i, i)] > 0.0 {
            let r = find(&mut parent, i);
            groups.entry(r).or_default().push(i);
        }
    }
    // Basis columns: dynamic (scaled eigenvectors) and algebraic (null vectors and the
    // nodes without capacitance).
    let mut td: Vec<Vec<(usize, f64)>> = Vec::new();
    let mut ta: Vec<Vec<(usize, f64)>> = Vec::new();
    let mut has_c = vec![false; n];
    for nodes in groups.values() {
        let k = nodes.len();
        let mut sub = Mat::zeros(k, k);
        for (a, &i) in nodes.iter().enumerate() {
            has_c[i] = true;
            for (b, &j) in nodes.iter().enumerate() {
                sub[(a, b)] = c[(i, j)];
            }
        }
        let (lam, u) = la::sym_eig(&sub);
        let lmax = lam[0].max(0.0);
        for (col, &l) in lam.iter().enumerate() {
            let v: Vec<(usize, f64)> = nodes
                .iter()
                .enumerate()
                .map(|(a, &i)| (i, u[(a, col)]))
                .collect();
            if l > 1e-11 * lmax {
                let s = 1.0 / l.sqrt();
                td.push(v.into_iter().map(|(i, x)| (i, x * s)).collect());
            } else {
                ta.push(v);
            }
        }
    }
    for (i, &hc) in has_c.iter().enumerate() {
        if !hc {
            ta.push(vec![(i, 1.0)]);
        }
    }
    let nd = td.len();
    let na = ta.len();
    let to_mat = |cols: &[Vec<(usize, f64)>]| {
        let mut m = Mat::zeros(n, cols.len());
        for (j, col) in cols.iter().enumerate() {
            for &(i, x) in col {
                m[(i, j)] = x;
            }
        }
        m
    };
    let tdm = to_mat(&td);
    let tam = to_mat(&ta);
    let gtd = g.mul(&tdm);
    let gta = g.mul(&tam);
    let tdt = tdm.t();
    let tat = tam.t();
    let g_dd = tdt.mul(&gtd);
    let g_da = tdt.mul(&gta);
    let g_ad = tat.mul(&gtd);
    let g_aa = tat.mul(&gta);
    let lu = Lu::new(&g_aa);
    // A network whose every unknown has a capacitor has no algebraic part (K-CIRCUIT: the
    // TR-808's networks always had one, so this guard changes none of them).
    assert!(na == 0 || lu.rcond > 0.0, "the algebraic part is singular");
    // G_aa⁻¹ G_ad and G_aa⁻¹ T_aᵀ.
    let gi_gad = lu.solve_mat(&g_ad);
    let gi_tat = lu.solve_mat(&tat);
    let a = g_dd.sub(&g_da.mul(&gi_gad)).scale(-1.0);
    let br = tdt.sub(&g_da.mul(&gi_tat));
    let b = br.mul(w);
    let mx = tdm.sub(&tam.mul(&gi_gad));
    let fx = tam.mul(&gi_tat);
    let cy = y.mul(&mx);
    let d = y.mul(&fx).mul(w);
    let _ = (nd, na);
    let xt = tdt.mul(c);
    (a, b, cy, d, xt)
}

/// A system in modal form: `dζ_k/dt = μ_k ζ_k + b_k · w`, `y = Re(Σ_k c_k ζ_k) + D w`, one
/// complex coordinate for each real mode and for each complex pair.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Modal {
    pub mu: Vec<C64>,
    /// Rows: modes; columns: inputs.
    pub b: Vec<Vec<C64>>,
    /// Rows: outputs; columns: modes.
    pub c: Vec<Vec<C64>>,
    pub d: Mat,
    /// The largest relative residual of `A V = V Λ`.
    pub residual: f64,
    /// An estimate of the eigenvectors' condition number, `|V| |V⁻¹| n`.
    pub cond: f64,
    /// The real eigenvectors (a complex pair's real and imaginary parts side by side), their
    /// inverse, and each mode's first column and whether it is a pair.
    pub v: Mat,
    pub vinv: Mat,
    pub cols: Vec<(usize, bool)>,
}

impl Modal {
    /// The state in the system's own coordinates from the modal state.
    pub fn to_real(&self, z: &[C64]) -> Vec<f64> {
        let n = self.v.rows;
        let mut xi = vec![0.0; n];
        for (k, &(c, pair)) in self.cols.iter().enumerate() {
            xi[c] = z[k].re;
            if pair {
                xi[c + 1] = z[k].im;
            }
        }
        self.v.mul_vec(&xi)
    }

    /// The modal state from the state in the system's own coordinates.
    pub fn from_real(&self, x: &[f64]) -> Vec<C64> {
        let xi = self.vinv.mul_vec(x);
        self.cols
            .iter()
            .map(|&(c, pair)| C64::new(xi[c], if pair { xi[c + 1] } else { 0.0 }))
            .collect()
    }

    pub fn new(sys: &System) -> Modal {
        let nn = sys.a.rows;
        let ed = la::eig(&sys.a);
        // Check the decomposition.
        let mut dm = Mat::zeros(nn, nn);
        for i in 0..nn {
            dm[(i, i)] = ed.d[i];
            if ed.e[i] > 0.0 {
                dm[(i, i + 1)] = ed.e[i];
            } else if ed.e[i] < 0.0 {
                dm[(i, i - 1)] = ed.e[i];
            }
        }
        let r = sys.a.mul(&ed.v).sub(&ed.v.mul(&dm));
        let residual = r.max_abs() / (sys.a.max_abs() * ed.v.max_abs()).max(1e-300);
        let vinv = Lu::new(&ed.v).inverse();
        let bm = vinv.mul(&sys.b);
        let cm = sys.cy.mul(&ed.v);
        let mut mu = Vec::new();
        let mut cols = Vec::new();
        let mut b = Vec::new();
        let mut c: Vec<Vec<C64>> = vec![Vec::new(); sys.cy.rows];
        let mut i = 0;
        while i < nn {
            if ed.e[i] == 0.0 {
                cols.push((i, false));
                mu.push(C64::new(ed.d[i], 0.0));
                b.push((0..bm.cols).map(|k| C64::new(bm[(i, k)], 0.0)).collect());
                for (o, row) in c.iter_mut().enumerate() {
                    row.push(C64::new(cm[(o, i)], 0.0));
                }
                i += 1;
            } else {
                // The block [[a, e], [-e, a]] on (ξ1, ξ2): ζ = ξ1 + i ξ2 runs at a - i e, is
                // driven by b1 + i b2, and gives Re((c1 - i c2) ζ).
                cols.push((i, true));
                mu.push(C64::new(ed.d[i], -ed.e[i]));
                b.push(
                    (0..bm.cols)
                        .map(|k| C64::new(bm[(i, k)], bm[(i + 1, k)]))
                        .collect(),
                );
                for (o, row) in c.iter_mut().enumerate() {
                    row.push(C64::new(cm[(o, i)], -cm[(o, i + 1)]));
                }
                i += 2;
            }
        }
        Modal {
            mu,
            b,
            c,
            d: sys.d.clone(),
            residual,
            cond: ed.v.max_abs() * vinv.max_abs() * nn as f64,
            v: ed.v,
            vinv,
            cols,
        }
    }

    /// The modal state at rest with inputs `w` held: ζ = -b·w / μ.
    pub fn rest(&self, w: &[f64]) -> Vec<C64> {
        self.mu
            .iter()
            .zip(&self.b)
            .map(|(m, row)| {
                let bw = row
                    .iter()
                    .zip(w)
                    .fold(C64::ZERO, |acc, (b, x)| acc + b.scale(*x));
                -(bw / *m)
            })
            .collect()
    }

    /// The outputs at state `z` and inputs `w`.
    pub fn outputs(&self, z: &[C64], w: &[f64]) -> Vec<f64> {
        let dw = self.d.mul_vec(w);
        self.c
            .iter()
            .zip(dw)
            .map(|(row, dv)| row.iter().zip(z).map(|(c, x)| (*c * *x).re).sum::<f64>() + dv)
            .collect()
    }
}

/// The exact discrete-time weights of one mode over a step `h`: `e^{μh}`, the weight of the
/// step's first and last input under a first-order hold (`γ0`, `γ1`), and of an input held at
/// its final value (`γa`).
pub fn weights(mu: C64, h: f64) -> (C64, C64, C64, C64) {
    let x = mu.scale(h);
    let e = x.exp();
    let (ga, g1) = if x.abs() < 0.05 {
        // Series: γa = h Σ x^k/(k+1)!, γ1 = h Σ x^k/(k+2)!.
        let mut ga = C64::ZERO;
        let mut g1 = C64::ZERO;
        let mut p = C64::ONE;
        let mut fa = 1.0;
        for k in 0..12 {
            fa *= (k + 1) as f64;
            ga = ga + p.scale(1.0 / fa);
            g1 = g1 + p.scale(1.0 / (fa * (k + 2) as f64));
            p = p * x;
        }
        (ga.scale(h), g1.scale(h))
    } else {
        let em1 = e - C64::ONE;
        (em1 / mu, (em1 - x) / (mu * x))
    };
    (e, ga - g1, g1, ga)
}
