//! An instrument's parts as the engine reads them: a part's reference, kind and value, and the
//! node on each of its terminals. An instrument transcribes its circuit in its own crate (its
//! sections, boards and builder) and hands the engine these, a part's section by its id.
//!
//! Node names: a name written with a leading `@` is global (a rail, ground's `0`, or a net that
//! sections share); every other name is local to its section and is written with the section's
//! prefix (`q46b` in section `sd` is `sd_q46b`). `0` is always ground. From the TR-808's
//! `olifant-circuit` (`part.rs`), unchanged but for the section and what is that instrument's
//! own (K-CIRCUIT).

use serde::{Deserialize, Serialize};

/// Kilo-, mega-, micro-, nano- and pico-: `4.7 * K`.
pub const K: f64 = 1e3;
pub const MEG: f64 = 1e6;
pub const U: f64 = 1e-6;
pub const N: f64 = 1e-9;
pub const P: f64 = 1e-12;

/// The smallest resistance a pot's section is given, ohm: a wiper at an end.
pub const POT_MIN: f64 = 0.1;

/// A closed switch contact, ohm.
pub const CLOSED: f64 = 0.1;

/// A panel pot's taper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Taper {
    /// "A": audio (logarithmic).
    A,
    /// "B": linear.
    B,
    /// "C": reverse audio.
    C,
}

impl Taper {
    /// The fraction of the track's resistance between terminal 1 and the wiper with the knob
    /// at `pos` (0 fully counter-clockwise, 1 fully clockwise). The audio taper is the generic
    /// `(81^p - 1) / 80` (10 % at mid travel), the reverse taper its mirror image.
    pub fn frac(self, pos: f64) -> f64 {
        let p = pos.clamp(0.0, 1.0);
        let a = |p: f64| (81f64.powf(p) - 1.0) / 80.0;
        match self {
            Taper::A => a(p),
            Taper::B => p,
            Taper::C => 1.0 - a(1.0 - p),
        }
    }

    pub fn letter(self) -> char {
        match self {
            Taper::A => 'A',
            Taper::B => 'B',
            Taper::C => 'C',
        }
    }
}

/// What a part is, with its value or model. The node order is given for each kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Kind {
    /// Resistor, ohm (a, b).
    R(f64),
    /// Non-polar capacitor, F (a, b).
    C(f64),
    /// Polarised (electrolytic or tantalum) capacitor, F (+, -).
    Ce(f64),
    /// Non-polar ("BP") electrolytic capacitor, F (a, b): ideal, as the electrolytics are.
    Cbp(f64),
    /// Panel potentiometer: total ohm, taper, and the knob's position (0 fully
    /// counter-clockwise, 1 fully clockwise) (terminal 3, wiper 2, terminal 1). Turning
    /// clockwise moves the wiper towards terminal 3.
    Pot { r: f64, taper: Taper, pos: f64 },
    /// Semi-fixed trimmer (linear): total ohm and its position as a pot's (3, 2, 1).
    Trim { r: f64, pos: f64 },
    /// Bipolar transistor of the named model (collector, base, emitter).
    Q(&'static str),
    /// Diode of the named model (anode, cathode).
    D(&'static str),
    /// N-channel JFET (drain, gate, source).
    J(&'static str),
    /// One amplifier of a dual op-amp (+, -, out, V+, V-).
    OpAmp(&'static str),
    /// One inverter of a CMOS hex Schmitt trigger (in, out, VDD, VSS).
    Schmitt(&'static str),
    /// A transconductance ("vari-conductance") amplifier; its nodes are listed by the model
    /// ([`crate::devices::Ota`]).
    Ota(&'static str),
    /// One comparator of a quad open-collector comparator (+, -, out, VCC, GND).
    Comparator(&'static str),
    /// MOS transistor of the named model (drain, gate, source, bulk).
    M(&'static str),
    /// A closed switch contact or a wire link, ohm (a, b).
    Link(f64),
}

impl Kind {
    /// How many nodes a part of this kind has (an OTA's, from its card in `library`).
    pub fn arity(&self, library: &crate::devices::Library) -> usize {
        match self {
            Kind::R(_) | Kind::C(_) | Kind::Ce(_) | Kind::Cbp(_) | Kind::D(_) | Kind::Link(_) => 2,
            Kind::Pot { .. } | Kind::Trim { .. } | Kind::Q(_) | Kind::J(_) => 3,
            Kind::Schmitt(_) | Kind::M(_) => 4,
            Kind::OpAmp(_) | Kind::Comparator(_) => 5,
            Kind::Ota(m) => library.ota(m).map_or(0, |o| o.pins.len()),
        }
    }
}

/// One part of the transcription.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Part {
    /// The service notes' reference (`R183`, `Q46`, `VR7`, `IC13`), unique within a section.
    pub refdes: String,
    pub kind: Kind,
    /// The node on each terminal, in the kind's order, with the section's prefix applied to
    /// local names.
    pub nodes: Vec<String>,
    /// The id of the section it belongs to, as the instrument names its sections (`sd`).
    pub section: String,
    /// The block of the section it belongs to (a subsection of the transcription's page).
    pub block: &'static str,
    /// The package pins of the nodes, where the drawing gives them (`"3,2,1,8,4"` for an
    /// op-amp's +, -, out, V+, V-); empty otherwise.
    pub pins: &'static str,
    /// A remark for the parts table: a production change, an erratum, a reading.
    pub note: &'static str,
}
