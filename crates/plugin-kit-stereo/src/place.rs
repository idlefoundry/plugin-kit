//! Where the voices sit in the stereo field and how loud: the pan law, SCATTER's placement of
//! POLY's voices at full SPREAD and DOUBLE's pairs (the CA-74's R27, R28, R30 and R41), INNER's
//! band on either side (K7), the glide a voice's place follows its controls with, and the trims
//! of voices summed (R25, R28).

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

    /// The side voice `k` of `n` sits on, -1 (left) or 1 (right): its place's; a voice placed
    /// in the centre (EVEN's last of an odd number, CENTER's first), the side its turn falls on,
    /// as EVEN takes them, the left first, so that INNER clears the centre of every voice (K7).
    pub fn side(self, k: usize, n: usize) -> f64 {
        let place = self.place(k, n);
        if place != 0.0 {
            place.signum()
        } else if (k % n.clamp(2, EDGES.len())).is_multiple_of(2) {
            -1.0
        } else {
            1.0
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

/// How far out on its side a voice sits by `inner` (0..1) of INNER, a share of SPREAD's way out,
/// for `out` (0 centre to 1 edge) where its placement puts it: the side's band from INNER's
/// share of the way out to SPREAD's edge, the voice as far across it as `out` (K7). At INNER 0
/// the band reaches the centre, `out` itself, to the bit, as before INNER; at 1 every voice at
/// the edge.
pub fn band(inner: f64, out: f64) -> f64 {
    inner + (1.0 - inner) * out
}

/// Where voice `k` of `n` sits, -1 (left) to 1 (right), as `placement` has it, by `spread`
/// (0..1) of SPREAD, in its side's band by `inner` (0..1) of INNER ([`band`],
/// [`Placement::side`]): where [`voice_gains`] puts it, for a display to draw it there.
pub fn voice_at(spread: f64, inner: f64, placement: Placement, k: usize, n: usize) -> f64 {
    spread * placement.side(k, n) * band(inner, placement.place(k, n).abs())
}

/// How far out to either side DOUBLE's pair `out` (0 to 1, [`Placement::pair`]) sits by `spread`
/// and `inner` (0..1): where [`pair_gains`] puts it, for a display to draw it there.
pub fn pair_at(spread: f64, inner: f64, out: f64) -> f64 {
    spread * band(inner, out)
}

/// The gains (left, right) that put voice `k` of `n` in its place, as `placement` has it, by
/// `spread` (0..1) of SPREAD, in its side's band by `inner` (0..1) of INNER ([`voice_at`],
/// [`pan_gains`]; both whole at SPREAD 0, whatever INNER).
pub fn voice_gains(
    spread: f64,
    inner: f64,
    placement: Placement,
    k: usize,
    n: usize,
) -> (f32, f32) {
    if spread > 0.0 {
        pan_gains(0.5 + 0.5 * voice_at(spread, inner, placement, k, n))
    } else {
        (1.0, 1.0)
    }
}

/// DOUBLE's pair `out` (0 to 1, [`Placement::pair`]) by `spread` (0..1), in each side's band by
/// `inner` (0..1, [`pair_at`]): the gains (left, right) of the voice, half the detune flat, that
/// far to the right, and of its twin, half sharp, as far to the left (the CA-74's R28 and R41;
/// K7).
pub fn pair_gains(spread: f64, inner: f64, out: f64) -> [(f32, f32); 2] {
    let at = pair_at(spread, inner, out);
    [pan_gains(0.5 + 0.5 * at), pan_gains(0.5 - 0.5 * at)]
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
            let [voice, twin] = pair_gains(0.8, 0.0, out);
            assert_eq!((voice.0, voice.1), (twin.1, twin.0), "out {out}");
        }
        assert_eq!(pair_gains(0.0, 0.0, 1.0), [pan_gains(0.5); 2]);
        assert_eq!(Placement::Edges.pair(3, 8), 1.0);
        assert_eq!(Placement::Centre.pair(0, 8), 0.0);
        assert_eq!(
            Placement::Even.pair(2, 8),
            Placement::Even.place(2, 8).abs()
        );
        assert_eq!(voice_gains(0.0, 0.0, Placement::Edges, 0, 4), (1.0, 1.0));
        assert_eq!(
            voice_gains(1.0, 0.0, Placement::Edges, 0, 4),
            pan_gains(0.0)
        );
    }

    /// Where a voice's gains put it, -1 (left) to 1 (right), back from the pan law.
    fn heard(g: (f32, f32)) -> f64 {
        4.0 * f64::from(g.1).atan2(f64::from(g.0)) / std::f64::consts::PI - 1.0
    }

    const PLACEMENTS: [Placement; 3] = [Placement::Even, Placement::Edges, Placement::Centre];

    /// INNER at 0 changes nothing: every voice and every pair, at every SPREAD, gets the gains it
    /// got before INNER, to the bit.
    #[test]
    fn inner_at_0_is_as_before_to_the_bit() {
        for placement in PLACEMENTS {
            for n in 1..=12 {
                for k in 0..24 {
                    for s in 0..=10 {
                        let spread = f64::from(s) / 10.0;
                        let before = if spread > 0.0 {
                            pan_gains(0.5 + 0.5 * spread * placement.place(k, n))
                        } else {
                            (1.0, 1.0)
                        };
                        assert_eq!(voice_gains(spread, 0.0, placement, k, n), before);
                        let out = placement.pair(k, n);
                        let pair = [
                            pan_gains(0.5 + 0.5 * spread * out),
                            pan_gains(0.5 - 0.5 * spread * out),
                        ];
                        assert_eq!(pair_gains(spread, 0.0, out), pair);
                    }
                }
            }
        }
    }

    /// INNER clears the centre: with SPREAD at `s` and INNER at `i` every voice sits between `i`
    /// times `s` and `s` out, on its own side, and in the same order across its side as before;
    /// at INNER 1 every voice at SPREAD's edge. At SPREAD 0 both sides are whole, whatever INNER.
    #[test]
    fn inner_keeps_every_voice_in_its_sides_band() {
        for placement in PLACEMENTS {
            for n in 2..=10 {
                for (s, i) in [(1.0, 0.25), (0.7, 0.6), (0.5, 1.0), (1.0, 1.0)] {
                    let at: Vec<f64> = (0..n)
                        .map(|k| heard(voice_gains(s, i, placement, k, n)))
                        .collect();
                    for (k, a) in at.iter().enumerate() {
                        let out = a.abs();
                        assert!(
                            out >= i * s - 1e-6 && out <= s + 1e-6,
                            "{placement:?}, {n} voices, SPREAD {s}, INNER {i}: voice {k} at {a}"
                        );
                        assert_eq!(a.signum(), placement.side(k, n), "voice {k}");
                    }
                    for a in 0..n {
                        for b in 0..n {
                            let (pa, pb) = (placement.place(a, n), placement.place(b, n));
                            if placement.side(a, n) == placement.side(b, n) && pa.abs() < pb.abs() {
                                assert!(at[a].abs() < at[b].abs() + 1e-9, "{placement:?} {n}");
                            }
                        }
                    }
                    if i == 1.0 {
                        assert!(at.iter().all(|a| (a.abs() - s).abs() < 1e-6), "{at:?}");
                    }
                }
                assert_eq!(voice_gains(0.0, 0.8, placement, 0, n), (1.0, 1.0));
            }
        }
    }

    /// What a display draws is where the gains put each voice and each pair.
    #[test]
    fn a_display_draws_each_voice_where_it_is_heard() {
        for placement in PLACEMENTS {
            for n in 2..=10 {
                for k in 0..n {
                    for (s, i) in [(0.3, 0.0), (1.0, 0.5), (0.8, 1.0)] {
                        let at = voice_at(s, i, placement, k, n);
                        let g = voice_gains(s, i, placement, k, n);
                        assert!((heard(g) - at).abs() < 1e-6, "{placement:?} {k}/{n}: {at}");
                        let out = placement.pair(k, n);
                        let a = pair_at(s, i, out);
                        assert!((heard(pair_gains(s, i, out)[0]) - a).abs() < 1e-6, "{a}");
                    }
                }
            }
        }
    }

    /// A voice placed in the centre goes to the side its turn falls on, the left first, and only
    /// as far as INNER takes it, so a little INNER moves it a little: EVEN's fifth of five, and
    /// CENTER's first.
    #[test]
    fn a_voice_in_the_centre_takes_its_turns_side() {
        assert_eq!(Placement::Even.place(4, 5), 0.0);
        assert_eq!(Placement::Even.side(4, 5), -1.0);
        assert_eq!(Placement::Centre.place(0, 8), 0.0);
        assert_eq!(Placement::Centre.side(0, 8), -1.0);
        // A voice past the count takes an earlier one's side, as it takes its place.
        assert_eq!(Placement::Centre.side(8, 8), Placement::Centre.side(0, 8));
        let a = heard(voice_gains(1.0, 0.01, Placement::Even, 4, 5));
        assert!((a + 0.01).abs() < 1e-6, "{a}");
        // Every other voice keeps its place's side.
        for placement in PLACEMENTS {
            for n in 2..=10 {
                for k in 0..n {
                    let p = placement.place(k, n);
                    if p != 0.0 {
                        assert_eq!(placement.side(k, n), p.signum());
                    }
                }
            }
        }
    }

    /// DOUBLE's pairs keep to the band too, still mirrored about the centre: a pair placed in the
    /// centre (CENTER's first voice) opens to INNER's share either side.
    #[test]
    fn a_pair_keeps_to_the_band() {
        for (out, i) in [(0.0, 0.4), (0.5, 0.4), (1.0, 0.4), (0.3, 1.0)] {
            let [voice, twin] = pair_gains(0.8, i, out);
            assert_eq!((voice.0, voice.1), (twin.1, twin.0), "out {out}");
            let a = heard(voice);
            assert!((a - 0.8 * band(i, out)).abs() < 1e-6, "out {out}: {a}");
            assert!(a >= 0.8 * i - 1e-6, "out {out}: {a}");
        }
        assert_eq!(pair_gains(0.0, 0.7, 1.0), [pan_gains(0.5); 2]);
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
