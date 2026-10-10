//! Where the voices sit in the stereo field and how loud: the pan law, SCATTER's placement of
//! POLY's voices at full SPREAD and DOUBLE's pairs (the CA-74's R27, R28, R30 and R41), the
//! glide a voice's place follows its controls with, and the trims of voices summed (R25, R28).

/// The pan law: at `pan` (0 left, 0.5 centre, 1 right) each side's gain, of constant power
/// (a voice as loud wherever it sits, so the centre does not outweigh the sides), both
/// whole at the centre as with SPREAD at 0: √2 cos and √2 sin of a quarter turn's `pan` (the
/// CA-74's R27).
pub fn pan_gains(pan: f64) -> (f32, f32) {
    let a = pan.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
    (
        (std::f64::consts::SQRT_2 * a.cos()) as f32,
        (std::f64::consts::SQRT_2 * a.sin()) as f32,
    )
}

/// Where SCATTER puts the voices POLY plays at full SPREAD, the player's choice of three (the
/// CA-74's R30); each voice as loud wherever it sits ([`pan_gains`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Evenly spaced from edge to edge, however many voices play: taken left and right in
    /// turn, each side's from its edge, then halfway in, then the rest from the outside
    /// inwards; with an odd number the last in the centre. The first two voices (a pair, a
    /// chord's first notes) are at the edges, and the field is filled without a hole.
    #[default]
    Even,
    /// From the edges inwards, out to either side in turn, every voice from 60 % out and none
    /// in the centre: the widest, its voices together at the sides (the CA-74's R27's).
    Edges,
    /// From the centre outwards, by the golden ratio: the first voice in the centre and the
    /// next spread evenly about it, however many play: the narrowest.
    Centre,
}

/// [`Placement::Edges`]'s places, voice by voice; also the most voices placed apart.
const EDGES: [f64; 10] = [-1.0, 1.0, -0.9, 0.9, -0.8, 0.8, -0.7, 0.7, -0.6, 0.6];

impl Placement {
    /// Where voice `k` of `n` (VOICES) sits at full SPREAD, -1 (left) to 1 (right). A voice
    /// past `n` (one letting go after VOICES was lowered) takes the place of `k` less `n`.
    pub fn place(self, k: usize, n: usize) -> f64 {
        let n = n.clamp(2, EDGES.len());
        let k = k % n;
        match self {
            Placement::Edges => EDGES[k],
            Placement::Centre => 2.0 * (0.5 + k as f64 * 0.618_033_988_749_894_8).fract() - 1.0,
            Placement::Even => {
                let side = n / 2;
                if k >= 2 * side {
                    return 0.0;
                }
                // The `i`th place taken on its side, `j` places in from its edge.
                let i = k / 2;
                let middle = side / 2;
                let j = match i {
                    0 => 0,
                    1 => middle,
                    _ if i <= middle => i - 1,
                    _ => i,
                };
                let at = 1.0 - 2.0 * j as f64 / (n - 1) as f64;
                if k.is_multiple_of(2) { -at } else { at }
            }
        }
    }

    /// How far out to either side DOUBLE's pair of voice `k` of `n` sits at full SPREAD, 0 to 1
    /// (the CA-74's R41): with EDGES every pair at the edges; with EVEN and CENTER as far out as
    /// the voice's own place, so each pair stays about the centre (a voice placed in the
    /// centre, its pair there too).
    pub fn pair(self, k: usize, n: usize) -> f64 {
        match self {
            Placement::Edges => 1.0,
            _ => self.place(k, n).abs(),
        }
    }

    /// Its index (0..3), as a voices' mix carries it and a preset saves it, and back.
    pub fn index(self) -> usize {
        match self {
            Placement::Even => 0,
            Placement::Edges => 1,
            Placement::Centre => 2,
        }
    }

    pub fn from_index(i: usize) -> Placement {
        match i {
            1 => Placement::Edges,
            2 => Placement::Centre,
            _ => Placement::Even,
        }
    }
}

/// The gains (left, right) that put voice `k` of `n` in its place, as `placement` has it, by
/// `spread` (0..1) of SPREAD ([`pan_gains`]; both whole at 0).
pub fn voice_gains(spread: f64, placement: Placement, k: usize, n: usize) -> (f32, f32) {
    if spread > 0.0 {
        pan_gains(0.5 + 0.5 * spread * placement.place(k, n))
    } else {
        (1.0, 1.0)
    }
}

/// DOUBLE's pair `out` (0 to 1, [`Placement::pair`]) by `spread` (0..1): the gains (left, right)
/// of the voice, half the detune flat, that far to the right, and of its twin, half sharp, as
/// far to the left (the CA-74's R28 and R41).
pub fn pair_gains(spread: f64, out: f64) -> [(f32, f32); 2] {
    [
        pan_gains(0.5 + 0.5 * spread * out),
        pan_gains(0.5 - 0.5 * spread * out),
    ]
}

/// The detune between DOUBLE's two voices of a note at its full amount, cents (the CA-74's R28).
pub const DOUBLE_CENTS: f64 = 20.0;

/// DOUBLE's trim on the output: a note's two voices, each its own, add in power, so it is
/// turned down by two's square root and sounds about as loud as one voice a note (R28).
pub const DOUBLE_TRIM: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// UNISON's trim on the output for `voices` voices on every key: not in phase, they add in
/// power, so it is turned down by their number's square root and sounds about as loud as one
/// voice (R25).
pub fn unison_trim(voices: usize) -> f64 {
    1.0 / (voices as f64).sqrt()
}

/// The time constant, seconds, with which a voice's place follows SPREAD, the placement,
/// VOICES and DOUBLE: a step (a host's automation at a block's start among them) glides
/// instead of stepping the output, a click (the CA-74's R27).
pub const GLIDE: f64 = 0.010;

/// A one-pole glide's share of the way a sample at `rate` Hz ([`GLIDE`]; all of it at no rate).
pub fn glide_share(rate: f64) -> f64 {
    if rate > 0.0 {
        1.0 - (-1.0 / (GLIDE * rate)).exp()
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every place carries a voice as loud as the centre does (constant power), and the
    /// centre is whole on both sides, as with SPREAD at 0.
    #[test]
    fn every_place_is_as_loud_as_the_centre() {
        for k in 0..=20 {
            let (l, r) = pan_gains(f64::from(k) / 20.0);
            let power = f64::from(l).powi(2) + f64::from(r).powi(2);
            assert!((power - 2.0).abs() < 1e-5, "at {k}/20: {power}");
        }
        let (l, r) = pan_gains(0.5);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6, "{l}, {r}");
        assert_eq!(pan_gains(0.0).1, 0.0);
    }

    /// EVEN: however many voices play, their places are evenly spaced from edge to edge (each
    /// one once), the first two at the edges, then left and right in turn as mirror pairs, the
    /// second pair halfway in; with an odd number the last in the centre. Eight: 100, 43, 71
    /// and 14 % out.
    #[test]
    fn even_fills_the_field_evenly_from_its_edges() {
        let place = |k, n| Placement::Even.place(k, n);
        for n in 2..=10 {
            let places: Vec<f64> = (0..n).map(|k| place(k, n)).collect();
            let mut sorted = places.clone();
            sorted.sort_by(f64::total_cmp);
            for (i, p) in sorted.iter().enumerate() {
                let even = -1.0 + 2.0 * i as f64 / (n - 1) as f64;
                assert!((p - even).abs() < 1e-12, "{n} voices: {places:?}");
            }
            assert_eq!((places[0], places[1]), (-1.0, 1.0), "{n} voices");
            for k in 0..n / 2 {
                assert_eq!(places[2 * k], -places[2 * k + 1], "{n} voices: {places:?}");
                assert!(places[2 * k] < 0.0, "{n} voices: {places:?}");
            }
            if n % 2 == 1 {
                assert_eq!(places[n - 1], 0.0);
            }
            if n >= 6 {
                assert!((0.3..=0.7).contains(&places[3]), "{n} voices: {places:?}");
            }
            // A voice past the count takes an earlier one's place.
            assert_eq!(place(n + 1, n), place(1, n));
        }
        let eight: Vec<f64> = (0..8).map(|k| (place(k, 8) * 7.0).round()).collect();
        assert_eq!(eight, [-7.0, 7.0, -3.0, 3.0, -5.0, 5.0, -1.0, 1.0]);
    }

    /// EDGES: no voice in the centre, none nearer it than 60 %; voices taken in turn go to
    /// either side in turn, from the edges inwards (the CA-74's R27's).
    #[test]
    fn edges_fills_the_field_from_its_edges() {
        let places: Vec<f64> = (0..10).map(|k| Placement::Edges.place(k, 10)).collect();
        for (k, p) in places.iter().enumerate() {
            assert!(p.abs() >= 0.6, "voice {k} at {p}");
            assert_eq!(*p < 0.0, k % 2 == 0, "voice {k} on the wrong side");
        }
        assert!(places.windows(2).all(|w| w[0].abs() >= w[1].abs()));
    }

    /// CENTER: the first voice in the centre, the rest by the golden ratio about it, as the
    /// CA-74's are; never two in one place.
    #[test]
    fn centre_starts_in_the_centre_and_spreads_by_the_golden_ratio() {
        let places: Vec<f64> = (0..10).map(|k| Placement::Centre.place(k, 10)).collect();
        let heard = [
            0.0, -0.763932, 0.472136, -0.291796, 0.944272, 0.18034, -0.583592, 0.652476, -0.111456,
            -0.875388,
        ];
        for (p, h) in places.iter().zip(heard) {
            assert!((p - h).abs() < 1e-6, "{places:?}");
        }
        let mut sorted = places.clone();
        sorted.sort_by(f64::total_cmp);
        assert!(sorted.windows(2).all(|w| w[1] - w[0] > 0.05), "{sorted:?}");
    }

    #[test]
    fn a_placement_goes_by_its_index_and_back() {
        for p in [Placement::Even, Placement::Edges, Placement::Centre] {
            assert_eq!(Placement::from_index(p.index()), p);
        }
        assert_eq!(Placement::default(), Placement::Even);
    }

    /// DOUBLE's pairs mirror each other about the centre (the twin to the left, the voice as far
    /// to the right), whole at no SPREAD; EDGES puts every pair at the edges, EVEN and CENTER
    /// as far out as the voice's own place.
    #[test]
    fn a_pair_mirrors_about_the_centre() {
        for out in [0.0, 0.3, 1.0] {
            let [voice, twin] = pair_gains(0.8, out);
            assert_eq!((voice.0, voice.1), (twin.1, twin.0), "out {out}");
        }
        assert_eq!(pair_gains(0.0, 1.0), [pan_gains(0.5); 2]);
        assert_eq!(Placement::Edges.pair(3, 8), 1.0);
        assert_eq!(Placement::Centre.pair(0, 8), 0.0);
        assert_eq!(
            Placement::Even.pair(2, 8),
            Placement::Even.place(2, 8).abs()
        );
        assert_eq!(voice_gains(0.0, Placement::Edges, 0, 4), (1.0, 1.0));
        assert_eq!(voice_gains(1.0, Placement::Edges, 0, 4), pan_gains(0.0));
    }

    /// The trims: voices summed in power, as loud as one; the glide's share a sample, all of it
    /// at no rate.
    #[test]
    fn the_trims_and_the_glide() {
        assert_eq!(unison_trim(4), 0.5);
        assert!((DOUBLE_TRIM * DOUBLE_TRIM - 0.5).abs() < 1e-15);
        assert_eq!(glide_share(0.0), 1.0);
        let k = glide_share(48_000.0);
        // After the time constant's worth of samples, 1 - 1/e of the way.
        let left = (1.0 - k).powi(480);
        assert!((left - (-1.0f64).exp()).abs() < 1e-9, "{left}");
    }
}
