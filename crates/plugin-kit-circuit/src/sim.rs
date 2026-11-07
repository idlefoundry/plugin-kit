//! What a simulation takes as given besides the parts and the device cards: the behavioural
//! models' internal constants, a film capacitor's dielectric loss, a pot's sections and
//! SPICE's shunt. An instrument's lab writes the same numbers into its ngspice netlists, so the
//! real-time model and the reference stay one circuit. From the TR-808's `olifant-circuit`
//! (`sim.rs`), unchanged but for what is that instrument's own (K-CIRCUIT).

/// The smooth ramp the behavioural models' clamps use: max(z, 0) + k ln(1 + exp(-|z| / k)),
/// which cannot overflow (the netlists write the same expression).
pub fn softplus(z: f64, k: f64) -> f64 {
    z.max(0.0) + k * libm::log1p(libm::exp(-(z.abs()) / k))
}

/// Its derivative in z: the logistic function of z / k.
pub fn softplus_d(z: f64, k: f64) -> f64 {
    let e = libm::exp(-(z.abs()) / k);
    if z >= 0.0 {
        1.0 / (1.0 + e)
    } else {
        e / (1.0 + e)
    }
}

/// SPICE's `rshunt`: a resistance from every node to ground, so nodes between capacitors have
/// a DC path.
pub const RSHUNT: f64 = 1e12;

/// A pot's or trimmer's two sections: (terminal 3 to wiper, wiper to terminal 1), ohm, each at
/// least [`crate::part::POT_MIN`].
pub fn pot_halves(r: f64, frac: f64) -> (f64, f64) {
    use crate::part::POT_MIN;
    ((r * (1.0 - frac)).max(POT_MIN), (r * frac).max(POT_MIN))
}

// --- The behavioural models' internals ------------------------------------------------------

/// The op-amp macromodel's transconductance, S (it sets the internal node's scale only).
pub const OPAMP_GM: f64 = 1e-3;
/// The clamp's knee, V, and conductance, S, holding the internal node within the output's
/// swing.
pub const OPAMP_CLAMP_KNEE: f64 = 0.02;
pub const OPAMP_CLAMP_G: f64 = 0.1;

/// The Schmitt inverter's state capacitance, F (its time constant sets the conductance).
pub const SCHMITT_CS: f64 = 1e-12;
/// The input difference over which the Schmitt's state is driven from one side to the other,
/// V.
pub const SCHMITT_WIDTH: f64 = 0.01;

/// The comparator's state capacitance, F.
pub const CMP_CS: f64 = 1e-12;
/// The width of the comparator's common-mode limit, V (added with `vcm_top`).
pub const CMP_CM_WIDTH: f64 = 0.05;

/// The transconductance amplifier's bias input is a diode whose drop is the model's `vbias` at
/// this current, A.
pub const OTA_BIAS_I: f64 = 0.5e-3;
/// How far short of either rail the transconductance amplifier's output saturates, V.
pub const OTA_SAT: f64 = 1.0;
/// The transconductance amplifier's buffer's output resistance, ohm (an ideal follower).
pub const OTA_BUF_R: f64 = 100.0;

// --- The film capacitors' loss ---------------------------------------------------------------

/// A capacitor's dielectric loss: tan δ at 1 kHz (where capacitors are specified) and its
/// slope with frequency, tan δ(f) = `at_1k` (f / 1 kHz)^`slope`. [`CapLoss::NONE`]: ideal.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapLoss {
    pub at_1k: f64,
    pub slope: f64,
}

impl CapLoss {
    pub const NONE: CapLoss = CapLoss {
        at_1k: 0.0,
        slope: 0.0,
    };

    pub fn is_none(&self) -> bool {
        self.at_1k <= 0.0
    }

    /// tan δ at `f` Hz.
    pub fn tan_d(&self, f: f64) -> f64 {
        self.at_1k * (f / 1e3).powf(self.slope)
    }

    /// `AT_1K` or `AT_1K:SLOPE`, as the command line gives it.
    pub fn parse(s: &str) -> Option<CapLoss> {
        let (a, b) = s.split_once(':').unwrap_or((s, "0"));
        Some(CapLoss {
            at_1k: a.parse().ok()?,
            slope: b.parse().ok()?,
        })
    }
}

/// The frequencies (Hz) of a lossy capacitor's relaxation branches: two a decade from 1 Hz to
/// 3.16 MHz, wide enough that a loss rising with frequency is held within 8 % of its target
/// from 20 Hz to 2.5 kHz (12 % at 15 kHz), a constant one within 2 % from 20 Hz to 20 kHz.
pub const LOSS_FREQS: [f64; 14] = [
    1.0,
    3.162_277_66,
    10.0,
    31.622_776_6,
    100.0,
    316.227_766,
    1e3,
    3_162.277_66,
    1e4,
    31_622.776_6,
    1e5,
    316_227.766,
    1e6,
    3_162_277.66,
];

/// A capacitor of `c` farad with the dielectric loss `loss` over the audio band: a capacitor
/// `c0` in parallel with series R-C branches (R, C) relaxing at [`LOSS_FREQS`]. A spread of
/// relaxation times g(τ) per unit of ln τ, relative to `c`, gives tan δ = π/2 g for a constant
/// spread and π/(2 cos(π s/2)) g for one that rises as f^s: each branch holds that for its own
/// half-decade, at the loss's tan δ there. The branches below 10 Hz merge into one, and those
/// above 31.6 kHz into another, each equivalent to its group to first order over the audio
/// band (the netlist's size and stiffness). `c0` makes the capacitance `c` at 1 kHz, where
/// capacitors are specified; a constant loss makes it 0.7 % higher at 20 Hz and 0.6 % lower at
/// 20 kHz (the Kramers-Kronig relation).
pub fn lossy_capacitor(c: f64, loss: CapLoss) -> (f64, Vec<(f64, f64)>) {
    use std::f64::consts::PI;
    let spread = 2.0 / PI * (PI * loss.slope / 2.0).cos() * 10f64.sqrt().ln();
    let all: Vec<(f64, f64, f64)> = LOSS_FREQS
        .iter()
        .map(|f| {
            let ck = spread * loss.tan_d(*f) * c;
            (*f, 1.0 / (2.0 * PI * f * ck), ck)
        })
        .collect();
    let mut branches: Vec<(f64, f64)> = Vec::new();
    // Below the band each branch is nearly its resistor: sum the conductances and the
    // capacitive remainders 1 / (ω R² C).
    let low: Vec<_> = all.iter().filter(|b| b.0 < 10.0).collect();
    let g: f64 = low.iter().map(|b| 1.0 / b.1).sum();
    let q: f64 = low.iter().map(|b| 1.0 / (b.1 * b.1 * b.2)).sum();
    branches.push((1.0 / g, g * g / q));
    branches.extend(
        all.iter()
            .filter(|b| (10.0..=40e3).contains(&b.0))
            .map(|b| (b.1, b.2)),
    );
    // Above it each is nearly its capacitor: sum the capacitances and the losses ω² R C².
    let high: Vec<_> = all.iter().filter(|b| b.0 > 40e3).collect();
    let ch: f64 = high.iter().map(|b| b.2).sum();
    let rc2: f64 = high.iter().map(|b| b.1 * b.2 * b.2).sum();
    branches.push((rc2 / (ch * ch), ch));
    let at_1k: f64 = branches
        .iter()
        .map(|(r, ck)| {
            let x = 2.0 * PI * 1e3 * r * ck;
            ck / (1.0 + x * x)
        })
        .sum();
    (c - at_1k, branches)
}
