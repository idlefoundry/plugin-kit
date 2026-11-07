//! Dense linear algebra for building the models (never on the audio thread): matrices, LU
//! with partial pivoting, the symmetric eigenproblem (cyclic Jacobi), the nonsymmetric one
//! (balancing, Householder reduction to Hessenberg form and the shifted QR algorithm with its
//! eigenvectors: EISPACK's `orthes` and `hqr2` as the public-domain JAMA library gives them)
//! and a little complex arithmetic.

use std::ops::{Add, Div, Index, IndexMut, Mul, Neg, Sub};

/// A dense row-major matrix.
#[derive(Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub a: Vec<f64>,
}

impl std::fmt::Debug for Mat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Mat {}x{}", self.rows, self.cols)?;
        for i in 0..self.rows.min(12) {
            for j in 0..self.cols.min(12) {
                write!(f, "{:11.3e} ", self[(i, j)])?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl Index<(usize, usize)> for Mat {
    type Output = f64;
    #[inline]
    fn index(&self, (i, j): (usize, usize)) -> &f64 {
        &self.a[i * self.cols + j]
    }
}

impl IndexMut<(usize, usize)> for Mat {
    #[inline]
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut f64 {
        &mut self.a[i * self.cols + j]
    }
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Mat {
        Mat {
            rows,
            cols,
            a: vec![0.0; rows * cols],
        }
    }

    pub fn eye(n: usize) -> Mat {
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            m[(i, i)] = 1.0;
        }
        m
    }

    pub fn row(&self, i: usize) -> &[f64] {
        &self.a[i * self.cols..(i + 1) * self.cols]
    }

    pub fn t(&self) -> Mat {
        let mut m = Mat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                m[(j, i)] = self[(i, j)];
            }
        }
        m
    }

    pub fn mul(&self, b: &Mat) -> Mat {
        assert_eq!(
            self.cols, b.rows,
            "mul: {}x{} by {}x{}",
            self.rows, self.cols, b.rows, b.cols
        );
        let mut c = Mat::zeros(self.rows, b.cols);
        for i in 0..self.rows {
            let ci = &mut c.a[i * b.cols..(i + 1) * b.cols];
            for k in 0..self.cols {
                let x = self.a[i * self.cols + k];
                if x == 0.0 {
                    continue;
                }
                let bk = &b.a[k * b.cols..(k + 1) * b.cols];
                for (c, b) in ci.iter_mut().zip(bk) {
                    *c += x * b;
                }
            }
        }
        c
    }

    pub fn sub(&self, b: &Mat) -> Mat {
        assert_eq!((self.rows, self.cols), (b.rows, b.cols));
        Mat {
            rows: self.rows,
            cols: self.cols,
            a: self.a.iter().zip(&b.a).map(|(x, y)| x - y).collect(),
        }
    }

    pub fn add(&self, b: &Mat) -> Mat {
        assert_eq!((self.rows, self.cols), (b.rows, b.cols));
        Mat {
            rows: self.rows,
            cols: self.cols,
            a: self.a.iter().zip(&b.a).map(|(x, y)| x + y).collect(),
        }
    }

    pub fn scale(&self, s: f64) -> Mat {
        Mat {
            rows: self.rows,
            cols: self.cols,
            a: self.a.iter().map(|x| x * s).collect(),
        }
    }

    /// The columns `cols` of the matrix.
    pub fn cols_of(&self, cols: &[usize]) -> Mat {
        let mut m = Mat::zeros(self.rows, cols.len());
        for i in 0..self.rows {
            for (k, &j) in cols.iter().enumerate() {
                m[(i, k)] = self[(i, j)];
            }
        }
        m
    }

    /// The rows `rows` of the matrix.
    pub fn rows_of(&self, rows: &[usize]) -> Mat {
        let mut m = Mat::zeros(rows.len(), self.cols);
        for (k, &i) in rows.iter().enumerate() {
            m.a[k * self.cols..(k + 1) * self.cols].copy_from_slice(self.row(i));
        }
        m
    }

    /// Side by side: `[self b]`.
    pub fn hcat(&self, b: &Mat) -> Mat {
        assert_eq!(self.rows, b.rows);
        let mut m = Mat::zeros(self.rows, self.cols + b.cols);
        for i in 0..self.rows {
            for j in 0..self.cols {
                m[(i, j)] = self[(i, j)];
            }
            for j in 0..b.cols {
                m[(i, self.cols + j)] = b[(i, j)];
            }
        }
        m
    }

    pub fn max_abs(&self) -> f64 {
        self.a.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    pub fn mul_vec(&self, x: &[f64]) -> Vec<f64> {
        assert_eq!(self.cols, x.len());
        (0..self.rows)
            .map(|i| self.row(i).iter().zip(x).map(|(a, b)| a * b).sum())
            .collect()
    }
}

/// An LU factorisation with partial pivoting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lu {
    n: usize,
    lu: Vec<f64>,
    piv: Vec<usize>,
    /// The smallest pivot's magnitude against the largest: near 0, the matrix is singular.
    pub rcond: f64,
}

impl Lu {
    pub fn new(m: &Mat) -> Lu {
        assert_eq!(m.rows, m.cols, "lu: not square");
        let n = m.rows;
        let mut lu = m.a.clone();
        let mut piv: Vec<usize> = (0..n).collect();
        let (mut pmin, mut pmax) = (f64::INFINITY, 0.0f64);
        for k in 0..n {
            let mut p = k;
            let mut best = lu[k * n + k].abs();
            for i in k + 1..n {
                let v = lu[i * n + k].abs();
                if v > best {
                    best = v;
                    p = i;
                }
            }
            if p != k {
                for j in 0..n {
                    lu.swap(k * n + j, p * n + j);
                }
                piv.swap(k, p);
            }
            let d = lu[k * n + k];
            pmin = pmin.min(d.abs());
            pmax = pmax.max(d.abs());
            if d == 0.0 {
                continue;
            }
            for i in k + 1..n {
                let f = lu[i * n + k] / d;
                lu[i * n + k] = f;
                if f != 0.0 {
                    for j in k + 1..n {
                        lu[i * n + j] -= f * lu[k * n + j];
                    }
                }
            }
        }
        Lu {
            n,
            lu,
            piv,
            rcond: if pmax > 0.0 { pmin / pmax } else { 0.0 },
        }
    }

    pub fn solve(&self, b: &[f64]) -> Vec<f64> {
        let n = self.n;
        let mut x: Vec<f64> = self.piv.iter().map(|&p| b[p]).collect();
        for i in 0..n {
            let mut s = x[i];
            for j in 0..i {
                s -= self.lu[i * n + j] * x[j];
            }
            x[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for j in i + 1..n {
                s -= self.lu[i * n + j] * x[j];
            }
            x[i] = s / self.lu[i * n + i];
        }
        x
    }

    /// `self⁻¹ b` for a matrix `b`.
    pub fn solve_mat(&self, b: &Mat) -> Mat {
        assert_eq!(b.rows, self.n);
        let mut out = Mat::zeros(b.rows, b.cols);
        let mut col = vec![0.0; b.rows];
        for j in 0..b.cols {
            for i in 0..b.rows {
                col[i] = b[(i, j)];
            }
            let x = self.solve(&col);
            for i in 0..b.rows {
                out[(i, j)] = x[i];
            }
        }
        out
    }

    pub fn inverse(&self) -> Mat {
        self.solve_mat(&Mat::eye(self.n))
    }
}

/// Solves `a x = b` in place for a small dense `n x n` system (`a` row-major, overwritten by
/// its LU factors; `b` by the solution), with partial pivoting and no allocation. Returns
/// `false` if a pivot is zero.
#[inline]
pub fn solve_in_place(a: &mut [f64], n: usize, b: &mut [f64]) -> bool {
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
        if p != k {
            for j in 0..n {
                a.swap(k * n + j, p * n + j);
            }
            b.swap(k, p);
        }
        let d = a[k * n + k];
        for i in k + 1..n {
            let f = a[i * n + k] / d;
            if f != 0.0 {
                for j in k + 1..n {
                    a[i * n + j] -= f * a[k * n + j];
                }
                b[i] -= f * b[k];
            }
        }
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for j in i + 1..n {
            s -= a[i * n + j] * b[j];
        }
        b[i] = s / a[i * n + i];
    }
    true
}

/// The eigenvalues and orthonormal eigenvectors (columns) of a symmetric matrix, by cyclic
/// Jacobi rotations; eigenvalues in descending order.
pub fn sym_eig(m: &Mat) -> (Vec<f64>, Mat) {
    let n = m.rows;
    assert_eq!(n, m.cols);
    let mut a = m.clone();
    let mut v = Mat::eye(n);
    for _sweep in 0..100 {
        let mut off = 0.0;
        let mut diag = 0.0;
        for i in 0..n {
            diag += a[(i, i)] * a[(i, i)];
            for j in i + 1..n {
                off += a[(i, j)] * a[(i, j)];
            }
        }
        if off <= 1e-34 * diag || off == 0.0 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[(p, q)];
                if apq == 0.0 {
                    continue;
                }
                let app = a[(p, p)];
                let aqq = a[(q, q)];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = a[(k, p)];
                    let akq = a[(k, q)];
                    a[(k, p)] = c * akp - s * akq;
                    a[(k, q)] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[(p, k)];
                    let aqk = a[(q, k)];
                    a[(p, k)] = c * apk - s * aqk;
                    a[(q, k)] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[(k, p)];
                    let vkq = v[(k, q)];
                    v[(k, p)] = c * vkp - s * vkq;
                    v[(k, q)] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| a[(j, j)].total_cmp(&a[(i, i)]));
    let w = idx.iter().map(|&i| a[(i, i)]).collect();
    (w, v.cols_of(&idx))
}

/// A complex number.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

impl C64 {
    pub const ZERO: C64 = C64 { re: 0.0, im: 0.0 };
    pub const ONE: C64 = C64 { re: 1.0, im: 0.0 };
    pub const fn new(re: f64, im: f64) -> C64 {
        C64 { re, im }
    }
    pub fn exp(self) -> C64 {
        let m = libm::exp(self.re);
        C64::new(m * libm::cos(self.im), m * libm::sin(self.im))
    }
    pub fn abs(self) -> f64 {
        libm::hypot(self.re, self.im)
    }
    pub fn conj(self) -> C64 {
        C64::new(self.re, -self.im)
    }
    pub fn scale(self, s: f64) -> C64 {
        C64::new(self.re * s, self.im * s)
    }
}

impl Add for C64 {
    type Output = C64;
    fn add(self, b: C64) -> C64 {
        C64::new(self.re + b.re, self.im + b.im)
    }
}
impl Sub for C64 {
    type Output = C64;
    fn sub(self, b: C64) -> C64 {
        C64::new(self.re - b.re, self.im - b.im)
    }
}
impl Mul for C64 {
    type Output = C64;
    fn mul(self, b: C64) -> C64 {
        C64::new(
            self.re * b.re - self.im * b.im,
            self.re * b.im + self.im * b.re,
        )
    }
}
impl Div for C64 {
    type Output = C64;
    fn div(self, b: C64) -> C64 {
        if b.re.abs() >= b.im.abs() {
            let r = b.im / b.re;
            let d = b.re + r * b.im;
            C64::new((self.re + r * self.im) / d, (self.im - r * self.re) / d)
        } else {
            let r = b.re / b.im;
            let d = b.im + r * b.re;
            C64::new((r * self.re + self.im) / d, (r * self.im - self.re) / d)
        }
    }
}
impl Neg for C64 {
    type Output = C64;
    fn neg(self) -> C64 {
        C64::new(-self.re, -self.im)
    }
}

/// A real matrix's eigen-decomposition `A V = V D`: `d` and `e` are the eigenvalues' real and
/// imaginary parts; `D` is block diagonal (a complex pair at `i, i+1` with `e[i] > 0` is the
/// block `[[d, e], [-e, d]]`), and the columns of `V` the eigenvectors (a complex pair's real
/// and imaginary parts at `i` and `i + 1`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Eig {
    pub d: Vec<f64>,
    pub e: Vec<f64>,
    pub v: Mat,
}

/// Balances `a` by diagonal similarity with powers of two (Parlett and Reinsch's algorithm):
/// returns the scales `s`, with the balanced matrix `S⁻¹ A S`.
fn balance(a: &mut Mat) -> Vec<f64> {
    let n = a.rows;
    let mut s = vec![1.0; n];
    let radix = 2.0;
    let sqrdx = radix * radix;
    let mut done = false;
    let mut passes = 0;
    while !done && passes < 200 {
        passes += 1;
        done = true;
        for i in 0..n {
            let mut r = 0.0;
            let mut c = 0.0;
            for j in 0..n {
                if j != i {
                    c += a[(j, i)].abs();
                    r += a[(i, j)].abs();
                }
            }
            if c == 0.0 || r == 0.0 {
                continue;
            }
            let mut g = r / radix;
            let mut f = 1.0;
            let sum = c + r;
            while c < g {
                f *= radix;
                c *= sqrdx;
            }
            g = r * radix;
            while c > g {
                f /= radix;
                c /= sqrdx;
            }
            if (c + r) / f < 0.95 * sum {
                done = false;
                let gi = 1.0 / f;
                for j in 0..n {
                    a[(i, j)] *= gi;
                }
                for j in 0..n {
                    a[(j, i)] *= f;
                }
                s[i] *= f;
            }
        }
    }
    s
}

/// The eigen-decomposition of a real square matrix (balanced first).
pub fn eig(m: &Mat) -> Eig {
    let n = m.rows;
    assert_eq!(n, m.cols);
    let mut h = m.clone();
    let scales = balance(&mut h);
    let mut v = Mat::zeros(n, n);
    let mut d = vec![0.0; n];
    let mut e = vec![0.0; n];
    orthes(&mut h, &mut v);
    hqr2(&mut h, &mut v, &mut d, &mut e, true);
    // Undo the balancing: the eigenvectors of A are S times the balanced matrix's.
    for i in 0..n {
        for j in 0..n {
            v[(i, j)] *= scales[i];
        }
    }
    Eig { d, e, v }
}

/// A complex Schur form of a balanced real matrix: `S⁻¹ A S = Z T Zᴴ`, `T` upper triangular,
/// `Z` unitary, `S` diagonal (the balancing's `scales`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Schur {
    pub t: CMat,
    pub z: CMat,
    pub scales: Vec<f64>,
}

/// The complex Schur form of `m` (balanced first): the real Schur form by the QR algorithm,
/// each 2 x 2 block of a complex pair then triangularised by a unitary rotation.
pub fn schur(m: &Mat) -> Schur {
    let n = m.rows;
    assert_eq!(n, m.cols);
    let mut h = m.clone();
    let scales = balance(&mut h);
    let mut v = Mat::zeros(n, n);
    let mut d = vec![0.0; n];
    let mut e = vec![0.0; n];
    orthes(&mut h, &mut v);
    hqr2(&mut h, &mut v, &mut d, &mut e, false);
    // The quasi-triangular form: only the complex pairs' subdiagonals stay.
    for i in 1..n {
        let pair = e[i - 1] > 0.0 && e[i] < 0.0;
        if !pair {
            h[(i, i - 1)] = 0.0;
        }
        for j in 0..i.saturating_sub(1) {
            h[(i, j)] = 0.0;
        }
    }
    let mut t = CMat::from_real(&h);
    let mut z = CMat::from_real(&v);
    let mut i = 0;
    while i + 1 < n {
        if e[i] > 0.0 && e[i + 1] < 0.0 {
            // The block [[a, b], [c, d]] and its eigenvalue λ = d[i] + i e[i]: the unit
            // eigenvector (b, λ - a) (or (λ - d, c)) and its orthogonal complement.
            let a = t.get(i, i);
            let b = t.get(i, i + 1);
            let c = t.get(i + 1, i);
            let dd = t.get(i + 1, i + 1);
            let lam = C64::new(d[i], e[i]);
            let (mut x0, mut x1) = (b, lam - a);
            if x0.abs() + x1.abs() < 1e-300 {
                x0 = lam - dd;
                x1 = c;
            }
            let nrm = (x0.abs().powi(2) + x1.abs().powi(2)).sqrt();
            let (x0, x1) = (x0.scale(1.0 / nrm), x1.scale(1.0 / nrm));
            // G = [[x0, -conj(x1)], [x1, conj(x0)]]: T ← Gᴴ T G, Z ← Z G.
            let g = [[x0, -x1.conj()], [x1, x0.conj()]];
            t.rotate_cols(i, &g);
            t.rotate_rows_h(i, &g);
            z.rotate_cols(i, &g);
            t.set(i + 1, i, C64::ZERO);
            i += 2;
        } else {
            i += 1;
        }
    }
    Schur { t, z, scales }
}

/// A dense row-major complex matrix.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CMat {
    pub rows: usize,
    pub cols: usize,
    pub a: Vec<C64>,
}

impl std::fmt::Debug for CMat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CMat {}x{}", self.rows, self.cols)
    }
}

impl CMat {
    pub fn zeros(rows: usize, cols: usize) -> CMat {
        CMat {
            rows,
            cols,
            a: vec![C64::ZERO; rows * cols],
        }
    }

    pub fn from_real(m: &Mat) -> CMat {
        CMat {
            rows: m.rows,
            cols: m.cols,
            a: m.a.iter().map(|&x| C64::new(x, 0.0)).collect(),
        }
    }

    #[inline]
    pub fn get(&self, i: usize, j: usize) -> C64 {
        self.a[i * self.cols + j]
    }

    #[inline]
    pub fn set(&mut self, i: usize, j: usize, v: C64) {
        self.a[i * self.cols + j] = v;
    }

    /// Columns `i`, `i + 1` times the 2 x 2 matrix `g`.
    fn rotate_cols(&mut self, i: usize, g: &[[C64; 2]; 2]) {
        for r in 0..self.rows {
            let (p, q) = (self.get(r, i), self.get(r, i + 1));
            self.set(r, i, p * g[0][0] + q * g[1][0]);
            self.set(r, i + 1, p * g[0][1] + q * g[1][1]);
        }
    }

    /// Rows `i`, `i + 1` times `gᴴ` from the left.
    fn rotate_rows_h(&mut self, i: usize, g: &[[C64; 2]; 2]) {
        for c in 0..self.cols {
            let (p, q) = (self.get(i, c), self.get(i + 1, c));
            self.set(i, c, g[0][0].conj() * p + g[1][0].conj() * q);
            self.set(i + 1, c, g[0][1].conj() * p + g[1][1].conj() * q);
        }
    }

    pub fn mul(&self, b: &CMat) -> CMat {
        assert_eq!(self.cols, b.rows);
        let mut c = CMat::zeros(self.rows, b.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let x = self.a[i * self.cols + k];
                if x.re == 0.0 && x.im == 0.0 {
                    continue;
                }
                for j in 0..b.cols {
                    c.a[i * b.cols + j] = c.a[i * b.cols + j] + x * b.a[k * b.cols + j];
                }
            }
        }
        c
    }

    /// The conjugate transpose.
    pub fn h(&self) -> CMat {
        let mut m = CMat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                m.set(j, i, self.get(i, j).conj());
            }
        }
        m
    }

    pub fn re(&self) -> Mat {
        Mat {
            rows: self.rows,
            cols: self.cols,
            a: self.a.iter().map(|x| x.re).collect(),
        }
    }
}

/// The solution `X` of `A X + X Aᵀ + W = 0` for a stable `A` and symmetric `W`, by
/// Bartels and Stewart's method on `A`'s complex Schur form.
pub fn lyap(sch: &Schur, w: &Mat) -> Mat {
    let n = sch.t.rows;
    // In the balanced coordinates: A = S Z T Zᴴ S⁻¹, so with Y = Zᴴ S⁻¹ X S⁻¹ Z,
    // T Y + Y Tᴴ + Zᴴ S⁻¹ W S⁻¹ Z = 0.
    let mut ws = w.clone();
    for i in 0..n {
        for j in 0..n {
            ws[(i, j)] /= sch.scales[i] * sch.scales[j];
        }
    }
    let wt = sch.z.h().mul(&CMat::from_real(&ws)).mul(&sch.z);
    let t = &sch.t;
    let mut y = CMat::zeros(n, n);
    for j in (0..n).rev() {
        // (T + conj(t_jj) I) y_j = -w_j - Σ_{k > j} conj(t_jk) y_k.
        let mut rhs: Vec<C64> = (0..n).map(|i| -wt.get(i, j)).collect();
        for k in j + 1..n {
            let tjk = t.get(j, k).conj();
            if tjk.re == 0.0 && tjk.im == 0.0 {
                continue;
            }
            for (i, r) in rhs.iter_mut().enumerate() {
                *r = *r - tjk * y.get(i, k);
            }
        }
        let shift = t.get(j, j).conj();
        for i in (0..n).rev() {
            let mut s = rhs[i];
            for k in i + 1..n {
                s = s - t.get(i, k) * y.get(k, j);
            }
            y.set(i, j, s / (t.get(i, i) + shift));
        }
    }
    // X = S Z Y Zᴴ S.
    let x = sch.z.mul(&y).mul(&sch.z.h()).re();
    let mut out = x;
    for i in 0..n {
        for j in 0..n {
            out[(i, j)] *= sch.scales[i] * sch.scales[j];
        }
    }
    // Symmetrise the rounding.
    for i in 0..n {
        for j in i + 1..n {
            let m = 0.5 * (out[(i, j)] + out[(j, i)]);
            out[(i, j)] = m;
            out[(j, i)] = m;
        }
    }
    out
}

/// The singular values and vectors of `m` (`m = U diag(s) Vᵀ`, `s` descending), by one-sided
/// Jacobi rotations.
pub fn svd(m: &Mat) -> (Mat, Vec<f64>, Mat) {
    // Work on columns of a copy: A V = U Σ.
    let (r, c) = (m.rows, m.cols);
    let mut a = m.clone();
    let mut v = Mat::eye(c);
    for _sweep in 0..80 {
        let mut rot = 0.0f64;
        for p in 0..c {
            for q in p + 1..c {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for i in 0..r {
                    let (x, y) = (a[(i, p)], a[(i, q)]);
                    alpha += x * x;
                    beta += y * y;
                    gamma += x * y;
                }
                if gamma == 0.0 {
                    continue;
                }
                let conv = gamma.abs() / (alpha * beta).sqrt();
                rot = rot.max(conv);
                if conv < 1e-15 {
                    continue;
                }
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let t = if zeta == 0.0 { 1.0 } else { t };
                let cs = 1.0 / (1.0 + t * t).sqrt();
                let sn = cs * t;
                for i in 0..r {
                    let (x, y) = (a[(i, p)], a[(i, q)]);
                    a[(i, p)] = cs * x - sn * y;
                    a[(i, q)] = sn * x + cs * y;
                }
                for i in 0..c {
                    let (x, y) = (v[(i, p)], v[(i, q)]);
                    v[(i, p)] = cs * x - sn * y;
                    v[(i, q)] = sn * x + cs * y;
                }
            }
        }
        if rot < 1e-15 {
            break;
        }
    }
    let mut s: Vec<(f64, usize)> = (0..c)
        .map(|j| ((0..r).map(|i| a[(i, j)] * a[(i, j)]).sum::<f64>().sqrt(), j))
        .collect();
    s.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut u = Mat::zeros(r, c);
    let mut vv = Mat::zeros(c, c);
    let mut sv = Vec::with_capacity(c);
    for (k, &(sk, j)) in s.iter().enumerate() {
        sv.push(sk);
        for i in 0..r {
            u[(i, k)] = if sk > 0.0 { a[(i, j)] / sk } else { 0.0 };
        }
        for i in 0..c {
            vv[(i, k)] = v[(i, j)];
        }
    }
    (u, sv, vv)
}

fn orthes(h: &mut Mat, v: &mut Mat) {
    let n = h.rows;
    if n == 0 {
        return;
    }
    let low = 0;
    let high = n - 1;
    let mut ort = vec![0.0; n];
    for m in low + 1..high {
        let mut scale = 0.0;
        for i in m..=high {
            scale += h[(i, m - 1)].abs();
        }
        if scale != 0.0 {
            let mut hh = 0.0;
            for i in (m..=high).rev() {
                ort[i] = h[(i, m - 1)] / scale;
                hh += ort[i] * ort[i];
            }
            let mut g = hh.sqrt();
            if ort[m] > 0.0 {
                g = -g;
            }
            hh -= ort[m] * g;
            ort[m] -= g;
            for j in m..n {
                let mut f = 0.0;
                for i in (m..=high).rev() {
                    f += ort[i] * h[(i, j)];
                }
                f /= hh;
                for i in m..=high {
                    h[(i, j)] -= f * ort[i];
                }
            }
            for i in 0..=high {
                let mut f = 0.0;
                for j in (m..=high).rev() {
                    f += ort[j] * h[(i, j)];
                }
                f /= hh;
                for j in m..=high {
                    h[(i, j)] -= f * ort[j];
                }
            }
            ort[m] *= scale;
            h[(m, m - 1)] = scale * g;
        }
    }
    for i in 0..n {
        for j in 0..n {
            v[(i, j)] = if i == j { 1.0 } else { 0.0 };
        }
    }
    if high < 2 {
        return;
    }
    for m in (low + 1..high).rev() {
        if h[(m, m - 1)] != 0.0 {
            for i in m + 1..=high {
                ort[i] = h[(i, m - 1)];
            }
            for j in m..=high {
                let mut g = 0.0;
                for i in m..=high {
                    g += ort[i] * v[(i, j)];
                }
                g = (g / ort[m]) / h[(m, m - 1)];
                for i in m..=high {
                    v[(i, j)] += g * ort[i];
                }
            }
        }
    }
}

fn cdiv(xr: f64, xi: f64, yr: f64, yi: f64) -> (f64, f64) {
    let q = C64::new(xr, xi) / C64::new(yr, yi);
    (q.re, q.im)
}

#[allow(clippy::many_single_char_names)]
fn hqr2(h: &mut Mat, v: &mut Mat, d: &mut [f64], e: &mut [f64], vectors: bool) {
    let nn = h.rows;
    if nn == 0 {
        return;
    }
    let mut n = nn as isize - 1;
    let low: isize = 0;
    let high: isize = nn as isize - 1;
    let eps = 2f64.powi(-52);
    let mut exshift = 0.0;
    let (mut p, mut q, mut r, mut s, mut z) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut t, mut w, mut x, mut y);
    let mut norm = 0.0;
    for i in 0..nn {
        for j in i.saturating_sub(1)..nn {
            norm += h[(i, j)].abs();
        }
    }
    let mut iter = 0;
    let mut total_iter = 0usize;
    macro_rules! hh {
        ($i:expr, $j:expr) => {
            h[(($i) as usize, ($j) as usize)]
        };
    }
    macro_rules! vv {
        ($i:expr, $j:expr) => {
            v[(($i) as usize, ($j) as usize)]
        };
    }
    while n >= low {
        total_iter += 1;
        assert!(total_iter < 100 * nn + 1000, "hqr2: no convergence");
        let mut l = n;
        while l > low {
            s = hh!(l - 1, l - 1).abs() + hh!(l, l).abs();
            if s == 0.0 {
                s = norm;
            }
            if hh!(l, l - 1).abs() < eps * s {
                break;
            }
            l -= 1;
        }
        if l == n {
            hh!(n, n) += exshift;
            d[n as usize] = hh!(n, n);
            e[n as usize] = 0.0;
            n -= 1;
            iter = 0;
        } else if l == n - 1 {
            w = hh!(n, n - 1) * hh!(n - 1, n);
            p = (hh!(n - 1, n - 1) - hh!(n, n)) / 2.0;
            q = p * p + w;
            z = q.abs().sqrt();
            hh!(n, n) += exshift;
            hh!(n - 1, n - 1) += exshift;
            x = hh!(n, n);
            if q >= 0.0 {
                z = if p >= 0.0 { p + z } else { p - z };
                d[(n - 1) as usize] = x + z;
                d[n as usize] = d[(n - 1) as usize];
                if z != 0.0 {
                    d[n as usize] = x - w / z;
                }
                e[(n - 1) as usize] = 0.0;
                e[n as usize] = 0.0;
                x = hh!(n, n - 1);
                s = x.abs() + z.abs();
                p = x / s;
                q = z / s;
                r = (p * p + q * q).sqrt();
                p /= r;
                q /= r;
                for j in (n - 1)..nn as isize {
                    z = hh!(n - 1, j);
                    hh!(n - 1, j) = q * z + p * hh!(n, j);
                    hh!(n, j) = q * hh!(n, j) - p * z;
                }
                for i in 0..=n {
                    z = hh!(i, n - 1);
                    hh!(i, n - 1) = q * z + p * hh!(i, n);
                    hh!(i, n) = q * hh!(i, n) - p * z;
                }
                for i in low..=high {
                    z = vv!(i, n - 1);
                    vv!(i, n - 1) = q * z + p * vv!(i, n);
                    vv!(i, n) = q * vv!(i, n) - p * z;
                }
            } else {
                d[(n - 1) as usize] = x + p;
                d[n as usize] = x + p;
                e[(n - 1) as usize] = z;
                e[n as usize] = -z;
            }
            n -= 2;
            iter = 0;
        } else {
            x = hh!(n, n);
            y = 0.0;
            w = 0.0;
            if l < n {
                y = hh!(n - 1, n - 1);
                w = hh!(n, n - 1) * hh!(n - 1, n);
            }
            if iter == 10 {
                exshift += x;
                for i in low..=n {
                    hh!(i, i) -= x;
                }
                s = hh!(n, n - 1).abs() + hh!(n - 1, n - 2).abs();
                x = 0.75 * s;
                y = x;
                w = -0.4375 * s * s;
            }
            if iter == 30 {
                s = (y - x) / 2.0;
                s = s * s + w;
                if s > 0.0 {
                    s = s.sqrt();
                    if y < x {
                        s = -s;
                    }
                    s = x - w / ((y - x) / 2.0 + s);
                    for i in low..=n {
                        hh!(i, i) -= s;
                    }
                    exshift += s;
                    x = 0.964;
                    y = x;
                    w = x;
                }
            }
            iter += 1;
            let mut m = n - 2;
            while m >= l {
                z = hh!(m, m);
                r = x - z;
                s = y - z;
                p = (r * s - w) / hh!(m + 1, m) + hh!(m, m + 1);
                q = hh!(m + 1, m + 1) - z - r - s;
                r = hh!(m + 2, m + 1);
                s = p.abs() + q.abs() + r.abs();
                p /= s;
                q /= s;
                r /= s;
                if m == l {
                    break;
                }
                if hh!(m, m - 1).abs() * (q.abs() + r.abs())
                    < eps
                        * (p.abs() * (hh!(m - 1, m - 1).abs() + z.abs() + hh!(m + 1, m + 1).abs()))
                {
                    break;
                }
                m -= 1;
            }
            for i in (m + 2)..=n {
                hh!(i, i - 2) = 0.0;
                if i > m + 2 {
                    hh!(i, i - 3) = 0.0;
                }
            }
            let mut k = m;
            while k < n {
                let notlast = k != n - 1;
                if k != m {
                    p = hh!(k, k - 1);
                    q = hh!(k + 1, k - 1);
                    r = if notlast { hh!(k + 2, k - 1) } else { 0.0 };
                    x = p.abs() + q.abs() + r.abs();
                    if x == 0.0 {
                        k += 1;
                        continue;
                    }
                    p /= x;
                    q /= x;
                    r /= x;
                }
                s = (p * p + q * q + r * r).sqrt();
                if p < 0.0 {
                    s = -s;
                }
                if s != 0.0 {
                    if k != m {
                        hh!(k, k - 1) = -s * x;
                    } else if l != m {
                        hh!(k, k - 1) = -hh!(k, k - 1);
                    }
                    p += s;
                    x = p / s;
                    y = q / s;
                    z = r / s;
                    q /= p;
                    r /= p;
                    for j in k..nn as isize {
                        p = hh!(k, j) + q * hh!(k + 1, j);
                        if notlast {
                            p += r * hh!(k + 2, j);
                            hh!(k + 2, j) -= p * z;
                        }
                        hh!(k, j) -= p * x;
                        hh!(k + 1, j) -= p * y;
                    }
                    for i in 0..=n.min(k + 3) {
                        p = x * hh!(i, k) + y * hh!(i, k + 1);
                        if notlast {
                            p += z * hh!(i, k + 2);
                            hh!(i, k + 2) -= p * r;
                        }
                        hh!(i, k) -= p;
                        hh!(i, k + 1) -= p * q;
                    }
                    for i in low..=high {
                        p = x * vv!(i, k) + y * vv!(i, k + 1);
                        if notlast {
                            p += z * vv!(i, k + 2);
                            vv!(i, k + 2) -= p * r;
                        }
                        vv!(i, k) -= p;
                        vv!(i, k + 1) -= p * q;
                    }
                }
                k += 1;
            }
        }
    }
    if !vectors || norm == 0.0 {
        return;
    }
    n = nn as isize - 1;
    while n >= 0 {
        p = d[n as usize];
        q = e[n as usize];
        if q == 0.0 {
            let mut l = n;
            hh!(n, n) = 1.0;
            let mut i = n - 1;
            while i >= 0 {
                w = hh!(i, i) - p;
                r = 0.0;
                for j in l..=n {
                    r += hh!(i, j) * hh!(j, n);
                }
                if e[i as usize] < 0.0 {
                    z = w;
                    s = r;
                } else {
                    l = i;
                    if e[i as usize] == 0.0 {
                        hh!(i, n) = if w != 0.0 { -r / w } else { -r / (eps * norm) };
                    } else {
                        x = hh!(i, i + 1);
                        y = hh!(i + 1, i);
                        q = (d[i as usize] - p) * (d[i as usize] - p)
                            + e[i as usize] * e[i as usize];
                        t = (x * s - z * r) / q;
                        hh!(i, n) = t;
                        hh!(i + 1, n) = if x.abs() > z.abs() {
                            (-r - w * t) / x
                        } else {
                            (-s - y * t) / z
                        };
                    }
                    t = hh!(i, n).abs();
                    if (eps * t) * t > 1.0 {
                        for j in i..=n {
                            hh!(j, n) /= t;
                        }
                    }
                }
                i -= 1;
            }
        } else if q < 0.0 {
            let mut l = n - 1;
            if hh!(n, n - 1).abs() > hh!(n - 1, n).abs() {
                hh!(n - 1, n - 1) = q / hh!(n, n - 1);
                hh!(n - 1, n) = -(hh!(n, n) - p) / hh!(n, n - 1);
            } else {
                let (cr, ci) = cdiv(0.0, -hh!(n - 1, n), hh!(n - 1, n - 1) - p, q);
                hh!(n - 1, n - 1) = cr;
                hh!(n - 1, n) = ci;
            }
            hh!(n, n - 1) = 0.0;
            hh!(n, n) = 1.0;
            let mut i = n - 2;
            while i >= 0 {
                let mut ra = 0.0;
                let mut sa = 0.0;
                for j in l..=n {
                    ra += hh!(i, j) * hh!(j, n - 1);
                    sa += hh!(i, j) * hh!(j, n);
                }
                w = hh!(i, i) - p;
                if e[i as usize] < 0.0 {
                    z = w;
                    r = ra;
                    s = sa;
                } else {
                    l = i;
                    if e[i as usize] == 0.0 {
                        let (cr, ci) = cdiv(-ra, -sa, w, q);
                        hh!(i, n - 1) = cr;
                        hh!(i, n) = ci;
                    } else {
                        x = hh!(i, i + 1);
                        y = hh!(i + 1, i);
                        let di = d[i as usize] - p;
                        let mut vr = di * di + e[i as usize] * e[i as usize] - q * q;
                        let vi = di * 2.0 * q;
                        if vr == 0.0 && vi == 0.0 {
                            vr = eps * norm * (w.abs() + q.abs() + x.abs() + y.abs() + z.abs());
                        }
                        let (cr, ci) =
                            cdiv(x * r - z * ra + q * sa, x * s - z * sa - q * ra, vr, vi);
                        hh!(i, n - 1) = cr;
                        hh!(i, n) = ci;
                        if x.abs() > z.abs() + q.abs() {
                            hh!(i + 1, n - 1) = (-ra - w * hh!(i, n - 1) + q * hh!(i, n)) / x;
                            hh!(i + 1, n) = (-sa - w * hh!(i, n) - q * hh!(i, n - 1)) / x;
                        } else {
                            let (cr, ci) = cdiv(-r - y * hh!(i, n - 1), -s - y * hh!(i, n), z, q);
                            hh!(i + 1, n - 1) = cr;
                            hh!(i + 1, n) = ci;
                        }
                    }
                    t = hh!(i, n - 1).abs().max(hh!(i, n).abs());
                    if (eps * t) * t > 1.0 {
                        for j in i..=n {
                            hh!(j, n - 1) /= t;
                            hh!(j, n) /= t;
                        }
                    }
                }
                i -= 1;
            }
        }
        n -= 1;
    }
    for j in (low..nn as isize).rev() {
        for i in low..=high {
            z = 0.0;
            for k in low..=j.min(high) {
                z += vv!(i, k) * hh!(k, j);
            }
            vv!(i, j) = z;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5
    }

    #[test]
    fn lu_solves() {
        let mut s = 1;
        let n = 9;
        let mut m = Mat::zeros(n, n);
        for x in &mut m.a {
            *x = rnd(&mut s);
        }
        let b: Vec<f64> = (0..n).map(|_| rnd(&mut s)).collect();
        let x = Lu::new(&m).solve(&b);
        let r = m.mul_vec(&x);
        for i in 0..n {
            assert!((r[i] - b[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn sym_eig_decomposes() {
        let mut s = 2;
        let n = 12;
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            for j in i..n {
                let x = rnd(&mut s);
                m[(i, j)] = x;
                m[(j, i)] = x;
            }
        }
        let (w, v) = sym_eig(&m);
        let mut d = Mat::zeros(n, n);
        for i in 0..n {
            d[(i, i)] = w[i];
        }
        let r = m.mul(&v).sub(&v.mul(&d));
        assert!(r.max_abs() < 1e-12, "{}", r.max_abs());
        assert!(w.windows(2).all(|p| p[0] >= p[1]));
    }

    fn check_eig(m: &Mat) {
        let n = m.rows;
        let ed = eig(m);
        let mut d = Mat::zeros(n, n);
        for i in 0..n {
            d[(i, i)] = ed.d[i];
            if ed.e[i] > 0.0 {
                d[(i, i + 1)] = ed.e[i];
            } else if ed.e[i] < 0.0 {
                d[(i, i - 1)] = ed.e[i];
            }
        }
        let r = m.mul(&ed.v).sub(&ed.v.mul(&d));
        let scale = m.max_abs() * ed.v.max_abs();
        assert!(r.max_abs() < 1e-10 * scale, "{} of {scale}", r.max_abs());
    }

    #[test]
    fn schur_and_lyapunov() {
        let mut s = 5;
        let n = 25;
        let mut a = Mat::zeros(n, n);
        for x in &mut a.a {
            *x = rnd(&mut s) * 3.0;
        }
        for i in 0..n {
            a[(i, i)] -= 10.0 * 10f64.powf(i as f64 * 0.2);
        }
        let sch = schur(&a);
        // S Z T Zᴴ S⁻¹ = A.
        let back = sch.z.mul(&sch.t).mul(&sch.z.h()).re();
        for i in 0..n {
            for j in 0..n {
                let x = back[(i, j)] * sch.scales[i] / sch.scales[j];
                assert!(
                    (x - a[(i, j)]).abs() < 1e-9 * a.max_abs(),
                    "{x} {}",
                    a[(i, j)]
                );
            }
            for j in 0..i {
                assert_eq!(sch.t.get(i, j), C64::ZERO);
            }
        }
        let mut b = Mat::zeros(n, 3);
        for x in &mut b.a {
            *x = rnd(&mut s);
        }
        let w = b.mul(&b.t());
        let x = lyap(&sch, &w);
        let r = a.mul(&x).add(&x.mul(&a.t())).add(&w);
        assert!(r.max_abs() < 1e-10 * w.max_abs(), "{}", r.max_abs());
    }

    #[test]
    fn svd_decomposes() {
        let mut s = 7;
        let mut m = Mat::zeros(9, 6);
        for x in &mut m.a {
            *x = rnd(&mut s);
        }
        let (u, sv, v) = svd(&m);
        let mut d = Mat::zeros(6, 6);
        for i in 0..6 {
            d[(i, i)] = sv[i];
        }
        let r = u.mul(&d).mul(&v.t()).sub(&m);
        assert!(r.max_abs() < 1e-12);
        assert!(sv.windows(2).all(|p| p[0] >= p[1]));
    }

    #[test]
    fn eig_decomposes() {
        let mut s = 3;
        for n in [1, 2, 3, 5, 17, 40] {
            let mut m = Mat::zeros(n, n);
            for x in &mut m.a {
                *x = rnd(&mut s);
            }
            check_eig(&m);
        }
        // Stiff and badly scaled: rates from 1 to 1e9.
        let n = 30;
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            for j in 0..n {
                m[(i, j)] = rnd(&mut s) * 10f64.powf((i as f64 + j as f64) * 0.15);
            }
            m[(i, i)] -= 10f64.powf(i as f64 * 0.3);
        }
        check_eig(&m);
    }
}
