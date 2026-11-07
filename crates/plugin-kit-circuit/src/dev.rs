//! The devices' equations, as ngspice 47 evaluates the models an instrument's lab writes into
//! its netlists: the Gummel-Poon transistor (bjtload.c), the junction diode with its
//! breakdown (dioload.c, diotemp.c), the level-1 JFET (jfetload.c), each with its junction and
//! transit charges; and the behavioural subcircuits (the op-amp's input stage and clamps, the
//! Schmitt inverter, the comparator, the transconductance amplifier), whose expressions are the
//! netlists' B-sources. Every model is at the circuit's temperature (TNOM = TEMP), so none of
//! ngspice's temperature adjustments changes a parameter.
//!
//! A device is a function from its controlling voltages to its outputs: static currents, and
//! charges (whose currents are their time derivatives). [`Dev::eval`] gives the outputs and
//! their Jacobian.

use crate::devices::{Bjt, Comparator, Diode, Jfet, Mos, OpAmp, Ota, Schmitt};
use crate::sim;

/// ngspice's Boltzmann constant and electron charge (`const.h`).
const BOLTZ: f64 = 1.380_648_52e-23;
const CHARGE: f64 = 1.602_176_620_8e-19;
/// ngspice's GMIN, S, across every junction.
pub const GMIN: f64 = 1e-12;
/// The default depletion-capacitance coefficient FC.
const FC: f64 = 0.5;
const E: f64 = std::f64::consts::E;

/// The thermal voltage at the circuit's temperature, as ngspice computes it.
pub fn vt() -> f64 {
    BOLTZ / CHARGE * (crate::devices::TEMP_C + 273.15)
}

/// A junction's depletion charge and capacitance (ngspice's form, with its linear extension
/// above `fc * pb`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Depletion {
    pub cj0: f64,
    pub pb: f64,
    pub m: f64,
    fcp: f64,
    f1: f64,
    f2: f64,
    f3: f64,
    /// `1 / pb`, `pb cj0 / (1 - m)`, `cj0 / f2`, `m / (2 pb)`: the divisions done once.
    inv_pb: f64,
    k1: f64,
    czf2: f64,
    mh: f64,
}

impl Depletion {
    pub fn new(cj0: f64, pb: f64, m: f64) -> Depletion {
        let xfc = libm::log(1.0 - FC);
        let f2 = libm::exp((1.0 + m) * xfc);
        Depletion {
            cj0,
            pb,
            m,
            fcp: FC * pb,
            f1: pb * (1.0 - libm::exp((1.0 - m) * xfc)) / (1.0 - m),
            f2,
            f3: 1.0 - FC * (1.0 + m),
            inv_pb: 1.0 / pb,
            k1: pb * cj0 / (1.0 - m),
            czf2: cj0 / f2,
            mh: m / (pb + pb),
        }
    }

    /// (charge, capacitance) at `v`.
    #[inline]
    pub fn qc(&self, v: f64) -> (f64, f64) {
        if self.cj0 == 0.0 {
            return (0.0, 0.0);
        }
        if v < self.fcp {
            let arg = 1.0 - v * self.inv_pb;
            let sarg = libm::exp(-self.m * libm::log(arg));
            (self.k1 * (1.0 - arg * sarg), self.cj0 * sarg)
        } else {
            (
                self.cj0 * self.f1
                    + self.czf2
                        * (self.f3 * (v - self.fcp) + self.mh * (v * v - self.fcp * self.fcp)),
                self.czf2 * (self.f3 + self.m * v * self.inv_pb),
            )
        }
    }
}

/// A junction's ideal current `is (exp(v / vtn) - 1)` and conductance, with ngspice's cubic
/// reverse extension below -3 vtn (bjtload.c, dioload.c).
/// `inv_vtn` is `1 / vtn`.
#[inline]
fn junction_r(is: f64, v: f64, vtn: f64, inv_vtn: f64) -> (f64, f64) {
    if v >= -3.0 * vtn {
        let ev = libm::exp(v * inv_vtn);
        let iev = is * ev;
        (iev - is, iev * inv_vtn)
    } else {
        let arg = 3.0 * vtn / (v * E);
        let arg3 = arg * arg * arg;
        (-is * (1.0 + arg3), is * 3.0 * arg3 / v)
    }
}

/// A bipolar transistor's parameters as ngspice uses them. Controls `[vbe, vbc]` (in the
/// device's own polarity, at its internal nodes); outputs: static `[ic, ib]` (into the
/// collector and the base), charges `[qbe, qbc]`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BjtP {
    pub pnp: bool,
    is: f64,
    bf: f64,
    br: f64,
    inv_vaf: f64,
    oik: f64,
    vt: f64,
    tf: f64,
    tr: f64,
    be: Depletion,
    bc: Depletion,
    /// `1 / bf`, `1 / br`, `1 / vt`.
    inv_bf: f64,
    inv_br: f64,
    inv_vt: f64,
    /// A behavioural transistor's junctions: exponential up to this voltage, straight on
    /// above it (infinite for the device-level model).
    vlin: f64,
}

impl BjtP {
    pub fn new(m: &Bjt) -> BjtP {
        BjtP {
            pnp: m.pnp,
            is: m.is,
            bf: m.bf,
            br: m.br,
            inv_vaf: if m.vaf.is_finite() && m.vaf > 0.0 {
                1.0 / m.vaf
            } else {
                0.0
            },
            oik: if m.ikf.is_finite() && m.ikf > 0.0 {
                1.0 / m.ikf
            } else {
                0.0
            },
            vt: vt(),
            tf: m.tf,
            tr: m.tr,
            be: Depletion::new(m.cje, m.vje, m.mje),
            bc: Depletion::new(m.cjc, m.vjc, m.mjc),
            inv_bf: 1.0 / m.bf,
            inv_br: 1.0 / m.br,
            inv_vt: 1.0 / vt(),
            vlin: f64::INFINITY,
        }
    }

    /// A behavioural transistor (D58): its junctions exponential up to `i_lin` and straight on
    /// above it (their slope there), so a junction driven hard forward is near linear.
    pub fn straightened(m: &Bjt, i_lin: f64) -> BjtP {
        let mut p = BjtP::new(m);
        p.vlin = p.vt * libm::log(i_lin / p.is + 1.0);
        p
    }

    /// A junction's current and slope, straightened above `vlin`.
    #[inline]
    fn junction(&self, v: f64) -> (f64, f64) {
        if v > self.vlin {
            let (c, g) = junction_r(self.is, self.vlin, self.vt, self.inv_vt);
            (c + g * (v - self.vlin), g)
        } else {
            junction_r(self.is, v, self.vt, self.inv_vt)
        }
    }

    /// A behavioural transistor (D58): its junctions `n` times softer (the emission coefficient
    /// n), their saturation current refitted so that each junction carries `i_ref` at the same
    /// voltage as the device-level model's; all else its own.
    pub fn soft(m: &Bjt, n: f64, i_ref: f64) -> BjtP {
        let mut p = BjtP::new(m);
        let v_ref = p.vt * libm::log(i_ref / p.is + 1.0);
        p.vt *= n;
        p.inv_vt = 1.0 / p.vt;
        p.is = i_ref / (libm::exp(v_ref / p.vt) - 1.0);
        p
    }

    /// Outputs `[ic, ib, qbe, qbc]` and their Jacobian (4 x 2, row-major) at `[vbe, vbc]`.
    #[inline]
    pub fn eval(&self, v: &[f64], out: &mut [f64], jac: &mut [f64]) {
        let (vbe, vbc) = (v[0], v[1]);
        let (cbe, gbe) = self.junction(vbe);
        let (cbc, gbc) = self.junction(vbc);
        // GMIN across each junction (cben, cbcn).
        let (cben, gben) = (GMIN * vbe, GMIN);
        let (cbcn, gbcn) = (GMIN * vbc, GMIN);
        // The base charge: Early effect and high injection (no IKR, no VAR).
        let q1 = 1.0 / (1.0 - self.inv_vaf * vbc);
        let (qb, dqbdve, dqbdvc) = if self.oik == 0.0 {
            (q1, 0.0, q1 * q1 * self.inv_vaf)
        } else {
            let q2 = self.oik * cbe;
            let arg = (1.0 + 4.0 * q2).max(0.0);
            let sqarg = if arg != 0.0 { arg.sqrt() } else { 1.0 };
            let qb = q1 * (1.0 + sqarg) / 2.0;
            (qb, q1 * (self.oik * gbe / sqarg), q1 * (qb * self.inv_vaf))
        };
        // Currents (bjtload.c: cc, cb) and their derivatives.
        let iqb = 1.0 / qb;
        let ct = (cbe - cbc) * iqb;
        let cc = ct - cbc * self.inv_br - cbcn;
        let cb = cbe * self.inv_bf + cben + cbc * self.inv_br + cbcn;
        let dcc_dvbe = (gbe - ct * dqbdve) * iqb;
        let dcc_dvbc = (-gbc - ct * dqbdvc) * iqb - gbc * self.inv_br - gbcn;
        let dcb_dvbe = gbe * self.inv_bf + gben;
        let dcb_dvbc = gbc * self.inv_br + gbcn;
        out[0] = cc;
        out[1] = cb;
        jac[0] = dcc_dvbe;
        jac[1] = dcc_dvbc;
        jac[2] = dcb_dvbe;
        jac[3] = dcb_dvbc;
        // Charges: the diffusion charge tf cbe / qb (for vbe > 0, no XTF) and tr cbc, and the
        // depletion charges.
        let (qde, cde) = self.be.qc(vbe);
        let (qdc, cdc) = self.bc.qc(vbc);
        let (qtf, dqtf_dve, dqtf_dvc) = if self.tf != 0.0 && vbe > 0.0 {
            let c = cbe * iqb;
            (
                self.tf * c,
                self.tf * (gbe - c * dqbdve) * iqb,
                self.tf * (-c * dqbdvc) * iqb,
            )
        } else {
            (self.tf * cbe, self.tf * gbe, 0.0)
        };
        out[2] = qtf + qde;
        out[3] = self.tr * cbc + qdc;
        jac[4] = dqtf_dve + cde;
        jac[5] = dqtf_dvc;
        jac[6] = 0.0;
        jac[7] = self.tr * gbc + cdc;
    }
}

/// A junction diode. Control `[vd]`; outputs: static `[id]`, charge `[q]`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiodeP {
    is: f64,
    vte: f64,
    tt: f64,
    cj: Depletion,
    /// The breakdown voltage ngspice derives (DIOtBrkdwnV), infinite without breakdown.
    bv: f64,
    vtebrk: f64,
}

impl DiodeP {
    /// A diode with ngspice's breakdown matched at `reltol` (diotemp.c takes the run's
    /// RELTOL as its tolerance).
    pub fn new(d: &Diode, reltol: f64) -> DiodeP {
        DiodeP::from_parts(d.is, d.n, d.cjo, 1.0, 0.5, d.tt, d.bv, d.ibv, reltol)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        is: f64,
        n: f64,
        cjo: f64,
        vj: f64,
        m: f64,
        tt: f64,
        bv: f64,
        ibv: f64,
        reltol: f64,
    ) -> DiodeP {
        let vt = vt();
        let mut xbv = f64::INFINITY;
        if bv.is_finite() {
            // diotemp.c: the breakdown voltage at which the current is IBV.
            let tbv = bv.abs();
            let cbv = ibv;
            if cbv < is * tbv / vt {
                // ngspice raises the breakdown current to is bv / vt and takes bv as it is.
                xbv = tbv;
            } else {
                let tol = reltol * cbv;
                let nbv = n;
                xbv = tbv - nbv * vt * libm::log(1.0 + cbv / is);
                for _ in 0..25 {
                    xbv = tbv - nbv * vt * libm::log(cbv / is + 1.0 - xbv / vt);
                    let xcbv = is * (libm::exp((tbv - xbv) / (nbv * vt)) - 1.0 + xbv / vt);
                    if (xcbv - cbv).abs() <= tol {
                        break;
                    }
                }
            }
        }
        DiodeP {
            is,
            vte: n * vt,
            tt,
            cj: Depletion::new(cjo, vj, m),
            bv: xbv,
            vtebrk: n * vt,
        }
    }

    /// The breakdown voltage in use (infinite without breakdown).
    pub fn breakdown(&self) -> f64 {
        self.bv
    }

    #[inline]
    pub fn eval(&self, v: &[f64], out: &mut [f64], jac: &mut [f64]) {
        let vd = v[0];
        let (cd, gd) = if vd >= -3.0 * self.vte {
            let ev = libm::exp(vd / self.vte);
            (self.is * (ev - 1.0), self.is * ev / self.vte)
        } else if vd >= -self.bv {
            let arg = 3.0 * self.vte / (vd * E);
            let arg3 = arg * arg * arg;
            (-self.is * (1.0 + arg3), self.is * 3.0 * arg3 / vd)
        } else {
            let evrev = libm::exp(-(self.bv + vd) / self.vtebrk);
            (-self.is * evrev, self.is * evrev / self.vtebrk)
        };
        let (cd, gd) = (cd + GMIN * vd, gd + GMIN);
        out[0] = cd;
        jac[0] = gd;
        let (qd, cdep) = self.cj.qc(vd);
        out[1] = self.tt * cd + qd;
        jac[1] = self.tt * gd + cdep;
    }
}

/// A level-1 N-channel JFET. Controls `[vgs, vgd]`; outputs: static `[id, igs, igd]` (drain to
/// source, gate to source, gate to drain), charges `[qgs, qgd]`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JfetP {
    vto: f64,
    beta: f64,
    lambda: f64,
    is: f64,
    vt: f64,
    gs: Depletion,
    gd: Depletion,
}

impl JfetP {
    pub fn new(j: &Jfet) -> JfetP {
        JfetP {
            vto: j.vto,
            beta: j.beta,
            lambda: j.lambda,
            is: j.is,
            vt: vt(),
            gs: Depletion::new(j.cgs, j.pb, 0.5),
            gd: Depletion::new(j.cgd, j.pb, 0.5),
        }
    }

    fn gate(&self, v: f64) -> (f64, f64) {
        if v <= -5.0 * self.vt {
            // ngspice's current (-is + gmin v); its "conductance" there is a secant value, so
            // the derivative is given instead (only Newton's path depends on it).
            let g = -self.is / v + GMIN;
            (g * v, GMIN)
        } else {
            let ev = libm::exp(v / self.vt);
            (
                self.is * (ev - 1.0) + GMIN * v,
                self.is * ev / self.vt + GMIN,
            )
        }
    }

    #[inline]
    pub fn eval(&self, v: &[f64], out: &mut [f64], jac: &mut [f64]) {
        let (vgs, vgd) = (v[0], v[1]);
        let (cgs, ggs) = self.gate(vgs);
        let (cgd, ggd) = self.gate(vgd);
        let vds = vgs - vgd;
        // Shichman-Hodges (B = 1): normal mode for vds >= 0, inverse otherwise.
        let (id, did_dvgs, did_dvgd) = if vds >= 0.0 {
            let vgst = vgs - self.vto;
            if vgst <= 0.0 {
                (0.0, 0.0, 0.0)
            } else {
                let betap = self.beta * (1.0 + self.lambda * vds);
                let (cpart, gm, gds) = if vgst >= vds {
                    let apart = 2.0;
                    let cpart = vds * (-vds + vgst * apart);
                    (
                        cpart,
                        betap * vds * apart,
                        betap * (vgst - vds) * apart + self.beta * self.lambda * cpart,
                    )
                } else {
                    let cpart = vgst * vgst;
                    (cpart, betap * vgst * 2.0, self.lambda * self.beta * cpart)
                };
                // id(vgs, vds): d/dvgs = gm + gds, d/dvgd = -gds.
                (betap * cpart, gm + gds, -gds)
            }
        } else {
            let vgdt = vgd - self.vto;
            if vgdt <= 0.0 {
                (0.0, 0.0, 0.0)
            } else {
                let nvds = -vds;
                let betap = self.beta * (1.0 + self.lambda * nvds);
                let (cpart, gm, gds) = if vgdt >= nvds {
                    let apart = 2.0;
                    let cpart = nvds * (-nvds + vgdt * apart);
                    (
                        cpart,
                        betap * nvds * apart,
                        betap * (vgdt - nvds) * apart + self.beta * self.lambda * cpart,
                    )
                } else {
                    let cpart = vgdt * vgdt;
                    (cpart, betap * vgdt * 2.0, self.lambda * self.beta * cpart)
                };
                // id = -betap cpart(vgd, -vds): d/dvgd = -(gm + gds), d/dvgs = gds.
                (-betap * cpart, gds, -(gm + gds))
            }
        };
        out[0] = id;
        out[1] = cgs;
        out[2] = cgd;
        jac[0] = did_dvgs;
        jac[1] = did_dvgd;
        jac[2] = ggs;
        jac[3] = 0.0;
        jac[4] = 0.0;
        jac[5] = ggd;
        let (qgs, cgs) = self.gs.qc(vgs);
        let (qgd, cgd) = self.gd.qc(vgd);
        out[3] = qgs;
        out[4] = qgd;
        jac[6] = cgs;
        jac[7] = 0.0;
        jac[8] = 0.0;
        jac[9] = cgd;
    }
}

/// A MOS transistor (`devices::Mos`), in its own polarity as `mos1load.c` works (every voltage
/// and current times the device's type, +1 N-channel, -1 P-channel; the network gives it its
/// controls and takes its outputs so, so that a junction's forward direction is positive for
/// Newton's limiting whatever the type): controls `[vgs, vds, vbs, vbd]`; outputs the channel's
/// current from drain to source and the bulk-source and bulk-drain junctions' currents (from the
/// bulk). The channel in normal mode for vds >= 0 and inverse mode otherwise; each junction
/// `is (e^(v / vt) - 1) + gmin v` above -3 vt, its exponent's argument capped at 709 as
/// ngspice's `MAX_EXP_ARG`, and `gmin v - is` below.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MosP {
    von: f64,
    beta: f64,
    lambda: f64,
    is: f64,
    vt: f64,
}

impl MosP {
    pub fn new(m: &Mos) -> MosP {
        MosP {
            von: m.type_sign() * m.vto,
            beta: m.beta,
            lambda: m.lambda,
            is: m.is,
            vt: vt(),
        }
    }

    /// The channel's current in normal mode (vds >= 0) and its derivatives in vgs and vds.
    fn channel(&self, vgs: f64, vds: f64) -> (f64, f64, f64) {
        let vgst = vgs - self.von;
        if vgst <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let betap = self.beta * (1.0 + self.lambda * vds);
        if vgst <= vds {
            (
                betap * vgst * vgst * 0.5,
                betap * vgst,
                self.lambda * self.beta * vgst * vgst * 0.5,
            )
        } else {
            (
                betap * vds * (vgst - 0.5 * vds),
                betap * vds,
                betap * (vgst - vds) + self.lambda * self.beta * vds * (vgst - 0.5 * vds),
            )
        }
    }

    fn junction(&self, v: f64) -> (f64, f64) {
        if v <= -3.0 * self.vt {
            (GMIN * v - self.is, GMIN)
        } else {
            let e = libm::exp((v / self.vt).min(709.0));
            (self.is * (e - 1.0) + GMIN * v, self.is * e / self.vt + GMIN)
        }
    }

    #[inline]
    pub fn eval(&self, v: &[f64], out: &mut [f64], jac: &mut [f64]) {
        let (vgs, vds, vbs, vbd) = (v[0], v[1], v[2], v[3]);
        let (id, did_vgs, did_vds) = if vds >= 0.0 {
            self.channel(vgs, vds)
        } else {
            // Inverse mode: the drain acts as the source. id(vgs, vds) = -f(vgs - vds, -vds).
            let (f, fg, fd) = self.channel(vgs - vds, -vds);
            (-f, -fg, fg + fd)
        };
        let (ibs, gbs) = self.junction(vbs);
        let (ibd, gbd) = self.junction(vbd);
        out[0] = id;
        out[1] = ibs;
        out[2] = ibd;
        for x in jac[..12].iter_mut() {
            *x = 0.0;
        }
        jac[0] = did_vgs;
        jac[1] = did_vds;
        jac[6] = gbs;
        jac[11] = gbd;
    }
}

/// The op-amp's input stage: control `[vp - vn]`, output the current into its internal node,
/// `imax tanh(gm vd / imax)`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpIn {
    pub gm: f64,
    pub imax: f64,
}

impl OpIn {
    pub fn new(o: &OpAmp) -> OpIn {
        let gm = sim::OPAMP_GM;
        let cx = gm / (2.0 * std::f64::consts::PI * o.gbw);
        OpIn {
            gm,
            imax: o.sr * cx * (1.0 + o.rout / o.sr_load),
        }
    }
}

/// A soft clamp: controls `[a, b]`, output `g (softplus(a) - softplus(b))` (the current out
/// of the clamped node).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clamp {
    pub g: f64,
    pub knee: f64,
}

/// The Schmitt inverter: controls `[vin - vss, vdd - vss, s, vout - vss]`; outputs: the
/// current into the state node `target / r`, the current from VDD into the output and the
/// current from the output into VSS.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SchmittP {
    pub vtp: f64,
    pub vtn: f64,
    pub w: f64,
    pub r: f64,
    pub rout: f64,
}

impl SchmittP {
    pub fn new(m: &Schmitt) -> SchmittP {
        SchmittP {
            vtp: m.vt_pos,
            vtn: m.vt_neg,
            w: sim::SCHMITT_WIDTH,
            r: m.tau / sim::SCHMITT_CS,
            rout: m.rout,
        }
    }
}

/// The comparator: controls `[vn - vp, vp - vcc, vn - vcc, s, vo - vgnd]`; outputs: the
/// current into the state node `target / r`, the output's sink current.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CmpP {
    pub w: f64,
    pub vcm: f64,
    pub wcm: f64,
    pub r: f64,
    pub isink: f64,
    pub vsat: f64,
}

impl CmpP {
    pub fn new(c: &Comparator) -> CmpP {
        CmpP {
            w: c.vin_width,
            vcm: c.vcm_top,
            wcm: sim::CMP_CM_WIDTH,
            r: c.tau / sim::CMP_CS,
            isink: c.isink,
            vsat: c.vsat,
        }
    }
}

/// The transconductance amplifier: controls `[v(iabc) - v(v-), v(in+) - v(in-)]`; outputs: the
/// bias diode's current (from the bias pin to V-) and the output current (into the output).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OtaP {
    pub diode: DiodeP,
    pub k: f64,
}

impl OtaP {
    pub fn new(o: &Ota) -> OtaP {
        let is = sim::OTA_BIAS_I / libm::exp(o.vbias / crate::devices::VT);
        OtaP {
            diode: DiodeP::from_parts(is, 1.0, 0.0, 1.0, 0.5, 0.0, f64::INFINITY, 0.0, 1e-3),
            k: o.gm_per_i,
        }
    }
}

/// A nonlinear device.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Dev {
    /// A panel pot's section in the real-time part: a conductance the knob sets, its current
    /// `g v` (from the section's first terminal to its second).
    Cond(f64),
    Bjt(BjtP),
    Diode(DiodeP),
    Jfet(JfetP),
    OpIn(OpIn),
    Clamp(Clamp),
    Schmitt(SchmittP),
    Cmp(CmpP),
    Ota(OtaP),
    Mos(MosP),
}

impl Dev {
    /// The controls ngspice starts a junction device's first Newton iteration at in its
    /// `MODEINITJCT` (`dioload.c`, `bjtload.c`, `jfetload.c`, `mos1load.c`), whatever the nodes'
    /// voltages: a diode's and a transistor's emitter junction at their critical voltage
    /// (`vte ln(vte / (sqrt 2 is))`), the collector junction at 0, a JFET's gate junctions at -1 V,
    /// and a MOS transistor at its threshold with vds 0 and its bulk junctions at -1 V (in its own
    /// polarity). `None` for the behavioural models, which ngspice evaluates at the nodes.
    pub fn init_junctions(&self) -> Option<Vec<f64>> {
        let vcrit = |vte: f64, is: f64| vte * libm::log(vte / (std::f64::consts::SQRT_2 * is));
        match self {
            Dev::Diode(d) => Some(vec![vcrit(d.vte, d.is)]),
            Dev::Bjt(b) => Some(vec![vcrit(b.vt, b.is), 0.0]),
            Dev::Jfet(_) => Some(vec![-1.0, -1.0]),
            Dev::Mos(m) => Some(vec![m.von, 0.0, -1.0, -1.0]),
            _ => None,
        }
    }

    /// (controls, static outputs, charge outputs).
    pub fn shape(&self) -> (usize, usize, usize) {
        match self {
            Dev::Cond(_) => (1, 1, 0),
            Dev::Bjt(_) => (2, 2, 2),
            Dev::Diode(_) => (1, 1, 1),
            Dev::Jfet(_) => (2, 3, 2),
            Dev::OpIn(_) => (1, 1, 0),
            Dev::Clamp(_) => (2, 1, 0),
            Dev::Schmitt(_) => (4, 3, 0),
            Dev::Cmp(_) => (5, 2, 0),
            Dev::Ota(_) => (2, 2, 0),
            Dev::Mos(_) => (4, 3, 0),
        }
    }

    /// The outputs (static currents, then charges) at the controls `v`, and their Jacobian,
    /// row-major (outputs x controls).
    #[inline]
    pub fn eval(&self, v: &[f64], out: &mut [f64], jac: &mut [f64]) {
        match self {
            Dev::Cond(g) => {
                out[0] = g * v[0];
                jac[0] = *g;
            }
            Dev::Bjt(p) => p.eval(v, out, jac),
            Dev::Diode(p) => p.eval(v, out, jac),
            Dev::Jfet(p) => p.eval(v, out, jac),
            Dev::Mos(p) => p.eval(v, out, jac),
            Dev::OpIn(p) => {
                let x = p.gm * v[0] / p.imax;
                let t = libm::tanh(x);
                out[0] = p.imax * t;
                jac[0] = p.gm * (1.0 - t * t);
            }
            Dev::Clamp(p) => {
                out[0] = p.g * (sim::softplus(v[0], p.knee) - sim::softplus(v[1], p.knee));
                jac[0] = p.g * sim::softplus_d(v[0], p.knee);
                jac[1] = -p.g * sim::softplus_d(v[1], p.knee);
            }
            Dev::Schmitt(p) => {
                let (vin, vdd, s, vo) = (v[0], v[1], v[2], v[3]);
                // th = vss + (vdd - vss) (vtp - (vtp - vtn) s), relative to vss.
                let k = p.vtp - (p.vtp - p.vtn) * s;
                let th = vdd * k;
                let t = libm::tanh((vin - th) / p.w);
                let target = 0.5 * (1.0 + t);
                let dt = 0.5 * (1.0 - t * t) / p.w;
                out[0] = target / p.r;
                jac[0] = dt / p.r;
                jac[1] = -dt * k / p.r;
                jac[2] = dt * vdd * (p.vtp - p.vtn) / p.r;
                jac[3] = 0.0;
                // From VDD into the output: (1 - s)(vdd - vo) / rout.
                out[1] = (1.0 - s) * (vdd - vo) / p.rout;
                jac[4] = 0.0;
                jac[5] = (1.0 - s) / p.rout;
                jac[6] = -(vdd - vo) / p.rout;
                jac[7] = -(1.0 - s) / p.rout;
                // From the output into VSS: s vo / rout.
                out[2] = s * vo / p.rout;
                jac[8] = 0.0;
                jac[9] = 0.0;
                jac[10] = vo / p.rout;
                jac[11] = s / p.rout;
            }
            Dev::Cmp(p) => {
                let (dnp, pc, nc, s, vo) = (v[0], v[1], v[2], v[3], v[4]);
                let t1 = libm::tanh(dnp / p.w);
                let a = 0.5 * (1.0 + t1);
                // min(v(p), v(n)) - v(vcc) + vcm.
                let (m, mp) = if pc <= nc { (pc, true) } else { (nc, false) };
                let t2 = libm::tanh((m + p.vcm) / p.wcm);
                let b = 0.5 * (1.0 + t2);
                for x in jac[..10].iter_mut() {
                    *x = 0.0;
                }
                if a >= b {
                    out[0] = a / p.r;
                    jac[0] = 0.5 * (1.0 - t1 * t1) / p.w / p.r;
                } else {
                    out[0] = b / p.r;
                    let d = 0.5 * (1.0 - t2 * t2) / p.wcm / p.r;
                    if mp {
                        jac[1] = d;
                    } else {
                        jac[2] = d;
                    }
                }
                let to = libm::tanh(vo / p.vsat);
                out[1] = p.isink * s * to;
                jac[8] = p.isink * to;
                jac[9] = p.isink * s * (1.0 - to * to) / p.vsat;
            }
            Dev::Ota(p) => {
                let mut o = [0.0; 2];
                let mut j = [0.0; 2];
                p.diode.eval(&v[..1], &mut o, &mut j);
                let (id, gd) = (o[0], j[0]);
                let t = libm::tanh(p.k * v[1]);
                out[0] = id;
                out[1] = id * t;
                jac[0] = gd;
                jac[1] = 0.0;
                jac[2] = gd * t;
                jac[3] = id * p.k * (1.0 - t * t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::fixtures as dv;

    /// Every Jacobian against central differences.
    fn check(dev: Dev, points: &[Vec<f64>]) {
        let (nv, ns, nq) = dev.shape();
        let no = ns + nq;
        for v in points {
            let mut out = vec![0.0; no];
            let mut jac = vec![0.0; no * nv];
            dev.eval(v, &mut out, &mut jac);
            for c in 0..nv {
                let h = 1e-7 * (1.0 + v[c].abs());
                let mut vp = v.clone();
                let mut vm = v.clone();
                vp[c] += h;
                vm[c] -= h;
                let mut op = vec![0.0; no];
                let mut om = vec![0.0; no];
                let mut scratch = vec![0.0; no * nv];
                dev.eval(&vp, &mut op, &mut scratch);
                dev.eval(&vm, &mut om, &mut scratch);
                for o in 0..no {
                    let num = (op[o] - om[o]) / (2.0 * h);
                    let ana = jac[o * nv + c];
                    // Relative, plus what rounding the outputs leaves a difference over 2h.
                    let tol = 1e-5 * (num.abs() + ana.abs())
                        + 1e-18
                        + 1e-14 * (op[o].abs() + om[o].abs()) / h;
                    assert!(
                        (num - ana).abs() <= tol,
                        "{dev:?} at {v:?}: d out[{o}] / d v[{c}] = {ana:e}, numerically {num:e}"
                    );
                }
            }
        }
    }

    #[test]
    fn jacobians_match_differences() {
        let pts2: Vec<Vec<f64>> = [
            [0.65, -5.0],
            [0.7, 0.6],
            [-2.0, -10.0],
            [0.3, 0.1],
            [-0.05, -0.2],
            [0.75, 0.2],
        ]
        .iter()
        .map(|p| p.to_vec())
        .collect();
        check(Dev::Bjt(BjtP::new(&dv::Q2SC945)), &pts2);
        check(Dev::Bjt(BjtP::new(&dv::Q2SA733)), &pts2);
        let pts1: Vec<Vec<f64>> = [0.6, 0.2, -0.01, -1.0, -6.6, -8.0, 0.75]
            .iter()
            .map(|p| vec![*p])
            .collect();
        check(Dev::Diode(DiodeP::new(&dv::D1S2473, 1e-5)), &pts1);
        let d = &dv::Q2SC828;
        check(
            Dev::Diode(DiodeP::from_parts(
                d.is, 1.0, d.cje, d.vje, d.mje, 0.0, d.bv_eb, 1e-5, 1e-5,
            )),
            &pts1,
        );
        let ptsj: Vec<Vec<f64>> = [
            [-0.5, -3.0],
            [-0.5, 0.2],
            [0.1, -0.1],
            [-2.0, -4.0],
            [-0.2, -0.1],
            [-0.1, -0.2],
        ]
        .iter()
        .map(|p| p.to_vec())
        .collect();
        check(Dev::Jfet(JfetP::new(&dv::J2SK30AY)), &ptsj);
        check(
            Dev::OpIn(OpIn::new(&dv::OA4558)),
            &[vec![1e-6], vec![0.03], vec![-0.2]],
        );
        check(
            Dev::Schmitt(SchmittP::new(&dv::HD14584)),
            &[
                vec![2.0, 4.8, 0.1, 4.0],
                vec![2.58, 4.8, 0.4, 2.0],
                vec![2.7, 4.7, 0.9, 0.3],
            ],
        );
        check(
            Dev::Cmp(CmpP::new(&dv::AN6912)),
            &[
                vec![0.0005, -3.0, -3.0005, 0.5, 0.3],
                vec![-0.002, -0.5, -0.502, 0.02, 4.0],
                vec![0.001, -1.48, -1.479, 0.9, 0.1],
            ],
        );
        check(
            Dev::Ota(OtaP::new(&dv::BA662)),
            &[vec![0.6, 0.003], vec![0.55, -0.05], vec![0.2, 0.0]],
        );
    }

    #[test]
    fn breakdown_matches_ibv() {
        let d = &dv::Q2SC828;
        let p = DiodeP::from_parts(d.is, 1.0, d.cje, d.vje, d.mje, 0.0, d.bv_eb, 1e-5, 1e-5);
        // At -BV the reverse current is IBV (ngspice's matching), within its tolerance.
        let mut o = [0.0; 2];
        let mut j = [0.0; 2];
        p.eval(&[-d.bv_eb], &mut o, &mut j);
        assert!((o[0] + 1e-5).abs() < 2e-9, "{}", o[0]);
        assert!(p.breakdown() > 6.4 && p.breakdown() < 6.6);
    }

    /// Two cards for the MOS device's tests (a CMOS gate's pair, as an instrument would write
    /// them): N- and P-channel, thresholds 1.5 V, KP W / L 0.36 mA/V^2.
    const NMOS: Mos = Mos {
        name: "TEST_N",
        pmos: false,
        vto: 1.5,
        beta: 0.36e-3,
        lambda: 0.01,
        is: 1e-14,
    };
    const PMOS: Mos = Mos {
        name: "TEST_P",
        pmos: true,
        vto: -1.5,
        ..NMOS
    };

    #[test]
    fn the_mos_devices_jacobians_match_differences() {
        // [vgs, vds, vbs, vbd]: cut off, linear, saturated, inverse mode, the bulk junctions
        // reverse and forward (away from ngspice's -3 vt switch, where the current is not
        // continuous).
        let pts: Vec<Vec<f64>> = [
            [0.5, 5.0, 0.0, -5.0],
            [5.0, 1.0, -1.0, -2.0],
            [5.0, 8.0, -0.5, -8.5],
            [5.0, -2.0, -3.0, -1.0],
            [3.0, -6.0, 0.6, 6.6],
            [-5.0, -1.0, 0.65, 1.65],
        ]
        .iter()
        .map(|p| p.to_vec())
        .collect();
        check(Dev::Mos(MosP::new(&NMOS)), &pts);
        // In its own polarity a P-channel device is the N-channel one: the network flips its
        // controls and currents (`net.rs`), so that Newton's junction limiting sees a junction's
        // forward direction as positive whatever the type.
        assert_eq!(MosP::new(&PMOS), MosP::new(&NMOS));
        // ngspice caps the junctions' exponent: a 20 V forward excursion stays finite.
        let (mut out, mut jac) = ([0.0; 3], [0.0; 12]);
        Dev::Mos(MosP::new(&NMOS)).eval(&[5.0, 1.0, 20.0, 19.0], &mut out, &mut jac);
        assert!(
            out.iter().chain(&jac).all(|x| x.is_finite()),
            "{out:?} {jac:?}"
        );
    }

    /// ngspice 47's terminal currents (into the device: drain, source, bulk) for the two cards
    /// above as the MS-20's CD4007 writes them (`LEVEL=1 VTO KP LAMBDA TOX=100n`, W = L = 1u),
    /// each held at (vd, vg, vs, vb) by voltage sources (the P-channel's negated), from an
    /// operating point (omarchy, ngspice 47, 2026-11-07; the deck in K-CIRCUIT-MS20): cut off,
    /// linear, saturated, inverse linear, inverse saturated, the bulk-source junction forward,
    /// the drain below the source, and deep in the linear region.
    const NGSPICE_MOS: [(bool, [f64; 4], [f64; 3]); 16] = [
        (false, [5.0, 0.5, 0.0, 0.0], [5.01e-12, -0.0, -5.01e-12]),
        (true, [5.0, 0.5, 0.0, 0.0], [-5.01e-12, -0.0, 5.01e-12]),
        (
            false,
            [1.0, 5.0, 0.0, -1.0],
            [0.00109080000201, -0.00109079999899, -3.02e-12],
        ),
        (
            true,
            [1.0, 5.0, 0.0, -1.0],
            [-0.00109080000201, 0.00109079999899, 3.02e-12],
        ),
        (
            false,
            [8.0, 5.0, 0.0, -0.5],
            [0.00238140000851, -0.002381399999490001, -9.02e-12],
        ),
        (
            true,
            [8.0, 5.0, 0.0, -0.5],
            [-0.002381400008510001, 0.00238139999949, 9.02e-12],
        ),
        (
            false,
            [0.0, 5.0, 2.0, 0.0],
            [-0.001836, 0.00183600000201, -2.01e-12],
        ),
        (
            true,
            [0.0, 5.0, 2.0, 0.0],
            [0.001836, -0.00183600000201, 2.01e-12],
        ),
        (
            false,
            [0.0, 4.0, 10.0, 0.0],
            [-0.0012375, 0.00123750001001, -1.001e-11],
        ),
        (
            true,
            [0.0, 4.0, 10.0, 0.0],
            [0.0012375, -0.00123750001001, 1.001e-11],
        ),
        (
            false,
            [3.0, 0.0, 0.0, 0.6],
            [2.41e-12, -0.0001387083995516999, 0.000138708397141701],
        ),
        (
            true,
            [3.0, 0.0, 0.0, 0.6],
            [-2.41e-12, 0.0001387083995517, -0.0001387083971417006],
        ),
        (
            false,
            [-0.5, 5.0, 0.0, 0.0],
            [-0.0006812047780550575, 6.783750e-04, 2.82977805505759e-06],
        ),
        (
            true,
            [-0.5, 5.0, 0.0, 0.0],
            [
                0.000681204778055057,
                -0.0006783749999999999,
                -2.829778055057586e-06,
            ],
        ),
        (
            false,
            [12.0, 15.0, 0.0, 0.0],
            [0.03628800001201, -0.036288, -1.201e-11],
        ),
        (
            true,
            [12.0, 15.0, 0.0, 0.0],
            [-0.03628800001201, 0.036288, 1.201e-11],
        ),
    ];

    #[test]
    fn the_mos_devices_currents_are_ngspices() {
        for (pmos, p, want) in NGSPICE_MOS {
            let card = if pmos { PMOS } else { NMOS };
            let t = card.type_sign();
            // The terminals' voltages as the deck set them, then the device's own polarity.
            let [vd, vg, vs, vb] = p.map(|x| t * x);
            let v = [t * (vg - vs), t * (vd - vs), t * (vb - vs), t * (vb - vd)];
            let (mut out, mut jac) = ([0.0; 3], [0.0; 12]);
            Dev::Mos(MosP::new(&card)).eval(&v, &mut out, &mut jac);
            let [id, ibs, ibd] = out;
            // Into each terminal: the channel's current enters the drain and leaves the source;
            // each junction's current leaves the bulk.
            let got = [t * (id - ibd), t * (-id - ibs), t * (ibs + ibd)];
            for k in 0..3 {
                let tol = 1e-12 * want[k].abs() + 1e-24;
                assert!(
                    (got[k] - want[k]).abs() <= tol,
                    "{} at {p:?}, terminal {k}: {:e} A, ngspice {:e} A",
                    if pmos { "P" } else { "N" },
                    got[k],
                    want[k]
                );
            }
        }
    }

    #[test]
    fn the_jfets_built_in_potential_and_the_op_amps_slew_load_are_the_cards() {
        let pts: Vec<Vec<f64>> = [[-1.0, -6.0], [0.2, -0.5], [-0.3, 0.1]]
            .iter()
            .map(|p| p.to_vec())
            .collect();
        let j = Jfet {
            pb: 2.0,
            ..dv::J2SK30AY
        };
        check(Dev::Jfet(JfetP::new(&j)), &pts);
        // The gate's depletion charge follows PB: a reverse-biased junction holds less charge
        // per volt with the larger potential.
        let (mut o1, mut o2) = ([0.0; 5], [0.0; 5]);
        let mut jac = [0.0; 10];
        Dev::Jfet(JfetP::new(&dv::J2SK30AY)).eval(&[-5.0, -5.0], &mut o1, &mut jac);
        let c1 = jac[6];
        Dev::Jfet(JfetP::new(&j)).eval(&[-5.0, -5.0], &mut o2, &mut jac);
        let c2 = jac[6];
        let expected = ((1.0_f64 + 5.0 / 1.0) / (1.0 + 5.0 / 2.0)).sqrt();
        assert!((c2 / c1 / expected - 1.0).abs() < 1e-12, "{c1:e} {c2:e}");
        // The op-amp's slew load raises the internal current limit by 1 + rout / sr_load.
        let a = OpIn::new(&dv::OA4558);
        let loaded = OpIn::new(&OpAmp {
            sr_load: 2e3,
            ..dv::OA4558
        });
        let k = 1.0 + dv::OA4558.rout / 2e3;
        assert!((loaded.imax / a.imax / k - 1.0).abs() < 1e-15);
    }
}
