//! Model reduction: a [`System`] cut to the states that matter for its inputs and outputs,
//! by balanced truncation (the square-root method), with the error bounded by twice the sum of
//! the dropped Hankel singular values.
//!
//! The inputs and outputs are weighed first, so that the bound means something: each input by
//! the largest it may be (a logic source's swing, a device output's current), each output by
//! the error it may carry (a microvolt). The constant input (the rails) is left out: the
//! reduced system runs in departures from the operating point, where it is exactly at rest.

use crate::la::{self, Mat};
use crate::ss::System;

/// What a reduction kept and what it dropped, and its projections: the reduced state is
/// `ti` times the full one, the full one about `t` times the reduced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Reduction {
    pub t: Mat,
    pub ti: Mat,
    /// The Hankel singular values, descending.
    pub hsv: Vec<f64>,
    /// The order kept.
    pub order: usize,
    /// Twice the sum of the dropped values: the weighted error's bound.
    pub bound: f64,
}

/// The weights of a system's inputs (the largest each may be) and outputs (the error each may
/// carry): `[sources, static remainders, charge currents]` and `[controls, probes]`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Weights {
    pub inputs: Vec<f64>,
    pub outputs: Vec<f64>,
}

impl Weights {
    /// Physical defaults: each logic source its swing (15 V), the noise voltage 10 mV, each
    /// device current 10 mA (the op-amp's input stage its slew limit), each charge current
    /// 1 mA; every output 1 uV.
    pub fn default_for(sys: &System) -> Weights {
        let mut inputs = vec![0.0; sys.nw()];
        for (k, name) in sys.input_names.iter().enumerate() {
            inputs[k] = match name.as_str() {
                "1" => 0.0,
                "noise" => 0.01,
                _ => 15.0,
            };
        }
        for s in &sys.slots {
            for o in 0..s.ns {
                inputs[s.s_off + o] = match s.dev {
                    crate::dev::Dev::OpIn(p) => p.imax,
                    _ => 0.01,
                };
            }
            for q in 0..s.nq {
                inputs[s.q_off + q] = 1e-3;
            }
        }
        Weights {
            inputs,
            outputs: vec![1e-6; sys.cy.rows],
        }
    }
}

/// The system cut to the order whose bound is at most `tol` (in the weighted units: 1 means
/// an output's error reaches its weight for an input at its weight).
pub fn reduce(sys: &System, wts: &Weights, tol: f64) -> (System, Reduction) {
    reduce_to(sys, wts, tol, None)
}

/// As [`reduce`], or to exactly `order` states when it is given.
pub fn reduce_to(
    sys: &System,
    wts: &Weights,
    tol: f64,
    order_wanted: Option<usize>,
) -> (System, Reduction) {
    let n = sys.a.rows;
    let nw = sys.nw();
    let no = sys.cy.rows;
    let mut bs = sys.b.clone();
    for i in 0..n {
        for k in 0..nw {
            bs[(i, k)] *= wts.inputs[k];
        }
    }
    let mut cs = sys.cy.clone();
    for o in 0..no {
        for i in 0..n {
            cs[(o, i)] /= wts.outputs[o];
        }
    }
    let sch = la::schur(&sys.a);
    let p = la::lyap(&sch, &bs.mul(&bs.t()));
    let scht = la::schur(&sys.a.t());
    let q = la::lyap(&scht, &cs.t().mul(&cs));
    let lp = factor(&p);
    let lq = factor(&q);
    let m = lq.t().mul(&lp);
    let (w, s, v) = la::svd(&m);
    let total: f64 = s.iter().sum();
    let mut order = s.len();
    let mut tail = 0.0;
    for k in (0..s.len()).rev() {
        if 2.0 * (tail + s[k]) > tol {
            break;
        }
        tail += s[k];
        order = k;
    }
    let _ = total;
    if let Some(k) = order_wanted {
        order = k.min(s.len());
        tail = s[order..].iter().sum();
    }
    // T = Lp V_r Σ_r^{-1/2}, Ti = Σ_r^{-1/2} W_rᵀ Lqᵀ.
    let mut t = Mat::zeros(n, order);
    let mut ti = Mat::zeros(order, n);
    let lpv = lp.mul(&v);
    let lqw = lq.mul(&w);
    for k in 0..order {
        let f = 1.0 / s[k].sqrt();
        for i in 0..n {
            t[(i, k)] = lpv[(i, k)] * f;
            ti[(k, i)] = lqw[(i, k)] * f;
        }
    }
    let red = System {
        a: ti.mul(&sys.a).mul(&t),
        b: ti.mul(&sys.b),
        cy: sys.cy.mul(&t),
        d: sys.d.clone(),
        xt: ti.mul(&sys.xt),
        ..sys.clone()
    };
    (
        red,
        Reduction {
            t,
            ti,
            hsv: s,
            order,
            bound: 2.0 * tail,
        },
    )
}

/// A balanced reduction with the discarded states at their static solution. In balanced
/// coordinates, x₂ = -A₂₂⁻¹(A₂₁x₁ + B₂w). Substitution into the retained dynamics and
/// outputs preserves the DC transfer, including the devices' feedback paths. Ordinary
/// truncation leaves those paths short of the discarded states' gain, which can destabilise
/// a nonlinear network when its pots move. The caller must fit its transient error offline.
pub fn reduce_dc(sys: &System, wts: &Weights, order: usize) -> System {
    assert!(order > 0, "a reduction keeps at least one state");
    let (balanced, _) = reduce_to(sys, wts, 1.0, Some(usize::MAX));
    if order >= balanced.a.rows {
        return balanced;
    }
    let keep: Vec<_> = (0..order).collect();
    let drop: Vec<_> = (order..balanced.a.rows).collect();
    let a12 = balanced.a.rows_of(&keep).cols_of(&drop);
    let c2 = balanced.cy.cols_of(&drop);
    let lu = la::Lu::new(&balanced.a.rows_of(&drop).cols_of(&drop));
    assert!(
        lu.rcond.is_finite() && lu.rcond > 0.0,
        "singular discarded block"
    );
    let a21 = lu.solve_mat(&balanced.a.rows_of(&drop).cols_of(&keep));
    let b2 = lu.solve_mat(&balanced.b.rows_of(&drop));
    System {
        a: balanced.a.rows_of(&keep).cols_of(&keep).sub(&a12.mul(&a21)),
        b: balanced.b.rows_of(&keep).sub(&a12.mul(&b2)),
        cy: balanced.cy.cols_of(&keep).sub(&c2.mul(&a21)),
        d: balanced.d.sub(&c2.mul(&b2)),
        xt: balanced.xt.rows_of(&keep),
        ..balanced
    }
}

/// A factor `L` with `L Lᵀ = X` for a symmetric positive semidefinite `X` (its eigenvectors
/// scaled by the square roots of its eigenvalues, the negligible ones dropped).
fn factor(x: &Mat) -> Mat {
    let (lam, u) = la::sym_eig(x);
    let top = lam.first().copied().unwrap_or(0.0).max(0.0);
    let keep: Vec<usize> = (0..lam.len())
        .filter(|&k| lam[k] > 1e-30 * top && lam[k] > 0.0)
        .collect();
    let mut l = Mat::zeros(x.rows, keep.len());
    for (c, &k) in keep.iter().enumerate() {
        let f = lam[k].sqrt();
        for i in 0..x.rows {
            l[(i, c)] = u[(i, k)] * f;
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarded_states_keep_their_dc_transfer() {
        let mat = |rows, cols, a: &[f64]| Mat {
            rows,
            cols,
            a: a.to_vec(),
        };
        let sys = System {
            a: mat(2, 2, &[-2.0, 0.5, 0.3, -1000.0]),
            b: mat(2, 1, &[1.0, 3.0]),
            cy: mat(1, 2, &[4.0, 5.0]),
            d: mat(1, 1, &[0.1]),
            xt: Mat::eye(2),
            nu: 1,
            ns: 0,
            nq: 0,
            nv: 0,
            probes: vec!["output".into()],
            slots: Vec::new(),
            u0: vec![0.0],
            y0: vec![0.0],
            input_names: vec!["signal".into()],
        };
        let weights = Weights {
            inputs: vec![1.0],
            outputs: vec![1.0],
        };
        let gain = |s: &System| s.d.sub(&s.cy.mul(&la::Lu::new(&s.a).solve_mat(&s.b)))[(0, 0)];
        let reduced = reduce_dc(&sys, &weights, 1);
        let truncated = reduce_to(&sys, &weights, 1.0, Some(1)).0;
        assert_eq!(reduced.a.rows, 1);
        assert!(reduced.a[(0, 0)] < 0.0, "still stable");
        assert!((gain(&reduced) - gain(&sys)).abs() < 1e-10);
        assert!((gain(&truncated) - gain(&sys)).abs() > 1e-5);
    }
}
