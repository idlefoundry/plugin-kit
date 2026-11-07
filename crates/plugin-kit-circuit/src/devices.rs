//! Device parameters: the types an instrument's device cards are written in, and the
//! [`Library`] of cards a network is built with. The engine evaluates each kind with ngspice's
//! equations (`crate::dev`). An instrument keeps its own cards (from its data sheets) and hands
//! them over as a [`Library`]; every card here is referred to 25 C, as its circuit runs.
//! The structures are the TR-808's (its `olifant-circuit`, from the MC-79's), moved unchanged
//! (K-CIRCUIT).

/// Boltzmann's constant over the electron charge, V/K.
pub const K_OVER_Q: f64 = 8.617_333_262e-5;
/// The circuit's temperature, C.
pub const TEMP_C: f64 = 25.0;
/// The thermal voltage at [`TEMP_C`], V.
pub const VT: f64 = K_OVER_Q * (TEMP_C + 273.15);

/// A bipolar transistor's Gummel-Poon parameters (the subset the models use; the rest take
/// SPICE's defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bjt {
    pub name: &'static str,
    pub pnp: bool,
    /// Saturation current, A.
    pub is: f64,
    /// Ideal forward and reverse beta.
    pub bf: f64,
    pub br: f64,
    /// Forward Early voltage, V.
    pub vaf: f64,
    /// Forward high-injection knee current, A.
    pub ikf: f64,
    /// Base, emitter and collector resistance, ohm.
    pub rb: f64,
    pub re: f64,
    pub rc: f64,
    /// Junction capacitances at zero bias (F), built-in potentials (V), grading.
    pub cje: f64,
    pub vje: f64,
    pub mje: f64,
    pub cjc: f64,
    pub vjc: f64,
    pub mjc: f64,
    /// Forward and reverse transit times, s.
    pub tf: f64,
    pub tr: f64,
    /// Emitter-base reverse breakdown, V (infinite: not modelled). Written as a diode in
    /// parallel with the emitter junction where a model needs it.
    pub bv_eb: f64,
}

/// A junction diode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Diode {
    pub name: &'static str,
    pub is: f64,
    pub n: f64,
    pub rs: f64,
    pub cjo: f64,
    pub tt: f64,
    /// Reverse breakdown, V (infinite: none).
    pub bv: f64,
    pub ibv: f64,
}

/// An N-channel JFET: SPICE level 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jfet {
    pub name: &'static str,
    pub vto: f64,
    pub beta: f64,
    pub lambda: f64,
    pub cgs: f64,
    pub cgd: f64,
    pub is: f64,
    /// The gate junctions' built-in potential, V (ngspice's PB, 1 by default).
    pub pb: f64,
    /// The drain's and source's ohmic resistances, ohm (ngspice's RD and RS; 0: none).
    pub rd: f64,
    pub rs: f64,
}

/// An operational amplifier as a behavioural macromodel (mc-79's): a transconductance input
/// stage limited to the slew rate, one dominant pole, an output that saturates `vsat_*` short
/// of the rails, an output resistance, and the inputs' bias current.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpAmp {
    pub name: &'static str,
    /// Open-loop DC gain.
    pub a0: f64,
    /// Gain-bandwidth product, Hz.
    pub gbw: f64,
    /// Slew rate, V/s.
    pub sr: f64,
    /// How far short of each rail the output saturates, V.
    pub vsat_pos: f64,
    pub vsat_neg: f64,
    /// Output resistance, ohm.
    pub rout: f64,
    /// Input bias current into each input, A.
    pub ib: f64,
    /// The load the slew rate is specified into, ohm: the internal node's current limit is
    /// raised by (1 + rout / sr_load) so the output slews at `sr` into it (infinite: none).
    pub sr_load: f64,
    /// The clamp holding the internal node within the output's swing: its conductance, S, and
    /// its knee, V (`sim::OPAMP_CLAMP_G` and `OPAMP_CLAMP_KNEE` are the TR-808's).
    pub clamp_g: f64,
    pub clamp_knee: f64,
}

/// One inverter of a CMOS hex Schmitt trigger, as a behavioural model: an input
/// capacitance, a regenerative state whose threshold falls from `vt_pos` to `vt_neg` (as
/// fractions of the supply) once the input has crossed it, a delay, and an output that
/// switches between the rails through `rout`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Schmitt {
    pub name: &'static str,
    /// The positive- and negative-going thresholds, as fractions of VDD - VSS.
    pub vt_pos: f64,
    pub vt_neg: f64,
    /// The output's resistance to either rail, ohm.
    pub rout: f64,
    /// Input capacitance to VSS, F.
    pub cin: f64,
    /// The state's time constant (about the propagation delay), s.
    pub tau: f64,
}

/// A transconductance amplifier ("vari-conductance amplifier"): an amplifier bias current
/// sets its transconductance. `pins` names its terminals in the order a part lists its nodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ota {
    pub name: &'static str,
    pub pins: &'static [&'static str],
    /// Transconductance per unit bias current, 1/V (`gm = gm_per_i * Iabc`; a bipolar
    /// differential pair's is 1 / (2 VT)).
    pub gm_per_i: f64,
    /// The output's full scale per unit bias current at low currents (1 for ideal mirrors; less
    /// where the mirrors and the input pair lose some of it).
    pub gain: f64,
    /// The bias current over which the full scale grows, A: it is `gain * Iabc * (1 + Iabc /
    /// i_knee)` (mirrors whose transistors' gain falls at high current where their diodes'
    /// does not); infinite for none.
    pub i_knee: f64,
    /// The input pair's current gain: each input draws `Iabc (1 +- tanh) / (2 beta_in)` to V-
    /// (the pair's base currents); infinite for none.
    pub beta_in: f64,
    /// The bias input's offset voltage above V-, V (one diode drop for a mirror input).
    pub vbias: f64,
    /// Output resistance, ohm, and capacitance, F.
    pub rout: f64,
    pub cout: f64,
}

/// One comparator of a quad with open-collector outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Comparator {
    pub name: &'static str,
    /// The output transistor's largest sink current, A.
    pub isink: f64,
    /// The output's saturation (the voltage over which the sink current builds up), V.
    pub vsat: f64,
    /// The input difference over which the output switches, V.
    pub vin_width: f64,
    /// The switching time constant, s.
    pub tau: f64,
    /// Input bias current (out of each input, PNP inputs), A.
    pub ib: f64,
    /// How far below VCC the inputs' common-mode range ends, V. With both inputs above it the
    /// PNP input stage has no tail current and the output transistor turns on (output low),
    /// as in the LM339-class topology; without it a comparator idling with both inputs at its
    /// supply has no defined state.
    pub vcm_top: f64,
    /// Input capacitance of each input to ground, F, and the output's capacitance, F (they
    /// also keep a positive-feedback loop round the comparator integrable).
    pub cin: f64,
    pub cout: f64,
}

/// A MOS transistor as ngspice's level 1 evaluates it (`mos1load.c`) with GAMMA 0 (no body
/// effect) and without the gate's Meyer charge: the channel's Shichman-Hodges current, its
/// source and drain swapping with the sign of vds, and the bulk-source and bulk-drain junctions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mos {
    pub name: &'static str,
    pub pmos: bool,
    /// The threshold, V, signed as the SPICE card gives it (negative for a P-channel device).
    pub vto: f64,
    /// KP W / L, A/V^2.
    pub beta: f64,
    /// Channel-length modulation, 1/V.
    pub lambda: f64,
    /// The bulk junctions' saturation current, A (ngspice's IS, 1e-14 by default).
    pub is: f64,
}

impl Mos {
    /// The device's type as `mos1load.c` multiplies by it: +1 N-channel, -1 P-channel.
    pub fn type_sign(&self) -> f64 {
        if self.pmos { -1.0 } else { 1.0 }
    }
}

/// The device cards a network is built with: every model a part may name, by its `name`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Library {
    pub bjts: &'static [&'static Bjt],
    pub diodes: &'static [&'static Diode],
    pub jfets: &'static [&'static Jfet],
    pub opamps: &'static [&'static OpAmp],
    pub schmitts: &'static [&'static Schmitt],
    pub otas: &'static [&'static Ota],
    pub comparators: &'static [&'static Comparator],
    pub mos: &'static [&'static Mos],
}

impl Library {
    /// The OTA card called `name`, if the library has it.
    pub fn ota(&self, name: &str) -> Option<&'static Ota> {
        self.otas.iter().copied().find(|o| o.name == name)
    }
}

/// The TR-808's cards (its `olifant-circuit`'s `devices.rs`, unchanged): the engine's own tests
/// evaluate real devices with them. Not part of any instrument's library.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// 2SC945 (NEC), class P (hFE 200-400): the boards' default NPN ("ALL NPN TRANSISTORS ARE
    /// 2SC945(P)", p. 8). Components C1.
    pub const Q2SC945: Bjt = Bjt {
        name: "Q2SC945",
        pnp: false,
        // DS: VBE 0.62 V typ at IC 1 mA, VCE 6 V.
        is: 3.31e-14,
        // DS: the geometric mean of class P.
        bf: 283.0,
        br: 5.0,
        // DS: hoe 18 uS at IC 1 mA (IC / hoe).
        vaf: 55.0,
        // DC: hFE falls about 10 % from its peak by 100 mA.
        ikf: 0.4,
        // DC: the noise curve's 1.8 nV/rtHz floor taken as rbb' alone (an upper bound).
        rb: 200.0,
        re: 0.3,
        rc: 1.0,
        // GS / DC: from the fT-IC curve's low-current slope.
        cje: 10e-12,
        vje: 0.75,
        mje: 0.33,
        // DS: Cob 3.0 pF typ at VCB 6 V, referred to 0 V.
        cjc: 6.2e-12,
        vjc: 0.75,
        mjc: 0.33,
        // DS: fT 250 MHz typ at IE 10 mA.
        tf: 0.6e-9,
        tr: 50e-9,
        bv_eb: f64::INFINITY,
    };

    /// 2SA733 (NEC), class P (hFE 200-400): the boards' default PNP. Components C2.
    pub const Q2SA733: Bjt = Bjt {
        name: "Q2SA733",
        pnp: true,
        // DS: VBE -0.62 V typ at -1 mA.
        is: 3.31e-14,
        // DS: class P.
        bf: 283.0,
        br: 5.0,
        // DS: hoe 28 uS at 1 mA.
        vaf: 36.0,
        ikf: 0.4,
        // DS: hie 5.5 kOhm at hfe 205, 1 mA (hie - hfe VT / IC).
        rb: 230.0,
        re: 0.3,
        rc: 1.0,
        cje: 10e-12,
        vje: 0.75,
        mje: 0.33,
        // DS: Cob 4.5 pF at 10 V.
        cjc: 10.8e-12,
        vjc: 0.75,
        mjc: 0.33,
        // DS: fT 180 MHz at 10 mA.
        tf: 0.83e-9,
        tr: 60e-9,
        bv_eb: f64::INFINITY,
    };

    /// 2SC828 (Matsushita), class R (hFE 180-360), "selected noise": the noise generator's
    /// junction. Only its emitter junction's breakdown matters (the noise's level is calibrated).
    /// Components C3.
    pub const Q2SC828: Bjt = Bjt {
        name: "Q2SC828",
        // DS: class R's geometric mean; VEBO and V(BR)EBO 7 V min at IE 10 uA (no typical
        // value: the minimum is taken).
        bf: 255.0,
        tf: 0.72e-9,
        bv_eb: 7.0,
        ..Q2SC945
    };

    /// 1S2473 (Rohm's data book; the parts list's maker): the boards' default diode ("ALL DIODES
    /// ARE 1S2473"). Components C7: IS, N, RS fitted to the VF-IF curve (0.2 to 100 mA, 10 mV RMS).
    pub const D1S2473: Diode = Diode {
        name: "D1S2473",
        is: 2.78e-10,
        n: 1.55,
        rs: 1.7,
        // DC: CT 1.6 pF at 0 V.
        cjo: 1.6e-12,
        // DC: trr 1.5 ns at IF 10 mA.
        tt: 2e-9,
        bv: f64::INFINITY,
        ibv: 0.0,
    };

    /// 2SK30ATM (Toshiba), class Y (IDSS 1.20-3.00 mA). Components C9.
    pub const J2SK30AY: Jfet = Jfet {
        name: "J2SK30AY",
        // DC: VGS(off) = -1.0 V (IDSS / 1 mA)^0.56 at class Y's geometric mean IDSS, 1.90 mA.
        vto: -1.43,
        // DS: IDSS / VTO^2.
        beta: 0.93e-3,
        lambda: 0.01,
        // DS: Ciss 8.2 pF, Crss 2.6 pF.
        cgs: 5.6e-12,
        cgd: 2.6e-12,
        is: 1e-14,
        pb: 1.0,
        rd: 0.0,
        rs: 0.0,
    };

    /// uPC4558C (NEC): mc-79's 4558 macromodel (its C8, NJM4558 typical values), the same part,
    /// with the uPC4558's own typical input bias current. Components C10.
    pub const OA4558: OpAmp = OpAmp {
        name: "OA4558",
        a0: 1e5,
        gbw: 3e6,
        sr: 1e6,
        vsat_pos: 1.0,
        vsat_neg: 1.0,
        rout: 75.0,
        ib: 60e-9,
        sr_load: f64::INFINITY,
        clamp_g: crate::sim::OPAMP_CLAMP_G,
        clamp_knee: crate::sim::OPAMP_CLAMP_KNEE,
    };

    /// HD14584B (Hitachi): hex Schmitt trigger, at its VDD of about 5 V on the voicing board (Q9's
    /// emitter, `metal.md`). Components C11: VT+ 2.7 V and VT- 2.1 V typ at VDD 5 V; output
    /// 0.88 mA typ at 0.4 V from the rail; 125 ns typ delay into 50 pF; 5 pF input.
    pub const HD14584: Schmitt = Schmitt {
        name: "HD14584",
        vt_pos: 2.7 / 5.0,
        vt_neg: 2.1 / 5.0,
        rout: 455.0,
        cin: 5e-12,
        tau: 125e-9,
    };

    /// BA662A (Rohm, made for Roland): vari-conductance amplifier with a buffer. No data sheet
    /// exists; the pins are the original maker's (the JP-4's and CR-8000's service notes), the
    /// transconductance a bipolar pair's (the BA6110's sheet agrees: 19.2 /V). Components C12.
    pub const BA662: Ota = Ota {
        name: "BA662",
        pins: &[
            "iabc", "in-", "in+", "bias", "v-", "out", "buf_in", "buf_out", "v+",
        ],
        gm_per_i: 1.0 / (2.0 * VT),
        gain: 1.0,
        i_knee: f64::INFINITY,
        beta_in: f64::INFINITY,
        vbias: 0.6,
        rout: 1e7,
        cout: 5e-12,
    };

    /// AN6912 (Matsushita): quad comparator, open-collector outputs (LM339 pins). Components C13:
    /// sink about 9.5 mA at its limit, 0.2 V at 3 mA; 0.3-0.65 us at 60 mV overdrive; bias
    /// about 45 nA.
    pub const AN6912: Comparator = Comparator {
        name: "AN6912",
        isink: 9.5e-3,
        vsat: 0.65,
        vin_width: 1e-3,
        tau: 0.3e-6,
        ib: 45e-9,
        // COORDINATOR: provisional (see the struct).
        vcm_top: 1.5,
        cin: 3e-12,
        cout: 5e-12,
    };
}
