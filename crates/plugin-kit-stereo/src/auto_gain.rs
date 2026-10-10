//! DRIVE's AUTO GAIN (the CA-74's R29): the output brought back down by as much as DRIVE made
//! the sound louder, measured for the sound itself. Off the audio thread, a plug-in's helper
//! plays a few short notes through voices made as its engine's are ([`Measure`], the
//! instrument's own), with the sound's controls, without DRIVE and at each of its steps, and
//! compares their loudness (K-weighted, as BS.1770 weighs it): the correction at each step, a
//! [`Curve`]. The curve is saved with the session ([`Saved`]), so that the session plays and
//! renders the same again; it is measured again only when the sound is changed in the plug-in's
//! own window ([`Calibration::ask`]), never by the host's automation or a learned controller, so
//! that a render does not depend on when a measurement finished. Until a sound has been
//! measured, the instrument's average stands in. The audio thread only reads the curve, without
//! a lock ([`Calibration::read`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The correction at each of `N` steps of DRIVE, dB (0 without DRIVE), the steps evenly apart up
/// to DRIVE's top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve<const N: usize>(pub [f32; N]);

impl<const N: usize> Curve<N> {
    /// The correction at DRIVE `db`, dB, its steps `top` / `N` dB apart up to `top`: drawn
    /// straight between the steps (and from none to the first).
    pub fn at(&self, db: f64, top: f64) -> f64 {
        if !db.is_finite() || db <= 0.0 || N == 0 {
            return 0.0;
        }
        let step = top / N as f64;
        let x = (db.min(top) / step).min(N as f64);
        let i = (x.ceil() as usize).clamp(1, N);
        let below = if i == 1 {
            0.0
        } else {
            f64::from(self.0[i - 2])
        };
        let above = f64::from(self.0[i - 1]);
        below + (above - below) * (x - (i - 1) as f64)
    }
}

/// The curve as a session saves it: the sound it was measured for (0: none, the average's) and
/// its corrections. In JSON, `{"sound": .., "db": [..]}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Saved<const N: usize> {
    pub sound: u64,
    pub db: [f32; N],
}

impl<const N: usize> Serialize for Saved<N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut t = s.serialize_struct("Saved", 2)?;
        t.serialize_field("sound", &self.sound)?;
        t.serialize_field("db", &self.db[..])?;
        t.end()
    }
}

impl<'de, const N: usize> Deserialize<'de> for Saved<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Any {
            sound: u64,
            db: Vec<f32>,
        }
        let a = Any::deserialize(d)?;
        let db = <[f32; N]>::try_from(a.db.as_slice())
            .map_err(|_| serde::de::Error::invalid_length(a.db.len(), &"one correction a step"))?;
        Ok(Saved { sound: a.sound, db })
    }
}

/// An instrument's measurement of a sound: voices made as its engine's are, playing its notes.
pub trait Measure: std::fmt::Debug + Send {
    /// What makes a sound: the controls the voices are made and played with.
    type Sound: Copy + Send + std::fmt::Debug;

    /// For `sound`, with the voices at `rate` Hz.
    fn new(sound: &Self::Sound, rate: f64) -> Self;

    /// The K-weighted energy ([`KWeighted`]) of the sound's notes at DRIVE `db` (dB), each time
    /// from the voices as they were made.
    fn energy(&self, db: f64) -> f64;
}

/// The curve, shared by the editor (which asks for a sound to be measured), the plug-in's helper
/// thread (which measures it), the audio thread (which reads it) and the host (which saves and
/// loads it with the session), for an instrument's sounds measured by `M` at `N` steps.
#[derive(Debug)]
pub struct Calibration<M: Measure, const N: usize> {
    /// DRIVE's top, dB, and the average curve (the correction for a sound not yet measured).
    top: f64,
    average: [f32; N],
    /// The curve: its writes (odd while one is under way: a reader then keeps what it had), the
    /// sound it was measured for and its corrections (f32's bits). One writer at a time.
    writes: AtomicU64,
    sound: AtomicU64,
    db: [AtomicU32; N],
    writing: Mutex<()>,
    /// The sound the curve is wanted for (the editor's latest, or a session's), the one to
    /// measure next if it is not the curve's already, and the rate to measure it at (f64's
    /// bits; 0 before the engine is prepared). A measurement finished for another sound than
    /// the one wanted is let go.
    wanted: AtomicU64,
    asked: Mutex<Option<(u64, M::Sound)>>,
    rate: AtomicU64,
    /// The helper thread, woken when a sound is asked for.
    helper: Mutex<Option<std::thread::Thread>>,
}

impl<M: Measure, const N: usize> Calibration<M, N> {
    /// DRIVE up to `top` dB, `average` the correction until a sound is measured.
    pub fn new(top: f64, average: Curve<N>) -> Self {
        let c = Calibration {
            top,
            average: average.0,
            writes: AtomicU64::new(0),
            sound: AtomicU64::new(0),
            db: std::array::from_fn(|_| AtomicU32::new(0)),
            writing: Mutex::new(()),
            wanted: AtomicU64::new(0),
            asked: Mutex::new(None),
            rate: AtomicU64::new(0),
            helper: Mutex::new(None),
        };
        c.store(&c.unmeasured());
        c
    }

    /// The curve of no sound measured: the average.
    pub fn unmeasured(&self) -> Saved<N> {
        Saved {
            sound: 0,
            db: self.average,
        }
    }

    /// The curve and the sound it was measured for, as it stands (not on the audio thread: it
    /// waits out a write).
    pub fn saved(&self) -> Saved<N> {
        let _w = self.writing.lock();
        Saved {
            sound: self.sound.load(Ordering::Acquire),
            db: std::array::from_fn(|i| f32::from_bits(self.db[i].load(Ordering::Acquire))),
        }
    }

    /// The curve, if it has been written since the reader last had it (`seen`, its writes
    /// then): for the audio thread, without a lock. A write under way leaves it to the next
    /// block.
    pub fn read(&self, seen: &mut u64) -> Option<Curve<N>> {
        let before = self.writes.load(Ordering::Acquire);
        if before == *seen || before % 2 == 1 {
            return None;
        }
        let db: [f32; N] =
            std::array::from_fn(|i| f32::from_bits(self.db[i].load(Ordering::Acquire)));
        std::sync::atomic::fence(Ordering::Acquire);
        if self.writes.load(Ordering::Relaxed) != before {
            return None;
        }
        *seen = before;
        Some(Curve(db))
    }

    /// The curve written: the helper's measurement, or a session's.
    fn store(&self, s: &Saved<N>) {
        let _w = self.writing.lock();
        self.writes.fetch_add(1, Ordering::AcqRel);
        std::sync::atomic::fence(Ordering::Release);
        self.sound.store(s.sound, Ordering::Release);
        for (a, v) in self.db.iter().zip(s.db) {
            let v = if v.is_finite() {
                v.clamp(-60.0, 12.0)
            } else {
                0.0
            };
            a.store(v.to_bits(), Ordering::Release);
        }
        self.writes.fetch_add(1, Ordering::AcqRel);
    }

    /// The sound `c`, its key `key` (never 0; the same for every sound DRIVE makes as loud),
    /// wanted: measured unless the curve is already its (a measurement under way for another
    /// let go), the helper woken. From the editor, once a change made there is done.
    pub fn ask(&self, key: u64, c: &M::Sound) {
        if self.wanted.swap(key, Ordering::AcqRel) == key {
            return;
        }
        let measured = self.sound.load(Ordering::Acquire) == key;
        if let Ok(mut a) = self.asked.lock() {
            *a = (!measured).then_some((key, *c));
        }
        if !measured {
            self.wake();
        }
    }

    /// Whether a sound has been asked for and its measurement not yet begun.
    pub fn waiting(&self) -> bool {
        self.asked.lock().is_ok_and(|a| a.is_some())
    }

    /// A session's curve: the instance's replaced, nothing left to measure (a measurement under
    /// way is for a sound the session has replaced).
    pub fn load(&self, s: &Saved<N>) {
        if let Ok(mut a) = self.asked.lock() {
            self.wanted.store(s.sound, Ordering::Release);
            *a = None;
        }
        self.store(s);
    }

    fn wake(&self) {
        if let Ok(h) = self.helper.lock()
            && let Some(t) = h.as_ref()
        {
            t.unpark();
        }
    }

    /// The rate the voices play at, for measuring at it.
    pub fn set_rate(&self, rate: f64) {
        self.rate.store(rate.to_bits(), Ordering::Release);
    }

    /// The helper thread that measures (None: none now).
    pub fn set_helper(&self, t: Option<std::thread::Thread>) {
        if let Ok(mut h) = self.helper.lock() {
            *h = t;
        }
    }

    /// The measurement a step further, on the helper thread: a sound newly asked for begun
    /// (one under way given up for it), else the next of its renders. Whether there is more
    /// to do.
    pub fn step(&self, job: &mut Option<Job<M, N>>) -> bool {
        let rate = f64::from_bits(self.rate.load(Ordering::Acquire));
        if rate > 0.0
            && let Some((key, c)) = self.asked.lock().ok().and_then(|mut a| a.take())
        {
            *job = Some(Job::new(key, &c, rate, self.top / N as f64));
            return true;
        }
        let Some(j) = job.as_mut() else {
            return false;
        };
        let wanted = self.wanted.load(Ordering::Acquire);
        if j.sound == wanted && j.step() {
            return true;
        }
        if let Ok(_a) = self.asked.lock()
            && j.sound == self.wanted.load(Ordering::Acquire)
            && let Some(saved) = j.done()
        {
            self.store(&saved);
        }
        *job = None;
        false
    }

    /// The sound asked for measured now, on this thread (the tests; an engine without its
    /// helper).
    pub fn measure_now(&self) {
        let mut job = None;
        while self.step(&mut job) {}
    }
}

/// A sound's measurement under way: its key, the voices to copy for each render, DRIVE's step,
/// and the loudness without DRIVE and at the steps measured so far.
#[derive(Debug)]
pub struct Job<M: Measure, const N: usize> {
    sound: u64,
    measure: M,
    step: f64,
    reference: Option<f64>,
    db: Vec<f32>,
}

impl<M: Measure, const N: usize> Job<M, N> {
    fn new(sound: u64, c: &M::Sound, rate: f64, step: f64) -> Self {
        Job {
            sound,
            measure: M::new(c, rate),
            step,
            reference: None,
            db: Vec::with_capacity(N),
        }
    }

    /// The next render: without DRIVE first, then each step. Whether there are more.
    fn step(&mut self) -> bool {
        match self.reference {
            None => {
                self.reference = Some(self.measure.energy(0.0));
                true
            }
            Some(r) if self.db.len() < N => {
                let at = self.step * (self.db.len() + 1) as f64;
                let e = self.measure.energy(at);
                let db = if r > 0.0 && e > 0.0 {
                    10.0 * libm::log10(r / e)
                } else {
                    0.0
                };
                self.db.push(db as f32);
                self.db.len() < N
            }
            Some(_) => false,
        }
    }

    fn done(&self) -> Option<Saved<N>> {
        (self.db.len() == N).then(|| Saved {
            sound: self.sound,
            db: std::array::from_fn(|i| self.db[i]),
        })
    }
}

/// K-weighting (BS.1770's two stages at any rate, as libebur128 derives them) and the energy
/// of what passes through it.
#[derive(Debug, Clone)]
pub struct KWeighted {
    stages: [([f64; 3], [f64; 2]); 2],
    state: [[f64; 4]; 2],
    pub energy: f64,
}

impl KWeighted {
    pub fn new(rate: f64) -> KWeighted {
        use std::f64::consts::PI;
        let (f0, g, q) = (
            1_681.974_450_955_533,
            3.999_843_853_973_347,
            0.707_175_236_955_419_6,
        );
        let k = libm::tan(PI * f0 / rate);
        let vh = libm::pow(10.0, g / 20.0);
        let vb = libm::pow(vh, 0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        let shelf = (
            [
                (vh + vb * k / q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / q + k * k) / a0,
            ],
            [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        );
        let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
        let k = libm::tan(PI * f0 / rate);
        let d = 1.0 + k / q + k * k;
        let high_pass = (
            [1.0, -2.0, 1.0],
            [2.0 * (k * k - 1.0) / d, (1.0 - k / q + k * k) / d],
        );
        KWeighted {
            stages: [shelf, high_pass],
            state: [[0.0; 4]; 2],
            energy: 0.0,
        }
    }

    pub fn push(&mut self, x: f64) {
        let mut y = x;
        for ((b, a), s) in self.stages.iter().zip(&mut self.state) {
            let z = b[0] * y + b[1] * s[0] + b[2] * s[1] - a[0] * s[2] - a[1] * s[3];
            *s = [y, s[0], z, s[2]];
            y = z;
        }
        self.energy += y * y;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A sound measured as a gain: DRIVE makes it louder by `drive`'s share of DRIVE's dB, so its
    /// correction at each step is that share of the step, negative.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Gain {
        drive: f64,
    }

    #[derive(Debug)]
    struct Fake(Gain);

    impl Measure for Fake {
        type Sound = Gain;
        fn new(sound: &Gain, _rate: f64) -> Self {
            Fake(*sound)
        }
        fn energy(&self, db: f64) -> f64 {
            libm::pow(10.0, self.0.drive * db / 10.0)
        }
    }

    const AVERAGE: Curve<4> = Curve([-4.0, -8.0, -10.0, -12.0]);

    fn calibration() -> Calibration<Fake, 4> {
        Calibration::new(24.0, AVERAGE)
    }

    #[test]
    fn the_curve_is_drawn_straight_between_its_steps() {
        let c = Curve([-6.0, -11.0, -15.0, -18.0]);
        let at = |db| c.at(db, 24.0);
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(-3.0), 0.0);
        assert!((at(3.0) + 3.0).abs() < 1e-9);
        assert!((at(6.0) + 6.0).abs() < 1e-9);
        assert!((at(9.0) + 8.5).abs() < 1e-9);
        assert!((at(24.0) + 18.0).abs() < 1e-9);
        assert!((at(30.0) + 18.0).abs() < 1e-9);
        assert_eq!(at(f64::NAN), 0.0);
    }

    #[test]
    fn a_reader_sees_each_write_once_and_never_half_of_one() {
        let c = calibration();
        let mut seen = 0;
        assert_eq!(c.read(&mut seen), Some(AVERAGE));
        assert_eq!(c.read(&mut seen), None);
        c.store(&Saved {
            sound: 7,
            db: [-1.0, -2.0, -3.0, -4.0],
        });
        assert_eq!(c.read(&mut seen), Some(Curve([-1.0, -2.0, -3.0, -4.0])));
        assert_eq!(c.saved().sound, 7);
        // A write under way (odd): the reader keeps what it had.
        c.writes.fetch_add(1, Ordering::AcqRel);
        let mut fresh = 0;
        assert_eq!(c.read(&mut fresh), None);
    }

    #[test]
    fn a_sound_is_asked_for_once_and_not_when_it_is_the_curves() {
        let c = calibration();
        let (a, b) = (Gain { drive: 0.5 }, Gain { drive: 0.25 });
        c.ask(1, &a);
        assert!(c.waiting());
        c.load(&Saved {
            sound: 1,
            db: [0.0; 4],
        });
        assert!(!c.waiting());
        c.ask(2, &b);
        assert!(c.waiting());
        c.ask(1, &a);
        assert!(!c.waiting(), "the curve's own again");
    }

    /// A measurement under way when the sound is changed back to the curve's, or when a session
    /// is loaded, is let go: the curve stays the one wanted. Measured to the end, it is the
    /// sound's.
    #[test]
    fn a_measurement_for_a_sound_no_longer_wanted_is_let_go() {
        let c = calibration();
        c.set_rate(48_000.0);
        let (first, other) = (Gain { drive: 0.5 }, Gain { drive: 0.25 });
        let mut job = None;
        c.load(&Saved {
            sound: 1,
            db: [-1.0; 4],
        });
        c.ask(2, &other);
        assert!(c.step(&mut job) && job.is_some());
        assert!(c.step(&mut job));
        c.ask(1, &first);
        while c.step(&mut job) {}
        assert_eq!(c.saved().db, [-1.0; 4]);
        c.ask(2, &other);
        assert!(c.step(&mut job) && c.step(&mut job));
        c.load(&Saved {
            sound: 99,
            db: [-2.0; 4],
        });
        while c.step(&mut job) {}
        assert_eq!(
            c.saved(),
            Saved {
                sound: 99,
                db: [-2.0; 4]
            }
        );
        c.ask(2, &other);
        c.measure_now();
        let s = c.saved();
        assert_eq!(s.sound, 2);
        for (k, d) in s.db.iter().enumerate() {
            let step = 6.0 * (k + 1) as f64;
            assert!((f64::from(*d) + 0.25 * step).abs() < 1e-5, "{s:?}");
        }
    }

    /// No measurement before the rate is known; none for a sound not asked for.
    #[test]
    fn nothing_is_measured_before_the_rate_or_unasked() {
        let c = calibration();
        let mut job = None;
        assert!(!c.step(&mut job));
        c.ask(3, &Gain { drive: 1.0 });
        assert!(!c.step(&mut job) && c.waiting());
        c.set_rate(44_100.0);
        c.measure_now();
        assert_eq!(c.saved().sound, 3);
    }

    /// A session saves `{"sound": .., "db": [..]}`, and one with another number of steps is not
    /// read as this curve.
    #[test]
    fn a_session_saves_it_and_reads_it_back() {
        let s = Saved {
            sound: 5,
            db: [-1.5f32, -2.0, -3.25, -4.0],
        };
        let t = serde_json::to_string(&s).unwrap();
        assert_eq!(t, r#"{"sound":5,"db":[-1.5,-2.0,-3.25,-4.0]}"#);
        assert_eq!(serde_json::from_str::<Saved<4>>(&t).unwrap(), s);
        assert!(serde_json::from_str::<Saved<4>>(r#"{"sound":5,"db":[-1.0]}"#).is_err());
    }

    /// K-weighting passes the middle of the band at about its level (a 1 kHz sine's mean square,
    /// half its peak's square, plus BS.1770's 0.691 dB less the shelf's small lift there), and
    /// takes away what is under its high-pass.
    #[test]
    fn k_weighting_weighs_as_bs1770() {
        let rate = 48_000.0;
        let energy = |hz: f64| {
            let mut k = KWeighted::new(rate);
            let n = 48_000;
            for i in 0..n {
                k.push((std::f64::consts::TAU * hz * f64::from(i) / rate).sin());
            }
            k.energy / f64::from(n)
        };
        let mid = 10.0 * libm::log10(energy(1000.0) / 0.5);
        assert!((0.0..1.0).contains(&mid), "1 kHz {mid} dB");
        let low = 10.0 * libm::log10(energy(10.0) / 0.5);
        assert!(low < -10.0, "10 Hz {low} dB");
    }
}
