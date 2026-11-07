//! A model run at twice the host's rate and decimated, for a circuit whose dynamics the host's
//! sample step is too coarse for (the TR-808's rim shot: at 48 kHz its accented strike is -12 dB
//! from ngspice, at 96 kHz -27 dB).

/// The decimator's stop band, dB.
const ATTEN: f64 = 100.0;

/// A decimator by two: a linear-phase low-pass FIR (Kaiser window), its pass band to 0.42 of the
/// output rate (20 kHz at 48 kHz) and its stop band from 0.58 (what folds onto the pass band)
/// [`ATTEN`] dB down; its delay a whole number of output samples ([`Half::delay`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Half {
    h: Vec<f64>,
    /// The input, twice over (the last `h.len()` inputs contiguous at `at`).
    x: Vec<f64>,
    at: usize,
}

impl Default for Half {
    fn default() -> Half {
        Half::new()
    }
}

impl Half {
    pub fn new() -> Half {
        // In input samples: the pass band to 0.21 of the input rate, the stop band from 0.29.
        let (fp, fs) = (0.21, 0.29);
        let n0 = ((ATTEN - 7.95) / (14.36 * (fs - fp))).ceil() as usize + 1;
        // Of length 4d + 1: its delay 2d input samples, d output samples.
        let d = n0.div_ceil(4);
        let n = 4 * d + 1;
        let beta = 0.1102 * (ATTEN - 8.7);
        let fc = 0.5 * (fp + fs);
        let m = (n - 1) as f64 / 2.0;
        let i0b = bessel_i0(beta);
        let mut h: Vec<f64> = (0..n)
            .map(|i| {
                let x = i as f64 - m;
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    libm::sin(2.0 * std::f64::consts::PI * fc * x) / (std::f64::consts::PI * x)
                };
                let r = x / m;
                sinc * bessel_i0(beta * libm::sqrt((1.0 - r * r).max(0.0))) / i0b
            })
            .collect();
        let sum: f64 = h.iter().sum();
        for c in &mut h {
            *c /= sum;
        }
        Half {
            x: vec![0.0; 2 * n],
            h,
            at: 0,
        }
    }

    /// The output's delay, output samples.
    pub fn delay(&self) -> usize {
        (self.h.len() - 1) / 4
    }

    /// Its history filled with `v`: the input held there from before the first sample (a voice
    /// at rest: its first outputs are its rest, not a ring-in from nothing).
    pub fn fill(&mut self, v: f64) {
        self.x.fill(v);
    }

    fn push_one(&mut self, v: f64) {
        let n = self.h.len();
        self.at = (self.at + 1) % n;
        self.x[self.at] = v;
        self.x[self.at + n] = v;
    }

    /// Two input samples in, one output sample out (centred `delay` output samples back, on the
    /// second's).
    pub fn push(&mut self, a: f64, b: f64) -> f64 {
        self.push_one(a);
        self.push_one(b);
        let n = self.h.len();
        // The newest input at `at + n`, the oldest at `at + 1`: `h` is symmetric.
        self.x[self.at + 1..self.at + n + 1]
            .iter()
            .zip(&self.h)
            .map(|(x, h)| x * h)
            .sum()
    }
}

/// The modified Bessel function of the first kind, order 0 (its series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    let mut k = 1.0;
    loop {
        term *= q / (k * k);
        sum += term;
        if term < 1e-17 * sum {
            return sum;
        }
        k += 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass band flat to 20 kHz at 48 kHz, the band that folds onto it (from 28 kHz) down
    /// by the stop band's attenuation, a step's delay as said.
    #[test]
    fn the_decimator() {
        let gain = |f: f64| {
            // f as a fraction of the input rate.
            let mut d = Half::new();
            let n = 4096;
            let mut acc = (0.0f64, 0.0f64);
            let mut k = 0;
            for j in 0..n {
                let a = libm::cos(2.0 * std::f64::consts::PI * f * (2 * j) as f64);
                let b = libm::cos(2.0 * std::f64::consts::PI * f * (2 * j + 1) as f64);
                let y = d.push(a, b);
                if j >= 512 {
                    acc.0 += y * y;
                    k += 1;
                }
            }
            acc.1 = (acc.0 / k as f64 * 2.0).sqrt();
            acc.1
        };
        for f in [0.01, 0.1, 0.2, 20.0 / 96.0] {
            let g = gain(f);
            assert!((g - 1.0).abs() < 1e-3, "pass band at {f}: {g}");
        }
        for f in [0.29, 0.35, 0.45] {
            let g = gain(f);
            assert!(20.0 * g.log10() < -ATTEN + 6.0, "stop band at {f}: {g}");
        }
        let mut d = Half::new();
        let dl = d.delay();
        let mut out = Vec::new();
        for j in 0..64 {
            let (a, b) = if j == 10 { (0.0, 1.0) } else { (0.0, 0.0) };
            out.push(d.push(a, b));
        }
        let peak = (0..out.len())
            .max_by(|&i, &j| out[i].total_cmp(&out[j]))
            .expect("one");
        assert_eq!(peak, 10 + dl, "the delay");
    }
}
